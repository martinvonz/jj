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
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use config::ConfigError;
use itertools::Itertools;
use jj_lib::backend::{TreeId, TreeValue};
use jj_lib::conflicts::materialize_merge_result;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::{EverythingMatcher, Matcher};
use jj_lib::repo_path::RepoPath;
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::store::Store;
use jj_lib::tree::Tree;
use jj_lib::working_copy::{
    CheckoutError, SnapshotError, SnapshotOptions, TreeState, TreeStateError,
};
use regex::{Captures, Regex};
use tempfile::TempDir;
use thiserror::Error;

use crate::config::CommandNameAndArgs;
use crate::ui::Ui;

#[derive(Debug, Error)]
pub enum ExternalToolError {
    #[error("Invalid config: {0}")]
    Config(#[from] ConfigError),
    #[error(
        "To use `{tool_name}` as a merge tool, the config `merge-tools.{tool_name}.merge-args` \
         must be defined (see docs for details)"
    )]
    MergeArgsNotConfigured { tool_name: String },
    #[error("Error setting up temporary directory: {0}")]
    SetUpDir(#[source] std::io::Error),
    // TODO: Remove the "(run with --verbose to see the exact invocation)"
    // from this and other errors. Print it as a hint but only if --verbose is *not* set.
    #[error(
        "Error executing '{tool_binary}' (run with --verbose to see the exact invocation). \
         {source}"
    )]
    FailedToExecute {
        tool_binary: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "Tool exited with a non-zero code (run with --verbose to see the exact invocation). Exit code: {}.",
         exit_status.code().map(|c| c.to_string()).unwrap_or_else(|| "<unknown>".to_string())
    )]
    ToolAborted {
        exit_status: std::process::ExitStatus,
    },
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),
}

#[derive(Debug, Error)]
pub enum DiffCheckoutError {
    #[error("Failed to write directories to diff: {0}")]
    Checkout(#[from] CheckoutError),
    #[error("Error setting up temporary directory: {0}")]
    SetUpDir(#[source] std::io::Error),
    #[error(transparent)]
    TreeState(#[from] TreeStateError),
}

#[derive(Debug, Error)]
pub enum DiffEditError {
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error(transparent)]
    DiffCheckoutError(#[from] DiffCheckoutError),
    #[error("Failed to snapshot changes: {0}")]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Config(#[from] config::ConfigError),
}

#[derive(Debug, Error)]
pub enum ConflictResolveError {
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error("Couldn't find the path {0:?} in this revision")]
    PathNotFound(RepoPath),
    #[error("Couldn't find any conflicts at {0:?} in this revision")]
    NotAConflict(RepoPath),
    #[error(
        "Only conflicts that involve normal files (not symlinks, not executable, etc.) are \
         supported. Conflict summary for {0:?}:\n{1}"
    )]
    NotNormalFiles(RepoPath, String),
    #[error("The conflict at {path:?} has {sides} sides. At most 2 sides are supported.")]
    ConflictTooComplicated { path: RepoPath, sides: usize },
    #[error(
        "The output file is either unchanged or empty after the editor quit (run with --verbose \
         to see the exact invocation)."
    )]
    EmptyOrUnchanged,
    #[error("Backend error: {0}")]
    Backend(#[from] jj_lib::backend::BackendError),
}

struct DiffWorkingCopies {
    _temp_dir: TempDir,
    left_tree_state: TreeState,
    right_tree_state: TreeState,
}

impl DiffWorkingCopies {
    fn left_working_copy_path(&self) -> &Path {
        self.left_tree_state.working_copy_path()
    }

    fn right_working_copy_path(&self) -> &Path {
        self.right_tree_state.working_copy_path()
    }

    fn to_command_variables(&self) -> HashMap<&'static str, &str> {
        let left_wc_dir = self.left_working_copy_path();
        let right_wc_dir = self.right_working_copy_path();
        maplit::hashmap! {
            "left" => left_wc_dir.to_str().expect("temp_dir should be valid utf-8"),
            "right" => right_wc_dir.to_str().expect("temp_dir should be valid utf-8"),
        }
    }
}

