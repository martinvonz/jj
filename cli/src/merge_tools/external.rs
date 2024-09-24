use std::collections::HashMap;
use std::io;
use std::io::Write;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;

use bstr::BString;
use itertools::Itertools;
use jj_lib::backend::FileId;
use jj_lib::backend::MergedTreeId;
use jj_lib::backend::TreeValue;
use jj_lib::conflicts;
use jj_lib::conflicts::materialize_merge_result;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::Matcher;
use jj_lib::merge::Merge;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree::MergedTreeBuilder;
use jj_lib::repo_path::RepoPath;
use pollster::FutureExt;
use thiserror::Error;

use super::diff_working_copies::check_out_trees;
use super::diff_working_copies::new_utf8_temp_dir;
use super::diff_working_copies::set_readonly_recursively;
use super::diff_working_copies::DiffEditWorkingCopies;
use super::diff_working_copies::DiffSide;
use super::ConflictResolveError;
use super::DiffEditError;
use super::DiffGenerateError;
use crate::config::find_all_variables;
use crate::config::interpolate_variables;
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
    /// Whether to execute the tool with a pair of directories or individual
    /// files.
    pub diff_invocation_mode: DiffToolMode,
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

#[derive(serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DiffToolMode {
    /// Invoke the diff tool on a temp directory of the modified files.
    Dir,
    /// Invoke the diff tool on each of the modified files individually.
    FileByFile,
}

impl Default for ExternalMergeTool {
    fn default() -> Self {
        Self {
            program: String::new(),
            // TODO(ilyagr): There should be a way to explicitly specify that a
            // certain tool (e.g. vscode as of this writing) cannot be used as a
            // diff editor (or a diff tool). A possible TOML syntax would be
            // `edit-args = false`, or `edit-args = []`, or `edit = { disabled =
            // true }` to go with `edit = { args = [...] }`.
            diff_args: ["$left", "$right"].map(ToOwned::to_owned).to_vec(),
            edit_args: ["$left", "$right"].map(ToOwned::to_owned).to_vec(),
            merge_args: vec![],
            merge_tool_edits_conflict_markers: false,
            diff_invocation_mode: DiffToolMode::Dir,
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
    #[error("Error setting up temporary directory")]
    SetUpDir(#[source] std::io::Error),
    // TODO: Remove the "(run with --debug to see the exact invocation)"
    // from this and other errors. Print it as a hint but only if --debug is *not* set.
    #[error("Error executing '{tool_binary}' (run with --debug to see the exact invocation)")]
    FailedToExecute {
        tool_binary: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Tool exited with {exit_status} (run with --debug to see the exact invocation)")]
    ToolAborted { exit_status: ExitStatus },
    #[error("I/O error")]
    Io(#[source] std::io::Error),
}

pub fn run_mergetool_external(
    editor: &ExternalMergeTool,
    file_merge: Merge<Option<FileId>>,
    content: Merge<BString>,
    repo_path: &RepoPath,
    conflict: MergedTreeValue,
    tree: &MergedTree,
) -> Result<MergedTreeId, ConflictResolveError> {
    let initial_output_content: Vec<u8> = if editor.merge_tool_edits_conflict_markers {
        let mut materialized_conflict = vec![];
        materialize_merge_result(&content, &mut materialized_conflict)
            .expect("Writing to an in-memory buffer should never fail");
        materialized_conflict
    } else {
        vec![]
    };
    assert_eq!(content.num_sides(), 2);
    let files: HashMap<&str, &[u8]> = maplit::hashmap! {
        "base" => content.get_remove(0).unwrap().as_slice(),
        "left" => content.get_add(0).unwrap().as_slice(),
        "right" => content.get_add(1).unwrap().as_slice(),
        "output" => initial_output_content.as_slice(),
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
        )
        .block_on()?
    } else {
        let new_file_id = tree
            .store()
            .write_file(repo_path, &mut output_file_contents.as_slice())
            .block_on()?;
        Merge::normal(new_file_id)
    };
    let new_tree_value = match new_file_ids.into_resolved() {
        Ok(new_file_id) => Merge::normal(TreeValue::File {
            id: new_file_id.unwrap(),
            executable: false,
        }),
        Err(new_file_ids) => conflict.with_new_file_ids(&new_file_ids),
    };
    let mut tree_builder = MergedTreeBuilder::new(tree.id());
    tree_builder.set_or_remove(repo_path.to_owned(), new_tree_value);
    let new_tree = tree_builder.write_tree(tree.store())?;
    Ok(new_tree)
}

pub fn edit_diff_external(
    editor: &ExternalMergeTool,
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    matcher: &dyn Matcher,
    instructions: Option<&str>,
    base_ignores: Arc<GitIgnoreFile>,
    exec_config: Option<bool>,
) -> Result<MergedTreeId, DiffEditError> {
    let got_output_field = find_all_variables(&editor.edit_args).contains(&"output");
    let store = left_tree.store();
    let diffedit_wc = DiffEditWorkingCopies::check_out(
        store,
        left_tree,
        right_tree,
        matcher,
        got_output_field.then_some(DiffSide::Right),
        instructions,
        exec_config,
    )?;

    let patterns = diffedit_wc.working_copies.to_command_variables();
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

    diffedit_wc.snapshot_results(base_ignores)
}

/// Generates textual diff by the specified `tool` and writes into `writer`.
pub fn generate_diff(
    ui: &Ui,
    writer: &mut dyn Write,
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    matcher: &dyn Matcher,
    tool: &ExternalMergeTool,
) -> Result<(), DiffGenerateError> {
    let store = left_tree.store();
    let diff_wc = check_out_trees(store, left_tree, right_tree, matcher, None, ui.exec_config)?;
    set_readonly_recursively(diff_wc.left_working_copy_path())
        .map_err(ExternalToolError::SetUpDir)?;
    set_readonly_recursively(diff_wc.right_working_copy_path())
        .map_err(ExternalToolError::SetUpDir)?;
    invoke_external_diff(ui, writer, tool, &diff_wc.to_command_variables())
}

/// Invokes the specified `tool` directing its output into `writer`.
pub fn invoke_external_diff(
    ui: &Ui,
    writer: &mut dyn Write,
    tool: &ExternalMergeTool,
    patterns: &HashMap<&str, &str>,
) -> Result<(), DiffGenerateError> {
    // TODO: Somehow propagate --color to the external command?
    let mut cmd = Command::new(&tool.program);
    cmd.args(interpolate_variables(&tool.diff_args, patterns));
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
    let copy_result = io::copy(&mut child.stdout.take().unwrap(), writer);
    // Non-zero exit code isn't an error. For example, the traditional diff command
    // will exit with 1 if inputs are different.
    let exit_status = child.wait().map_err(ExternalToolError::Io)?;
    tracing::info!(?cmd, ?exit_status, "The external diff generator exited:");
    if !exit_status.success() {
        writeln!(
            ui.warning_default(),
            "Tool exited with {exit_status} (run with --debug to see the exact invocation)",
        )
        .ok();
    }
    copy_result.map_err(ExternalToolError::Io)?;
    Ok(())
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

    #[test]
    fn test_find_all_variables() {
        assert_eq!(
            find_all_variables(
                &[
                    "$left",
                    "$right",
                    "--two=$1 and $2",
                    "--can-be-part-of-string=$output",
                    "$NOT_CAPITALS",
                    "--can-repeat=$right"
                ]
                .map(ToOwned::to_owned),
            )
            .collect_vec(),
            ["left", "right", "1", "2", "output", "right"],
        );
    }
}
