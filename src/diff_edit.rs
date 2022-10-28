// Copyright 2020 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use config::ConfigError;
use itertools::Itertools;
use jujutsu_lib::backend::{TreeId, TreeValue};
use jujutsu_lib::conflicts::{
    describe_conflict, extract_file_conflict_as_single_hunk, materialize_merge_result,
};
use jujutsu_lib::gitignore::GitIgnoreFile;
use jujutsu_lib::matchers::EverythingMatcher;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::Store;
use jujutsu_lib::tree::Tree;
use jujutsu_lib::working_copy::{CheckoutError, SnapshotError, TreeState};
use thiserror::Error;

use crate::ui::Ui;

#[derive(Debug, Error)]
pub enum ExternalToolError {
    #[error("Invalid config: {0}")]
    ConfigError(#[from] ConfigError),
    #[error("Error setting up temporary directory: {0:?}")]
    SetUpDirError(#[source] std::io::Error),
    #[error("Error executing '{tool_binary}': {source}")]
    FailedToExecute {
        tool_binary: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Tool exited with a non-zero code.")]
    ToolAborted,
    #[error("I/O error: {0:?}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum DiffEditError {
    #[error("{0}")]
    ExternalToolError(#[from] ExternalToolError),
    #[error("Failed to write directories to diff: {0:?}")]
    CheckoutError(#[from] CheckoutError),
    #[error("Failed to snapshot changes: {0:?}")]
    SnapshotError(#[from] SnapshotError),
}

#[derive(Debug, Error)]
pub enum ConflictResolveError {
    #[error("{0}")]
    ExternalToolError(#[from] ExternalToolError),
    #[error("Couldn't find the path {0:?} in this revision")]
    PathNotFoundError(RepoPath),
    #[error("Couldn't find any conflicts at {0:?} in this revision")]
    NotAConflictError(RepoPath),
    #[error(
        "Only conflicts that involve normal files (not symlinks, not executable, etc.) are \
         supported. Conflict summary:\n {1}"
    )]
    NotNormalFilesError(RepoPath, String),
    #[error(
        "The conflict at {path:?} has {removes} removes and {adds} adds.\nAt most 1 remove and 2 \
         adds are supported."
    )]
    ConflictTooComplicatedError {
        path: RepoPath,
        removes: usize,
        adds: usize,
    },
    #[error("The output file is either unchanged or empty after the editor quit.")]
    EmptyOrUnchanged,
    #[error("Backend error: {0:?}")]
    BackendError(#[from] jujutsu_lib::backend::BackendError),
}

impl From<std::io::Error> for DiffEditError {
    fn from(err: std::io::Error) -> Self {
        DiffEditError::ExternalToolError(ExternalToolError::from(err))
    }
}
impl From<std::io::Error> for ConflictResolveError {
    fn from(err: std::io::Error) -> Self {
        ConflictResolveError::ExternalToolError(ExternalToolError::from(err))
    }
}

fn check_out(
    store: Arc<Store>,
    wc_dir: PathBuf,
    state_dir: PathBuf,
    tree: &Tree,
    sparse_patterns: Vec<RepoPath>,
) -> Result<TreeState, DiffEditError> {
    std::fs::create_dir(&wc_dir).map_err(ExternalToolError::SetUpDirError)?;
    std::fs::create_dir(&state_dir).map_err(ExternalToolError::SetUpDirError)?;
    let mut tree_state = TreeState::init(store, wc_dir, state_dir);
    tree_state.set_sparse_patterns(sparse_patterns)?;
    tree_state.check_out(tree)?;
    Ok(tree_state)
}

fn set_readonly_recursively(path: &Path) -> Result<(), std::io::Error> {
    // Directory permission is unchanged since files under readonly directory cannot
    // be removed.
    if path.is_dir() {
        for entry in path.read_dir()? {
            set_readonly_recursively(&entry?.path())?;
        }
        Ok(())
    } else {
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(path, perms)
    }
}

pub fn run_mergetool(
    _ui: &mut Ui,
    tree: &Tree,
    repo_path: &RepoPath,
) -> Result<TreeId, ConflictResolveError> {
    let conflict_id = match tree.path_value(repo_path) {
        Some(TreeValue::Conflict(id)) => id,
        Some(_) => return Err(ConflictResolveError::NotAConflictError(repo_path.clone())),
        None => return Err(ConflictResolveError::PathNotFoundError(repo_path.clone())),
    };
    let conflict = tree.store().read_conflict(repo_path, &conflict_id)?;
    let mut content = match extract_file_conflict_as_single_hunk(tree.store(), repo_path, &conflict)
    {
        Some(c) => c,
        _ => {
            let mut summary_bytes: Vec<u8> = vec![];
            describe_conflict(&conflict, &mut summary_bytes)
                .expect("Writing to an in-memory buffer should never fail");
            return Err(ConflictResolveError::NotNormalFilesError(
                repo_path.clone(),
                String::from_utf8_lossy(summary_bytes.as_slice()).to_string(),
            ));
        }
    };
    // The usual case is 1 `removes` and 2 `adds`. 0 `removes` means the file did
    // not exist in the conflict base. Only 1 `adds` may exist for an
    // edit-delete conflict.
    if content.removes.len() > 1 || content.adds.len() > 2 {
        return Err(ConflictResolveError::ConflictTooComplicatedError {
            path: repo_path.clone(),
            removes: content.removes.len(),
            adds: content.adds.len(),
        });
    };

    let mut materialized_conflict: Vec<u8> = vec![];
    materialize_merge_result(&content, &mut materialized_conflict)
        .expect("Writing to an in-memory buffer should never fail");
    let materialized_conflict = materialized_conflict;

    let files: HashMap<&str, _> = maplit::hashmap! {
        "base" => content.removes.pop().unwrap_or_default(),
        "right" => content.adds.pop().unwrap_or_default(),
        "left" => content.adds.pop().unwrap_or_default(),
        "output" => materialized_conflict.clone(),
    };

    let temp_dir = tempfile::Builder::new()
        .prefix("jj-resolve-")
        .tempdir()
        .map_err(ExternalToolError::SetUpDirError)?;
    let suffix = repo_path
        .components()
        .last()
        .map(|filename| format!("_{}", filename.as_str()))
        // The default case below should never actually trigger, but we support it just in case
        // resolving the root path ever makes sense.
        .unwrap_or_default();
    let paths: Result<HashMap<&str, _>, ConflictResolveError> = files
        .iter()
        .map(|(role, contents)| {
            let path = temp_dir.path().join(format!("{role}{suffix}"));
            std::fs::write(&path, contents).map_err(ExternalToolError::SetUpDirError)?;
            if *role != "output" {
                // TODO: Should actually ignore the error here, or have a warning.
                set_readonly_recursively(&path).map_err(ExternalToolError::SetUpDirError)?;
            }
            Ok((*role, path))
        })
        .collect();
    let paths = paths?;

    let progname = "vimdiff";
    let exit_status = Command::new(progname)
        .args(["-f", "-d"])
        .arg(paths.get("output").unwrap())
        .arg("-M")
        .args(["left", "base", "right"].map(|n| paths.get(n).unwrap()))
        .args(["-c", "wincmd J", "-c", "setl modifiable write"])
        .status()
        .map_err(|e| ExternalToolError::FailedToExecute {
            tool_binary: progname.to_string(),
            source: e,
        })?;
    if !exit_status.success() {
        return Err(ConflictResolveError::from(ExternalToolError::ToolAborted));
    }

    let output_file_contents: Vec<u8> = std::fs::read(paths.get("output").unwrap())?;
    if output_file_contents.is_empty() || output_file_contents == materialized_conflict {
        return Err(ConflictResolveError::EmptyOrUnchanged);
    }
    // TODO: parse any remaining conflicts (done in followup commit)
    let new_file_id = tree
        .store()
        .write_file(repo_path, &mut File::open(paths.get("output").unwrap())?)?;
    let mut tree_builder = tree.store().tree_builder(tree.id().clone());
    tree_builder.set(
        repo_path.clone(),
        TreeValue::File {
            id: new_file_id,
            executable: false,
        },
    );
    Ok(tree_builder.write_tree())
}

pub fn edit_diff(
    ui: &mut Ui,
    left_tree: &Tree,
    right_tree: &Tree,
    instructions: &str,
    base_ignores: Arc<GitIgnoreFile>,
) -> Result<TreeId, DiffEditError> {
    let store = left_tree.store();
    let changed_files = left_tree
        .diff(right_tree, &EverythingMatcher)
        .map(|(path, _value)| path)
        .collect_vec();

    // Check out the two trees in temporary directories. Only include changed files
    // in the sparse checkout patterns.
    let temp_dir = tempfile::Builder::new()
        .prefix("jj-diff-edit-")
        .tempdir()
        .map_err(ExternalToolError::SetUpDirError)?;
    let left_wc_dir = temp_dir.path().join("left");
    let left_state_dir = temp_dir.path().join("left_state");
    let right_wc_dir = temp_dir.path().join("right");
    let right_state_dir = temp_dir.path().join("right_state");
    check_out(
        store.clone(),
        left_wc_dir.clone(),
        left_state_dir,
        left_tree,
        changed_files.clone(),
    )?;
    set_readonly_recursively(&left_wc_dir).map_err(ExternalToolError::SetUpDirError)?;
    let mut right_tree_state = check_out(
        store.clone(),
        right_wc_dir.clone(),
        right_state_dir,
        right_tree,
        changed_files,
    )?;
    let instructions_path = right_wc_dir.join("JJ-INSTRUCTIONS");
    // In the unlikely event that the file already exists, then the user will simply
    // not get any instructions.
    let add_instructions = !instructions.is_empty() && !instructions_path.exists();
    if add_instructions {
        // TODO: This can be replaced with std::fs::write. Is this used in other places
        // as well?
        let mut file =
            File::create(&instructions_path).map_err(ExternalToolError::SetUpDirError)?;
        file.write_all(instructions.as_bytes())
            .map_err(ExternalToolError::SetUpDirError)?;
    }

    // TODO: Make this configuration have a table of possible editors and detect the
    // best one here.
    let editor_name = match ui.settings().config().get_string("ui.diff-editor") {
        Ok(editor_binary) => editor_binary,
        Err(_) => {
            let default_editor = "meld".to_string();
            ui.write_hint(format!(
                "Using default editor '{}'; you can change this by setting ui.diff-editor\n",
                default_editor
            ))
            .map_err(ExternalToolError::IoError)?;
            default_editor
        }
    };
    let editor = get_tool(ui.settings(), &editor_name).map_err(ExternalToolError::ConfigError)?;
    // Start a diff editor on the two directories.
    let exit_status = Command::new(&editor.program)
        .args(&editor.edit_args)
        .arg(&left_wc_dir)
        .arg(&right_wc_dir)
        .status()
        .map_err(|e| ExternalToolError::FailedToExecute {
            tool_binary: editor.program.clone(),
            source: e,
        })?;
    if !exit_status.success() {
        return Err(DiffEditError::from(ExternalToolError::ToolAborted));
    }
    if add_instructions {
        std::fs::remove_file(instructions_path).ok();
    }

    right_tree_state.snapshot(base_ignores)?;
    Ok(right_tree_state.current_tree_id().clone())
}

/// Merge/diff tool loaded from the settings.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct MergeTool {
    /// Program to execute.
    pub program: String,
    /// Arguments to pass to the program when editing diffs.
    #[serde(default)]
    pub edit_args: Vec<String>,
}

impl MergeTool {
    pub fn with_program(program: &str) -> Self {
        MergeTool {
            program: program.to_owned(),
            edit_args: vec![],
        }
    }
}

/// Loads merge tool options from `[merge-tools.<name>]`. The given name is used
/// as an executable name if no configuration found for that name.
fn get_tool(settings: &UserSettings, name: &str) -> Result<MergeTool, ConfigError> {
    const TABLE_KEY: &str = "merge-tools";
    let tools_table = match settings.config().get_table(TABLE_KEY) {
        Ok(table) => table,
        Err(ConfigError::NotFound(_)) => return Ok(MergeTool::with_program(name)),
        Err(err) => return Err(err),
    };
    if let Some(v) = tools_table.get(name) {
        v.clone()
            .try_deserialize()
            // add config key, deserialize error is otherwise unclear
            .map_err(|e| ConfigError::Message(format!("{TABLE_KEY}.{name}: {e}")))
    } else {
        Ok(MergeTool::with_program(name))
    }
}
