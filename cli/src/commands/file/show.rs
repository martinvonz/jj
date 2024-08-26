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

use std::io;
use std::io::Write;

use jj_lib::backend::BackendResult;
use jj_lib::conflicts::materialize_tree_value;
use jj_lib::conflicts::MaterializedTreeValue;
use jj_lib::fileset::FilePattern;
use jj_lib::fileset::FilesetExpression;
use jj_lib::merge::MergedTreeValue;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use pollster::FutureExt;
use tracing::instrument;

use crate::cli_util::print_unmatched_explicit_paths;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Print contents of files in a revision
///
/// If the given path is a directory, files in the directory will be visited
/// recursively.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct FileShowArgs {
    /// The revision to get the file contents from
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Paths to print
    #[arg(required = true, value_hint = clap::ValueHint::FilePath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_file_show(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &FileShowArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let tree = commit.tree()?;
    // TODO: No need to add special case for empty paths when switching to
    // parse_union_filesets(). paths = [] should be "none()" if supported.
    let fileset_expression = workspace_command.parse_file_patterns(&args.paths)?;

    // Try fast path for single file entry
    if let Some(path) = get_single_path(&fileset_expression) {
        let value = tree.path_value(path)?;
        if value.is_absent() {
            let ui_path = workspace_command.format_file_path(path);
            return Err(user_error(format!("No such path: {ui_path}")));
        }
        if !value.is_tree() {
            ui.request_pager();
            write_tree_entries(ui, &workspace_command, [(path, Ok(value))])?;
            return Ok(());
        }
    }

    let matcher = fileset_expression.to_matcher();
    ui.request_pager();
    write_tree_entries(
        ui,
        &workspace_command,
        tree.entries_matching(matcher.as_ref()),
    )?;
    print_unmatched_explicit_paths(ui, &workspace_command, &fileset_expression, [&tree])?;
    Ok(())
}

fn get_single_path(expression: &FilesetExpression) -> Option<&RepoPath> {
    match &expression {
        FilesetExpression::Pattern(pattern) => match pattern {
            // Not using pattern.as_path() because files-in:<path> shouldn't
            // select the literal <path> itself.
            FilePattern::FilePath(path) | FilePattern::PrefixPath(path) => Some(path),
            FilePattern::FileGlob { .. } => None,
        },
        _ => None,
    }
}

fn write_tree_entries<P: AsRef<RepoPath>>(
    ui: &Ui,
    workspace_command: &WorkspaceCommandHelper,
    entries: impl IntoIterator<Item = (P, BackendResult<MergedTreeValue>)>,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();
    for (path, result) in entries {
        let value = result?;
        let materialized = materialize_tree_value(repo.store(), path.as_ref(), value).block_on()?;
        match materialized {
            MaterializedTreeValue::Absent => panic!("absent values should be excluded"),
            MaterializedTreeValue::AccessDenied(err) => {
                let ui_path = workspace_command.format_file_path(path.as_ref());
                writeln!(
                    ui.warning_default(),
                    "Path '{ui_path}' exists but access is denied: {err}"
                )?;
            }
            MaterializedTreeValue::File { mut reader, .. } => {
                io::copy(&mut reader, &mut ui.stdout_formatter().as_mut())?;
            }
            MaterializedTreeValue::FileConflict { contents, .. } => {
                ui.stdout_formatter().write_all(&contents)?;
            }
            MaterializedTreeValue::OtherConflict { id } => {
                ui.stdout_formatter().write_all(id.describe().as_bytes())?;
            }
            MaterializedTreeValue::Symlink { .. } | MaterializedTreeValue::GitSubmodule(_) => {
                let ui_path = workspace_command.format_file_path(path.as_ref());
                writeln!(
                    ui.warning_default(),
                    "Path '{ui_path}' exists but is not a file"
                )?;
            }
            MaterializedTreeValue::Tree(_) => panic!("entries should not contain trees"),
        }
    }
    Ok(())
}
