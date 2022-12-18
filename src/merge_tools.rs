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
    update_conflict_from_content,
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
    #[error("Error executing '{tool_binary}' with args {args:?}: {source}")]
    FailedToExecute {
        tool_binary: String,
        args: Vec<String>,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "Tool exited with a non-zero code.\n Details: tool '{tool_binary}' was \
         executed with args {args:?}. Exit code: {}.",
         exit_status.code().map(|c| c.to_string()).unwrap_or_else(|| "<unknown>".to_string())
    )]
    ToolAborted {
        tool_binary: String,
        args: Vec<String>,
        exit_status: std::process::ExitStatus,
    },
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
    #[error(
        "To use `{tool_name}` as a merge tool, the config `merge-tools.{tool_name}.merge-args` \
         must be defined (see docs for details)"
    )]
    MergeArgsNotConfigured { tool_name: String },
    #[error("Couldn't find the path {0:?} in this revision")]
    PathNotFoundError(RepoPath),
    #[error("Couldn't find any conflicts at {0:?} in this revision")]
    NotAConflictError(RepoPath),
    #[error(
        "Only conflicts that involve normal files (not symlinks, not executable, etc.) are \
         supported. Conflict summary for {0:?}:\n{1}"
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

// TODO: Rearrange the functions. This should be on the bottom, options should
// be on the top.
pub fn run_mergetool(
    ui: &mut Ui,
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

    let editor = get_merge_tool_from_settings(ui)?;
    let initial_output_content: Vec<u8> = if editor.merge_tool_edits_conflict_markers {
        let mut materialized_conflict = vec![];
        materialize_merge_result(&content, &mut materialized_conflict)
            .expect("Writing to an in-memory buffer should never fail");
        materialized_conflict
    } else {
        vec![]
    };
    let files: HashMap<&str, _> = maplit::hashmap! {
        "base" => content.removes.pop().unwrap_or_default(),
        "right" => content.adds.pop().unwrap_or_default(),
        "left" => content.adds.pop().unwrap_or_default(),
        "output" => initial_output_content.clone(),
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
    let paths: HashMap<&str, _> = files
        .iter()
        .map(|(role, contents)| -> Result<_, ConflictResolveError> {
            let path = temp_dir.path().join(format!("{role}{suffix}"));
            std::fs::write(&path, contents).map_err(ExternalToolError::SetUpDirError)?;
            if *role != "output" {
                // TODO: Should actually ignore the error here, or have a warning.
                set_readonly_recursively(&path).map_err(ExternalToolError::SetUpDirError)?;
            }
            Ok((*role, path))
        })
        .try_collect()?;

    let args = interpolate_mergetool_filename_patterns(&editor.merge_args, &paths);
    let args_str = args
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect_vec();
    let exit_status = Command::new(&editor.program)
        .args(args)
        .status()
        .map_err(|e| ExternalToolError::FailedToExecute {
            tool_binary: editor.program.clone(),
            args: args_str.clone(),
            source: e,
        })?;
    if !exit_status.success() {
        return Err(ConflictResolveError::from(ExternalToolError::ToolAborted {
            tool_binary: editor.program,
            args: args_str,
            exit_status,
        }));
    }

    let output_file_contents: Vec<u8> = std::fs::read(paths.get("output").unwrap())?;
    if output_file_contents.is_empty() || output_file_contents == initial_output_content {
        return Err(ConflictResolveError::EmptyOrUnchanged);
    }

    let mut new_tree_value: Option<TreeValue> = None;
    if editor.merge_tool_edits_conflict_markers {
        if let Some(new_conflict_id) = update_conflict_from_content(
            tree.store(),
            repo_path,
            &conflict_id,
            output_file_contents.as_slice(),
        )? {
            new_tree_value = Some(TreeValue::Conflict(new_conflict_id));
        }
    }
    let new_tree_value = new_tree_value.unwrap_or({
        let new_file_id = tree
            .store()
            .write_file(repo_path, &mut File::open(paths.get("output").unwrap())?)?;
        TreeValue::File {
            id: new_file_id,
            executable: false,
        }
    });
    let mut tree_builder = tree.store().tree_builder(tree.id().clone());
    tree_builder.set(repo_path.clone(), new_tree_value);
    Ok(tree_builder.write_tree())
}

fn interpolate_mergetool_filename_patterns<T: std::str::FromStr + From<PathBuf>>(
    merge_args: &[String],
    paths: &HashMap<&str, PathBuf>,
) -> Vec<T>
where
    Vec<T>: FromIterator<PathBuf>,
{
    merge_args
        .iter()
        .map(|arg| {
            // TODO: Match all instances of `\$\w+` pattern and replace them
            // so that portions of args can be replaced, and so that file paths
            // that include the '$' character are processed correctly.
            arg.strip_prefix('$')
                .and_then(|p| paths.get(p))
                .and_then(|p| From::from(p.clone()))
                .unwrap_or_else(|| From::from(arg.clone()))
        })
        .collect()
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

    let editor = get_diff_editor_from_settings(ui)?;
    let mut args_str = editor.edit_args.clone();
    args_str.push(left_wc_dir.display().to_string());
    args_str.push(right_wc_dir.display().to_string());
    // Start a diff editor on the two directories.
    let exit_status = Command::new(&editor.program)
        .args(&editor.edit_args)
        .arg(&left_wc_dir)
        .arg(&right_wc_dir)
        .status()
        .map_err(|e| ExternalToolError::FailedToExecute {
            tool_binary: editor.program.clone(),
            args: args_str.clone(),
            source: e,
        })?;
    if !exit_status.success() {
        return Err(DiffEditError::from(ExternalToolError::ToolAborted {
            tool_binary: editor.program,
            args: args_str,
            exit_status,
        }));
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
    /// Program to execute. Must be defined; defaults to the tool name
    /// if not specified in the config.
    #[serde(default)]
    pub program: String,
    /// Arguments to pass to the program when editing diffs.
    #[serde(default)]
    pub edit_args: Vec<String>,
    /// Arguments to pass to the program when resolving 3-way conflicts.
    /// `$left`, `$right`, `$base`, and `$output` are replaced with
    /// paths to the corresponding files.
    /// TODO: Currently, the entire argument has to match one of these 4
    /// strings to be substituted.
    #[serde(default)]
    pub merge_args: Vec<String>,
    /// If false (default), the `$output` file starts out empty and is accepted
    /// as a full conflict resolution as-is by `jj` after the merge tool is
    /// done with it. If true, the `$output` file starts out with the
    /// contents of the conflict, with JJ's conflict markers. After the
    /// merge tool is done, any remaining conflict markers in the
    /// file parsed and taken to mean that the conflict was only partially
    /// resolved.
    // TODO: Instead of a boolean, this could denote the flavor of conflict markers to put in
    // the file (`jj` or `diff3` for example).
    #[serde(default)]
    pub merge_tool_edits_conflict_markers: bool,
}

impl MergeTool {
    pub fn with_program(program: &str) -> Self {
        MergeTool {
            program: program.to_owned(),
            edit_args: vec![],
            merge_args: vec![],
            merge_tool_edits_conflict_markers: false,
        }
    }
}

/// Loads merge tool options from `[merge-tools.<name>]`. The given name is used
/// as an executable name if no configuration found for that name.
fn get_tool_config(settings: &UserSettings, name: &str) -> Result<MergeTool, ConfigError> {
    const TABLE_KEY: &str = "merge-tools";
    let tools_table = match settings.config().get_table(TABLE_KEY) {
        Ok(table) => table,
        Err(ConfigError::NotFound(_)) => return Ok(MergeTool::with_program(name)),
        Err(err) => return Err(err),
    };
    if let Some(v) = tools_table.get(name) {
        let mut result: MergeTool = v
            .clone()
            .try_deserialize()
            // add config key, deserialize error is otherwise unclear
            .map_err(|e| ConfigError::Message(format!("{TABLE_KEY}.{name}: {e}")))?;

        if result.program.is_empty() {
            result.program.clone_from(&name.to_string());
        };
        Ok(result)
    } else {
        Ok(MergeTool::with_program(name))
    }
}

fn get_diff_editor_from_settings(ui: &mut Ui) -> Result<MergeTool, ExternalToolError> {
    let editor_name = editor_name_from_settings(ui, "diff")?;
    Ok(get_tool_config(ui.settings(), &editor_name)?)
}

fn get_merge_tool_from_settings(ui: &mut Ui) -> Result<MergeTool, ConflictResolveError> {
    let editor_name = editor_name_from_settings(ui, "merge")?;
    let editor =
        get_tool_config(ui.settings(), &editor_name).map_err(ExternalToolError::ConfigError)?;
    if editor.merge_args.is_empty() {
        Err(ConflictResolveError::MergeArgsNotConfigured {
            tool_name: editor_name,
        })
    } else {
        Ok(editor)
    }
}

/// Finds the appropriate tool for diff editing or merges
fn editor_name_from_settings(
    ui: &mut Ui,
    diff_or_merge: &str,
) -> Result<String, ExternalToolError> {
    // TODO: Make this configuration have a table of possible editors and detect the
    // best one here.
    match ui
        .settings()
        .config()
        .get_string(&format!("ui.{diff_or_merge}-editor"))
    {
        Ok(editor_binary) => Ok(editor_binary),
        Err(_) => {
            let default_editor = "meld".to_string();
            ui.write_hint(format!(
                "Using default editor '{default_editor}'; you can change this by setting \
                 ui.{diff_or_merge}-editor\n"
            ))?;
            Ok(default_editor)
        }
    }
}