/// Check out the two trees in temporary directories. Only include changed files
/// in the sparse checkout patterns.
fn check_out_trees(
    store: &Arc<Store>,
    left_tree: &Tree,
    right_tree: &Tree,
    matcher: &dyn Matcher,
) -> Result<DiffWorkingCopies, DiffCheckoutError> {
    let changed_files = left_tree
        .diff(right_tree, matcher)
        .map(|(path, _value)| path)
        .collect_vec();

    let temp_dir = new_utf8_temp_dir("jj-diff-").map_err(DiffCheckoutError::SetUpDir)?;
    let left_wc_dir = temp_dir.path().join("left");
    let left_state_dir = temp_dir.path().join("left_state");
    let right_wc_dir = temp_dir.path().join("right");
    let right_state_dir = temp_dir.path().join("right_state");
    let left_tree_state = check_out(
        store.clone(),
        left_wc_dir,
        left_state_dir,
        left_tree,
        changed_files.clone(),
    )?;
    let right_tree_state = check_out(
        store.clone(),
        right_wc_dir,
        right_state_dir,
        right_tree,
        changed_files,
    )?;
    Ok(DiffWorkingCopies {
        _temp_dir: temp_dir,
        left_tree_state,
        right_tree_state,
    })
}

fn check_out(
    store: Arc<Store>,
    wc_dir: PathBuf,
    state_dir: PathBuf,
    tree: &Tree,
    sparse_patterns: Vec<RepoPath>,
) -> Result<TreeState, DiffCheckoutError> {
    std::fs::create_dir(&wc_dir).map_err(DiffCheckoutError::SetUpDir)?;
    std::fs::create_dir(&state_dir).map_err(DiffCheckoutError::SetUpDir)?;
    let mut tree_state = TreeState::init(store, wc_dir, state_dir)?;
    tree_state.set_sparse_patterns(sparse_patterns)?;
    tree_state.check_out(tree)?;
    Ok(tree_state)
}

