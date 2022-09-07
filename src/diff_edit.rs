// Copyright 2020 Google LLC
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

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use config::ConfigError;
use itertools::Itertools;
use jujutsu_lib::backend::TreeId;
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
pub enum DiffEditError {
    #[error("Invalid config: {0}")]
    ConfigError(#[from] ConfigError),
    #[error("The diff tool exited with a non-zero code")]
    DifftoolAborted,
    #[error("Failed to write directories to diff: {0:?}")]
    CheckoutError(CheckoutError),
    #[error("Error setting up temporary directory: {0:?}")]
    SetUpDirError(#[source] std::io::Error),
    #[error("Error executing editor '{editor_binary}': {source}")]
    ExecuteEditorError {
        editor_binary: String,
        #[source]
        source: std::io::Error,
    },
    #[error("I/O error: {0:?}")]
    IoError(#[source] std::io::Error),
    #[error("Failed to snapshot changes: {0:?}")]
    SnapshotError(SnapshotError),
}

impl From<CheckoutError> for DiffEditError {
    fn from(err: CheckoutError) -> Self {
        DiffEditError::CheckoutError(err)
    }
}

impl From<SnapshotError> for DiffEditError {
    fn from(err: SnapshotError) -> Self {
        DiffEditError::SnapshotError(err)
    }
}

fn check_out(
    store: Arc<Store>,
    wc_dir: PathBuf,
    state_dir: PathBuf,
    tree: &Tree,
    sparse_patterns: Vec<RepoPath>,
) -> Result<TreeState, DiffEditError> {
    std::fs::create_dir(&wc_dir).map_err(DiffEditError::SetUpDirError)?;
    std::fs::create_dir(&state_dir).map_err(DiffEditError::SetUpDirError)?;
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

pub fn edit_diff(
    ui: &mut Ui,
    settings: &UserSettings,
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
        .map_err(DiffEditError::SetUpDirError)?;
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
    set_readonly_recursively(&left_wc_dir).map_err(DiffEditError::SetUpDirError)?;
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
        let mut file = File::create(&instructions_path).map_err(DiffEditError::SetUpDirError)?;
        file.write_all(instructions.as_bytes())
            .map_err(DiffEditError::SetUpDirError)?;
    }

    // TODO: Make this configuration have a table of possible editors and detect the
    // best one here.
    let editor_name = match settings.config().get_string("ui.diff-editor") {
        Ok(editor_binary) => editor_binary,
        Err(_) => {
            let default_editor = "meld".to_string();
            ui.write_hint(format!(
                "Using default editor '{}'; you can change this by setting ui.diff-editor\n",
                default_editor
            ))
            .map_err(DiffEditError::IoError)?;
            default_editor
        }
    };
    let editor = get_tool(settings, &editor_name)?;
    // Start a diff editor on the two directories.
    let exit_status = Command::new(&editor.program)
        .args(&editor.edit_args)
        .arg(&left_wc_dir)
        .arg(&right_wc_dir)
        .status()
        .map_err(|e| DiffEditError::ExecuteEditorError {
            editor_binary: editor.program,
            source: e,
        })?;
    if !exit_status.success() {
        return Err(DiffEditError::DifftoolAborted);
    }
    if add_instructions {
        std::fs::remove_file(instructions_path).ok();
    }

    Ok(right_tree_state.snapshot(base_ignores)?)
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
