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
use std::io::{self, BufRead, Seek, SeekFrom, Write};
use std::path::Path;

use clap::Subcommand;
use itertools::Itertools;
use jj_lib::file_util;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{
    print_checkout_stats, run_ui_editor, user_error, CommandError, CommandHelper,
};
use crate::ui::Ui;

/// Manage which paths from the working-copy commit are present in the working
/// copy
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum SparseArgs {
    List(SparseListArgs),
    Set(SparseSetArgs),
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
    #[arg(long, value_hint = clap::ValueHint::AnyPath)]
    add: Vec<String>,
    /// Patterns to remove from the working copy
    #[arg(long, conflicts_with = "clear", value_hint = clap::ValueHint::AnyPath)]
    remove: Vec<String>,
    /// Include no files in the working copy (combine with --add)
    #[arg(long)]
    clear: bool,
    /// Edit patterns with $EDITOR
    #[arg(long)]
    edit: bool,
    /// Include all files in the working copy
    #[arg(long, conflicts_with_all = &["add", "remove", "clear"])]
    reset: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_sparse(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SparseArgs,
) -> Result<(), CommandError> {
    match args {
        SparseArgs::List(sub_args) => cmd_sparse_list(ui, command, sub_args),
        SparseArgs::Set(sub_args) => cmd_sparse_set(ui, command, sub_args),
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
        let ui_path = workspace_command.format_file_path(path);
        writeln!(ui.stdout(), "{ui_path}")?;
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
    let paths_to_add: Vec<_> = args
        .add
        .iter()
        .map(|v| workspace_command.parse_file_path(v))
        .try_collect()?;
    let paths_to_remove: Vec<_> = args
        .remove
        .iter()
        .map(|v| workspace_command.parse_file_path(v))
        .try_collect()?;
    // Determine inputs of `edit` operation now, since `workspace_command` is
    // inaccessible while the working copy is locked.
    let edit_inputs = args.edit.then(|| {
        (
            workspace_command.repo().clone(),
            workspace_command.workspace_root().clone(),
        )
    });
    let (mut locked_ws, wc_commit) = workspace_command.start_working_copy_mutation()?;
    let mut new_patterns = HashSet::new();
    if args.reset {
        new_patterns.insert(RepoPathBuf::root());
    } else {
        if !args.clear {
            new_patterns.extend(locked_ws.locked_wc().sparse_patterns()?.iter().cloned());
            for path in paths_to_remove {
                new_patterns.remove(&path);
            }
        }
        for path in paths_to_add {
            new_patterns.insert(path);
        }
    }
    let mut new_patterns = new_patterns.into_iter().collect_vec();
    new_patterns.sort();
    if let Some((repo, workspace_root)) = edit_inputs {
        new_patterns = edit_sparse(
            &workspace_root,
            repo.repo_path(),
            &new_patterns,
            command.settings(),
        )?;
        new_patterns.sort();
    }
    let stats = locked_ws
        .locked_wc()
        .set_sparse_patterns(new_patterns)
        .map_err(|err| {
            CommandError::InternalError(format!("Failed to update working copy paths: {err}"))
        })?;
    let operation_id = locked_ws.locked_wc().old_operation_id().clone();
    locked_ws.finish(operation_id)?;
    print_checkout_stats(ui, stats, &wc_commit)?;

    Ok(())
}

fn edit_sparse(
    workspace_root: &Path,
    repo_path: &Path,
    sparse: &[RepoPathBuf],
    settings: &UserSettings,
) -> Result<Vec<RepoPathBuf>, CommandError> {
    let file = (|| -> Result<_, io::Error> {
        let mut file = tempfile::Builder::new()
            .prefix("editor-")
            .suffix(".jjsparse")
            .tempfile_in(repo_path)?;
        for sparse_path in sparse {
            let workspace_relative_sparse_path =
                file_util::relative_path(workspace_root, &sparse_path.to_fs_path(workspace_root));
            file.write_all(
                workspace_relative_sparse_path
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "stored sparse path is not valid utf-8: {}",
                                workspace_relative_sparse_path.display()
                            ),
                        )
                    })?
                    .as_bytes(),
            )?;
            file.write_all(b"\n")?;
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    })()
    .map_err(|e| {
        user_error(format!(
            r#"Failed to create sparse patterns file in "{path}": {e}"#,
            path = repo_path.display()
        ))
    })?;
    let file_path = file.path().to_owned();

    run_ui_editor(settings, &file_path)?;

    // Read and parse patterns.
    io::BufReader::new(file)
        .lines()
        .filter(|line| {
            line.as_ref()
                .map(|line| !line.starts_with("JJ: ") && !line.trim().is_empty())
                .unwrap_or(true)
        })
        .map(|line| {
            let line = line.map_err(|e| {
                user_error(format!(
                    r#"Failed to read sparse patterns file "{path}": {e}"#,
                    path = file_path.display()
                ))
            })?;
            Ok::<_, CommandError>(RepoPathBuf::parse_fs_path(
                workspace_root,
                workspace_root,
                line.trim(),
            )?)
        })
        .try_collect()
}