fn new_utf8_temp_dir(prefix: &str) -> io::Result<TempDir> {
    let temp_dir = tempfile::Builder::new().prefix(prefix).tempdir()?;
    if temp_dir.path().to_str().is_none() {
        // Not using .display() as we know the path contains unprintable character
        let message = format!("path {:?} is not valid UTF-8", temp_dir.path());
        return Err(io::Error::new(io::ErrorKind::InvalidData, message));
    }
    Ok(temp_dir)
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
    ui: &Ui,
    tree: &Tree,
    repo_path: &RepoPath,
    settings: &UserSettings,
) -> Result<TreeId, ConflictResolveError> {
    let conflict_id = match tree.path_value(repo_path) {
        Some(TreeValue::Conflict(id)) => id,
        Some(_) => return Err(ConflictResolveError::NotAConflict(repo_path.clone())),
        None => return Err(ConflictResolveError::PathNotFound(repo_path.clone())),
    };
    let conflict = tree.store().read_conflict(repo_path, &conflict_id)?;
    let file_conflict = conflict.to_file_conflict().ok_or_else(|| {
        let mut summary_bytes: Vec<u8> = vec![];
        conflict
            .describe(&mut summary_bytes)
            .expect("Writing to an in-memory buffer should never fail");
        ConflictResolveError::NotNormalFiles(
            repo_path.clone(),
            String::from_utf8_lossy(summary_bytes.as_slice()).to_string(),
        )
    })?;
    // We only support conflicts with 2 sides (3-way conflicts)
    if file_conflict.adds().len() > 2 {
        return Err(ConflictResolveError::ConflictTooComplicated {
            path: repo_path.clone(),
            sides: file_conflict.adds().len(),
        });
    };
    let content = file_conflict.extract_as_single_hunk(tree.store(), repo_path);

    let editor = get_merge_tool_from_settings(ui, settings)?;
    let initial_output_content: Vec<u8> = if editor.merge_tool_edits_conflict_markers {
        let mut materialized_conflict = vec![];
        materialize_merge_result(&content, &mut materialized_conflict)
            .expect("Writing to an in-memory buffer should never fail");
        materialized_conflict
    } else {
        vec![]
    };
    let (mut removes, mut adds) = content.take();
    let files: HashMap<&str, _> = maplit::hashmap! {
        "base" => removes.pop().unwrap().0,
        "right" => adds.pop().unwrap().0,
        "left" => adds.pop().unwrap().0,
        "output" => initial_output_content.clone(),
    };

    let temp_dir = new_utf8_temp_dir("jj-resolve-").map_err(ExternalToolError::SetUpDir)?;
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
            std::fs::write(&path, contents).map_err(ExternalToolError::SetUpDir)?;
            if *role != "output" {
                // TODO: Should actually ignore the error here, or have a warning.
                set_readonly_recursively(&path).map_err(ExternalToolError::SetUpDir)?;
            }
            Ok((
                *role,
                path.into_os_string()
                    .into_string()
                    .expect("temp_dir should be valid utf-8"),
            ))
        })
        .try_collect()?;

    let mut cmd = Command::new(&editor.program);
    cmd.args(interpolate_variables(&editor.merge_args, &paths));
    tracing::info!(?cmd, "Invoking the external merge tool:");
    let exit_status = cmd
        .status()
        .map_err(|e| ExternalToolError::FailedToExecute {
            tool_binary: editor.program.clone(),
            source: e,
        })?;
    if !exit_status.success() {
        return Err(ConflictResolveError::from(ExternalToolError::ToolAborted {
            exit_status,
        }));
    }

    let output_file_contents: Vec<u8> =
        std::fs::read(paths.get("output").unwrap()).map_err(ExternalToolError::Io)?;
    if output_file_contents.is_empty() || output_file_contents == initial_output_content {
        return Err(ConflictResolveError::EmptyOrUnchanged);
    }

    let mut new_tree_value: Option<TreeValue> = None;
    if editor.merge_tool_edits_conflict_markers {
        if let Some(new_conflict) = conflict.update_from_content(
            tree.store(),
            repo_path,
            output_file_contents.as_slice(),
        )? {
            let new_conflict_id = tree.store().write_conflict(repo_path, &new_conflict)?;
            new_tree_value = Some(TreeValue::Conflict(new_conflict_id));
        }
    }
    let new_tree_value = new_tree_value.unwrap_or({
        let new_file_id = tree
            .store()
            .write_file(repo_path, &mut output_file_contents.as_slice())?;
        TreeValue::File {
            id: new_file_id,
            executable: false,
        }
    });
    let mut tree_builder = tree.store().tree_builder(tree.id().clone());
    tree_builder.set(repo_path.clone(), new_tree_value);
    Ok(tree_builder.write_tree())
}

fn interpolate_variables<V: AsRef<str>>(
    args: &[String],
    variables: &HashMap<&str, V>,
) -> Vec<String> {
    // Not interested in $UPPER_CASE_VARIABLES
    let re = Regex::new(r"\$([a-z0-9_]+)\b").unwrap();
    args.iter()
        .map(|arg| {
            re.replace_all(arg, |caps: &Captures| {
                let name = &caps[1];
                if let Some(subst) = variables.get(name) {
                    subst.as_ref().to_owned()
                } else {
                    caps[0].to_owned()
                }
            })
            .into_owned()
        })
        .collect()
}

