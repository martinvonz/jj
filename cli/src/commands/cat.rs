// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;

use jj_lib::conflicts::{materialize_tree_value, MaterializedTreeValue};
use jj_lib::repo::Repo;
use pollster::FutureExt;
use tracing::instrument;

use crate::cli_util::{user_error, CommandError, CommandHelper, RevisionArg};
use crate::ui::Ui;

/// Print contents of a file in a revision
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct CatArgs {
    /// The revision to get the file contents from
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The file to print
    #[arg(value_hint = clap::ValueHint::FilePath)]
    path: String,
}

#[instrument(skip_all)]
pub(crate) fn cmd_cat(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CatArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let tree = commit.tree()?;
    let path = workspace_command.parse_file_path(&args.path)?;
    let repo = workspace_command.repo();
    let value = tree.path_value(&path);
    let materialized = materialize_tree_value(repo.store(), &path, value).block_on()?;
    match materialized {
        MaterializedTreeValue::Absent => {
            return Err(user_error("No such path"));
        }
        MaterializedTreeValue::File { mut reader, .. } => {
            ui.request_pager();
            std::io::copy(&mut reader, &mut ui.stdout_formatter().as_mut())?;
        }
        MaterializedTreeValue::Conflict { contents, .. } => {
            ui.request_pager();
            ui.stdout_formatter().write_all(&contents)?;
        }
        MaterializedTreeValue::Symlink { .. }
        | MaterializedTreeValue::Tree(_)
        | MaterializedTreeValue::GitSubmodule(_) => {
            return Err(user_error("Path exists but is not a file"));
        }
    }
    Ok(())
}
