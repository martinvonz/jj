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

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use clap::Subcommand;
use itertools::Itertools;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{
    edit_temp_file, print_checkout_stats, CommandHelper, WorkspaceCommandHelper,
};
use crate::command_error::{
    internal_error, internal_error_with_message, user_error_with_message, CommandError,
};
use crate::ui::Ui;

/// Manage which paths from the working-copy commit are present in the working
/// copy
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum SparseArgs {
    List(SparseListArgs),
    Set(SparseSetArgs),
    Reset(SparseResetArgs),
    Edit(SparseEditArgs),
}

/// List the patterns that are currently present in the working copy
///
/// By default, a newly cloned or initialized repo will have have a pattern
/// matching all files from the repo root. That pattern is rendered as `.` (a
/// single period).
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SparseListArgs {}

/// Update the patterns that are present in the working copy
///
/// For example, if all you need is the `README.md` and the `lib/`
/// directory, use `jj sparse set --clear --add README.md --add lib`.
/// If you no longer need the `lib` directory, use `jj sparse set --remove lib`.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SparseSetArgs {
    /// Patterns to add to the working copy
    #[arg(
        long,
        value_hint = clap::ValueHint::AnyPath,
        value_parser = |s: &str| RepoPathBuf::from_relative_path(s),
    )]
    add: Vec<RepoPathBuf>,
    /// Patterns to remove from the working copy
    #[arg(
        long,
        conflicts_with = "clear",
        value_hint = clap::ValueHint::AnyPath,
        value_parser = |s: &str| RepoPathBuf::from_relative_path(s),
    )]
    remove: Vec<RepoPathBuf>,
    /// Include no files in the working copy (combine with --add)
    #[arg(long)]
    clear: bool,
}

/// Reset the patterns to include all files in the working copy
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SparseResetArgs {}

/// Start an editor to update the patterns that are present in the working copy
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SparseEditArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_sparse(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SparseArgs,
) -> Result<(), CommandError> {
    match args {
        SparseArgs::List(sub_args) => cmd_sparse_list(ui, command, sub_args),
        SparseArgs::Set(sub_args) => cmd_sparse_set(ui, command, sub_args),
        SparseArgs::Reset(sub_args) => cmd_sparse_reset(ui, command, sub_args),
        SparseArgs::Edit(sub_args) => cmd_sparse_edit(ui, command, sub_args),
    }
}

#[instrument(skip_all)]
fn cmd_sparse_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &SparseListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    for path in workspace_command.working_copy().sparse_patterns()? {
        writeln!(ui.stdout(), "{}", path.to_fs_path(Path::new("")).display())?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_sparse_set(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SparseSetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    update_sparse_patterns_with(ui, &mut workspace_command, |_ui, old_patterns| {
        let mut new_patterns = HashSet::new();
        if !args.clear {
            new_patterns.extend(old_patterns.iter().cloned());
            for path in &args.remove {
                new_patterns.remove(path);
            }
        }
        for path in &args.add {
            new_patterns.insert(path.to_owned());
        }
        Ok(new_patterns.into_iter().sorted_unstable().collect())
    })
}

#[instrument(skip_all)]
fn cmd_sparse_reset(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &SparseResetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    update_sparse_patterns_with(ui, &mut workspace_command, |_ui, _old_patterns| {
        Ok(vec![RepoPathBuf::root()])
    })
}

#[instrument(skip_all)]
fn cmd_sparse_edit(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &SparseEditArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo_path = workspace_command.repo().repo_path().to_owned();
    update_sparse_patterns_with(ui, &mut workspace_command, |_ui, old_patterns| {
        let mut new_patterns = edit_sparse(&repo_path, old_patterns, command.settings())?;
        new_patterns.sort_unstable();
        new_patterns.dedup();
        Ok(new_patterns)
    })
}

fn edit_sparse(
    repo_path: &Path,
    sparse: &[RepoPathBuf],
    settings: &UserSettings,
) -> Result<Vec<RepoPathBuf>, CommandError> {
    let mut content = String::new();
    for sparse_path in sparse {
        let workspace_relative_sparse_path = sparse_path.to_fs_path(Path::new(""));
        let path_string = workspace_relative_sparse_path.to_str().ok_or_else(|| {
            internal_error(format!(
                "Stored sparse path is not valid utf-8: {}",
                workspace_relative_sparse_path.display()
            ))
        })?;
        writeln!(&mut content, "{}", path_string).unwrap();
    }

    let content = edit_temp_file(
        "sparse patterns",
        ".jjsparse",
        repo_path,
        &content,
        settings,
    )?;

    content
        .lines()
        .filter(|line| !line.starts_with("JJ: "))
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            RepoPathBuf::from_relative_path(line).map_err(|err| {
                user_error_with_message(format!("Failed to parse sparse pattern: {line}"), err)
            })
        })
        .try_collect()
}

fn update_sparse_patterns_with(
    ui: &mut Ui,
    workspace_command: &mut WorkspaceCommandHelper,
    f: impl FnOnce(&mut Ui, &[RepoPathBuf]) -> Result<Vec<RepoPathBuf>, CommandError>,
) -> Result<(), CommandError> {
    let (mut locked_ws, wc_commit) = workspace_command.start_working_copy_mutation()?;
    let new_patterns = f(ui, locked_ws.locked_wc().sparse_patterns()?)?;
    let stats = locked_ws
        .locked_wc()
        .set_sparse_patterns(new_patterns)
        .map_err(|err| internal_error_with_message("Failed to update working copy paths", err))?;
    let operation_id = locked_ws.locked_wc().old_operation_id().clone();
    locked_ws.finish(operation_id)?;
    print_checkout_stats(ui, stats, &wc_commit)?;
    Ok(())
}