pub fn edit_diff(
    ui: &Ui,
    left_tree: &Tree,
    right_tree: &Tree,
    instructions: &str,
    base_ignores: Arc<GitIgnoreFile>,
    settings: &UserSettings,
) -> Result<TreeId, DiffEditError> {
    let store = left_tree.store();
    let diff_wc = check_out_trees(store, left_tree, right_tree, &EverythingMatcher)?;
    set_readonly_recursively(diff_wc.left_working_copy_path())
        .map_err(ExternalToolError::SetUpDir)?;
    let instructions_path = diff_wc.right_working_copy_path().join("JJ-INSTRUCTIONS");
    // In the unlikely event that the file already exists, then the user will simply
    // not get any instructions.
    let add_instructions =
        settings.diff_instructions() && !instructions.is_empty() && !instructions_path.exists();
    if add_instructions {
        // TODO: This can be replaced with std::fs::write. Is this used in other places
        // as well?
        let mut file = File::create(&instructions_path).map_err(ExternalToolError::SetUpDir)?;
        file.write_all(instructions.as_bytes())
            .map_err(ExternalToolError::SetUpDir)?;
    }

    // Start a diff editor on the two directories.
    let editor = get_diff_editor_from_settings(ui, settings)?;
    let patterns = diff_wc.to_command_variables();
    let mut cmd = Command::new(&editor.program);
    cmd.args(interpolate_variables(&editor.edit_args, &patterns));
    tracing::info!(?cmd, "Invoking the external diff editor:");
    let exit_status = cmd
        .status()
        .map_err(|e| ExternalToolError::FailedToExecute {
            tool_binary: editor.program.clone(),
            source: e,
        })?;
    if !exit_status.success() {
        return Err(DiffEditError::from(ExternalToolError::ToolAborted {
            exit_status,
        }));
    }
    if add_instructions {
        std::fs::remove_file(instructions_path).ok();
    }

    let mut right_tree_state = diff_wc.right_tree_state;
    right_tree_state.snapshot(SnapshotOptions {
        base_ignores,
        fsmonitor_kind: settings.fsmonitor_kind()?,
        progress: None,
    })?;
    Ok(right_tree_state.current_tree_id().clone())
}

/// Merge/diff tool loaded from the settings.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct MergeTool {
    /// Program to execute. Must be defined; defaults to the tool name
    /// if not specified in the config.
    pub program: String,
    /// Arguments to pass to the program when editing diffs.
    /// `$left` and `$right` are replaced with the corresponding directories.
    pub edit_args: Vec<String>,
    /// Arguments to pass to the program when resolving 3-way conflicts.
    /// `$left`, `$right`, `$base`, and `$output` are replaced with
    /// paths to the corresponding files.
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
    pub merge_tool_edits_conflict_markers: bool,
}

impl Default for MergeTool {
    fn default() -> Self {
        MergeTool {
            program: String::new(),
            edit_args: ["$left", "$right"].map(ToOwned::to_owned).to_vec(),
            merge_args: vec![],
            merge_tool_edits_conflict_markers: false,
        }
    }
}

impl MergeTool {
    pub fn with_edit_args(command_args: &CommandNameAndArgs) -> Self {
        let (name, args) = command_args.split_name_and_args();
        let mut tool = MergeTool {
            program: name.into_owned(),
            ..Default::default()
        };
        if !args.is_empty() {
            tool.edit_args = args.to_vec();
        }
        tool
    }

    pub fn with_merge_args(command_args: &CommandNameAndArgs) -> Self {
        let (name, args) = command_args.split_name_and_args();
        let mut tool = MergeTool {
            program: name.into_owned(),
            ..Default::default()
        };
        if !args.is_empty() {
            tool.merge_args = args.to_vec();
        }
        tool
    }
}

/// Loads merge tool options from `[merge-tools.<name>]`.
fn get_tool_config(settings: &UserSettings, name: &str) -> Result<Option<MergeTool>, ConfigError> {
    const TABLE_KEY: &str = "merge-tools";
    let tools_table = settings.config().get_table(TABLE_KEY)?;
    if let Some(v) = tools_table.get(name) {
        let mut result: MergeTool = v
            .clone()
            .try_deserialize()
            // add config key, deserialize error is otherwise unclear
            .map_err(|e| ConfigError::Message(format!("{TABLE_KEY}.{name}: {e}")))?;

        if result.program.is_empty() {
            result.program.clone_from(&name.to_string());
        };
        Ok(Some(result))
    } else {
        Ok(None)
    }
}

