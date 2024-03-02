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

use std::io::Write;

use jj_lib::conflicts::{materialize_tree_value, MaterializedTreeValue};
use jj_lib::repo::Repo;
use pollster::FutureExt;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
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
