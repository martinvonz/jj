use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;

use config::ConfigError;
use itertools::Itertools;
use jj_lib::backend::{FileId, TreeId, TreeValue};
use jj_lib::conflicts::{self, materialize_merge_result};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::{EverythingMatcher, Matcher};
use jj_lib::merge::Merge;
use jj_lib::repo_path::RepoPath;
use jj_lib::settings::UserSettings;
use jj_lib::store::Store;
use jj_lib::tree::Tree;
use jj_lib::working_copy::{CheckoutError, SnapshotOptions, TreeState, TreeStateError};
use regex::{Captures, Regex};
use tempfile::TempDir;
use thiserror::Error;

use super::{ConflictResolveError, DiffEditError, DiffGenerateError};
use crate::config::CommandNameAndArgs;
use crate::ui::Ui;

/// Merge/diff tool loaded from the settings.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ExternalMergeTool {
    /// Program to execute. Must be defined; defaults to the tool name
    /// if not specified in the config.
    pub program: String,
    /// Arguments to pass to the program when generating diffs.
    /// `$left` and `$right` are replaced with the corresponding directories.
    pub diff_args: Vec<String>,
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

impl Default for ExternalMergeTool {
    fn default() -> Self {
        Self {
            program: String::new(),
            diff_args: ["$left", "$right"].map(ToOwned::to_owned).to_vec(),
            edit_args: ["$left", "$right"].map(ToOwned::to_owned).to_vec(),
            merge_args: vec![],
            merge_tool_edits_conflict_markers: false,
        }
    }
}

impl ExternalMergeTool {
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Default::default()
        }
    }

    pub fn with_diff_args(command_args: &CommandNameAndArgs) -> Self {
        Self::with_args_inner(command_args, |tool| &mut tool.diff_args)
    }

    pub fn with_edit_args(command_args: &CommandNameAndArgs) -> Self {
        Self::with_args_inner(command_args, |tool| &mut tool.edit_args)
    }

    pub fn with_merge_args(command_args: &CommandNameAndArgs) -> Self {
        Self::with_args_inner(command_args, |tool| &mut tool.merge_args)
    }

    fn with_args_inner(
        command_args: &CommandNameAndArgs,
        get_mut_args: impl FnOnce(&mut Self) -> &mut Vec<String>,
    ) -> Self {
        let (name, args) = command_args.split_name_and_args();
        let mut tool = Self {
            program: name.into_owned(),
            ..Default::default()
        };
        if !args.is_empty() {
            *get_mut_args(&mut tool) = args.to_vec();
        }
        tool
    }
}

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
    #[error("{}", format_tool_aborted(.exit_status))]
    ToolAborted { exit_status: ExitStatus },
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

pub fn run_mergetool_external(
    editor: &ExternalMergeTool,
    file_merge: Merge<Option<FileId>>,
    content: jj_lib::merge::Merge<jj_lib::files::ContentHunk>,
    repo_path: &RepoPath,
    conflict: jj_lib::merge::Merge<Option<TreeValue>>,
    tree: &Tree,
) -> Result<TreeId, ConflictResolveError> {
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

    let new_file_ids = if editor.merge_tool_edits_conflict_markers {
        conflicts::update_from_content(
            &file_merge,
            tree.store(),
            repo_path,
            output_file_contents.as_slice(),
        )?
    } else {
        let new_file_id = tree
            .store()
            .write_file(repo_path, &mut output_file_contents.as_slice())?;
        Merge::normal(new_file_id)
    };
    let new_tree_value = match new_file_ids.into_resolved() {
        Ok(new_file_id) => TreeValue::File {
            id: new_file_id.unwrap(),
            executable: false,
        },
        Err(new_file_ids) => {
            let new_conflict = conflict.with_new_file_ids(&new_file_ids);
            let new_conflict_id = tree.store().write_conflict(repo_path, &new_conflict)?;
            TreeValue::Conflict(new_conflict_id)
        }
    };
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

pub fn edit_diff_external(
    editor: ExternalMergeTool,
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

/// Generates textual diff by the specified `tool`, and writes into `writer`.
pub fn generate_diff(
    ui: &Ui,
    writer: &mut dyn Write,
    left_tree: &Tree,
    right_tree: &Tree,
    matcher: &dyn Matcher,
    tool: &ExternalMergeTool,
) -> Result<(), DiffGenerateError> {
    let store = left_tree.store();
    let diff_wc = check_out_trees(store, left_tree, right_tree, matcher)?;
    set_readonly_recursively(diff_wc.left_working_copy_path())
        .map_err(ExternalToolError::SetUpDir)?;
    set_readonly_recursively(diff_wc.right_working_copy_path())
        .map_err(ExternalToolError::SetUpDir)?;
    // TODO: Add support for tools without directory diff functionality?
    // TODO: Somehow propagate --color to the external command?
    let patterns = diff_wc.to_command_variables();
    let mut cmd = Command::new(&tool.program);
    cmd.args(interpolate_variables(&tool.diff_args, &patterns));
    tracing::info!(?cmd, "Invoking the external diff generator:");
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(ui.stderr_for_child().map_err(ExternalToolError::Io)?)
        .spawn()
        .map_err(|source| ExternalToolError::FailedToExecute {
            tool_binary: tool.program.clone(),
            source,
        })?;
    io::copy(&mut child.stdout.take().unwrap(), writer).map_err(ExternalToolError::Io)?;
    // Non-zero exit code isn't an error. For example, the traditional diff command
    // will exit with 1 if inputs are different.
    let exit_status = child.wait().map_err(ExternalToolError::Io)?;
    tracing::info!(?cmd, ?exit_status, "The external diff generator exited:");
    if !exit_status.success() {
        writeln!(ui.warning(), "{}", format_tool_aborted(&exit_status)).ok();
    }
    Ok(())
}

fn format_tool_aborted(exit_status: &ExitStatus) -> String {
    let code = exit_status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    format!(
        "Tool exited with a non-zero code (run with --verbose to see the exact invocation). Exit \
         code: {code}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