fn get_diff_editor_from_settings(
    ui: &Ui,
    settings: &UserSettings,
) -> Result<MergeTool, ExternalToolError> {
    let args = editor_args_from_settings(ui, settings, "ui.diff-editor")?;
    let maybe_editor = match &args {
        CommandNameAndArgs::String(name) => get_tool_config(settings, name)?,
        CommandNameAndArgs::Vec(_) => None,
        CommandNameAndArgs::Structured { .. } => None,
    };
    Ok(maybe_editor.unwrap_or_else(|| MergeTool::with_edit_args(&args)))
}

fn get_merge_tool_from_settings(
    ui: &Ui,
    settings: &UserSettings,
) -> Result<MergeTool, ExternalToolError> {
    let args = editor_args_from_settings(ui, settings, "ui.merge-editor")?;
    let maybe_editor = match &args {
        CommandNameAndArgs::String(name) => get_tool_config(settings, name)?,
        CommandNameAndArgs::Vec(_) => None,
        CommandNameAndArgs::Structured { .. } => None,
    };
    let editor = maybe_editor.unwrap_or_else(|| MergeTool::with_merge_args(&args));
    if editor.merge_args.is_empty() {
        Err(ExternalToolError::MergeArgsNotConfigured {
            tool_name: args.to_string(),
        })
    } else {
        Ok(editor)
    }
}

/// Finds the appropriate tool for diff editing or merges
fn editor_args_from_settings(
    ui: &Ui,
    settings: &UserSettings,
    key: &str,
) -> Result<CommandNameAndArgs, ExternalToolError> {
    // TODO: Make this configuration have a table of possible editors and detect the
    // best one here.
    if let Some(args) = settings.config().get(key).optional()? {
        Ok(args)
    } else {
        let default_editor = "meld";
        writeln!(
            ui.hint(),
            "Using default editor '{default_editor}'; you can change this by setting {key}"
        )
        .map_err(ExternalToolError::Io)?;
        Ok(default_editor.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from_string(text: &str) -> config::Config {
        config::Config::builder()
            // Load defaults to test the default args lookup
            .add_source(crate::config::default_config())
            .add_source(config::File::from_str(text, config::FileFormat::Toml))
            .build()
            .unwrap()
    }

    #[test]
    fn test_get_diff_editor() {
        let get = |text| {
            let config = config_from_string(text);
            let ui = Ui::with_config(&config).unwrap();
            let settings = UserSettings::from_config(config);
            get_diff_editor_from_settings(&ui, &settings)
        };

        // Default
        insta::assert_debug_snapshot!(get("").unwrap(), @r###"
        MergeTool {
            program: "meld",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [
                "$left",
                "$base",
                "$right",
                "-o",
                "$output",
                "--auto-merge",
            ],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // Just program name, edit_args are filled by default
        insta::assert_debug_snapshot!(get(r#"ui.diff-editor = "my-diff""#).unwrap(), @r###"
        MergeTool {
            program: "my-diff",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // String args (with interpolation variables)
        insta::assert_debug_snapshot!(
            get(r#"ui.diff-editor = "my-diff -l $left -r $right""#).unwrap(), @r###"
        MergeTool {
            program: "my-diff",
            edit_args: [
                "-l",
                "$left",
                "-r",
                "$right",
            ],
            merge_args: [],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // List args (with interpolation variables)
        insta::assert_debug_snapshot!(
            get(r#"ui.diff-editor = ["my-diff", "--diff", "$left", "$right"]"#).unwrap(), @r###"
        MergeTool {
            program: "my-diff",
            edit_args: [
                "--diff",
                "$left",
                "$right",
            ],
            merge_args: [],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // Pick from merge-tools
        insta::assert_debug_snapshot!(get(
        r#"
        ui.diff-editor = "foo bar"
        [merge-tools."foo bar"]
        edit-args = ["--edit", "args", "$left", "$right"]
        "#,
        ).unwrap(), @r###"
        MergeTool {
            program: "foo bar",
            edit_args: [
                "--edit",
                "args",
                "$left",
                "$right",
            ],
            merge_args: [],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // Pick from merge-tools, but no edit-args specified
        insta::assert_debug_snapshot!(get(
        r#"
        ui.diff-editor = "my-diff"
        [merge-tools.my-diff]
        program = "MyDiff"
        "#,
        ).unwrap(), @r###"
        MergeTool {
            program: "MyDiff",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // List args should never be a merge-tools key, edit_args are filled by default
        insta::assert_debug_snapshot!(get(r#"ui.diff-editor = ["meld"]"#).unwrap(), @r###"
        MergeTool {
            program: "meld",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // Invalid type
        assert!(get(r#"ui.diff-editor.k = 0"#).is_err());
    }

    #[test]
    fn test_get_merge_tool() {
        let get = |text| {
            let config = config_from_string(text);
            let ui = Ui::with_config(&config).unwrap();
            let settings = UserSettings::from_config(config);
            get_merge_tool_from_settings(&ui, &settings)
        };

        // Default
        insta::assert_debug_snapshot!(get("").unwrap(), @r###"
        MergeTool {
            program: "meld",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [
                "$left",
                "$base",
                "$right",
                "-o",
                "$output",
                "--auto-merge",
            ],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // Just program name
        insta::assert_debug_snapshot!(get(r#"ui.merge-editor = "my-merge""#).unwrap_err(), @r###"
        MergeArgsNotConfigured {
            tool_name: "my-merge",
        }
        "###);

        // String args
        insta::assert_debug_snapshot!(
            get(r#"ui.merge-editor = "my-merge $left $base $right $output""#).unwrap(), @r###"
        MergeTool {
            program: "my-merge",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [
                "$left",
                "$base",
                "$right",
                "$output",
            ],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // List args
        insta::assert_debug_snapshot!(
            get(
                r#"ui.merge-editor = ["my-merge", "$left", "$base", "$right", "$output"]"#,
            ).unwrap(), @r###"
        MergeTool {
            program: "my-merge",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [
                "$left",
                "$base",
                "$right",
                "$output",
            ],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // Pick from merge-tools
        insta::assert_debug_snapshot!(get(
        r#"
        ui.merge-editor = "foo bar"
        [merge-tools."foo bar"]
        merge-args = ["$base", "$left", "$right", "$output"]
        "#,
        ).unwrap(), @r###"
        MergeTool {
            program: "foo bar",
            edit_args: [
                "$left",
                "$right",
            ],
            merge_args: [
                "$base",
                "$left",
                "$right",
                "$output",
            ],
            merge_tool_edits_conflict_markers: false,
        }
        "###);

        // List args should never be a merge-tools key
        insta::assert_debug_snapshot!(
            get(r#"ui.merge-editor = ["meld"]"#).unwrap_err(), @r###"
        MergeArgsNotConfigured {
            tool_name: "meld",
        }
        "###);

        // Invalid type
        assert!(get(r#"ui.merge-editor.k = 0"#).is_err());
    }

    #[test]
    fn test_interpolate_variables() {
        let patterns = maplit::hashmap! {
            "left" => "LEFT",
            "right" => "RIGHT",
            "left_right" => "$left $right",
        };

        assert_eq!(
            interpolate_variables(
                &["$left", "$1", "$right", "$2"].map(ToOwned::to_owned),
                &patterns
            ),
            ["LEFT", "$1", "RIGHT", "$2"],
        );

        // Option-like
        assert_eq!(
            interpolate_variables(&["-o$left$right".to_owned()], &patterns),
            ["-oLEFTRIGHT"],
        );

        // Sexp-like
        assert_eq!(
            interpolate_variables(&["($unknown $left $right)".to_owned()], &patterns),
            ["($unknown LEFT RIGHT)"],
        );

        // Not a word "$left"
        assert_eq!(
            interpolate_variables(&["$lefty".to_owned()], &patterns),
            ["$lefty"],
        );

        // Patterns in pattern: not expanded recursively
        assert_eq!(
            interpolate_variables(&["$left_right".to_owned()], &patterns),
            ["$left $right"],
        );
    }
}
