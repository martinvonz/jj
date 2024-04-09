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

use std::io::{self, Write};

use jj_lib::conflicts::{materialize_tree_value, MaterializedTreeValue};
use jj_lib::merge::MergedTreeValue;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use pollster::FutureExt;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg, WorkspaceCommandHelper};
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
    // TODO: migrate to .parse_file_patterns()?.to_matcher()?
    let path = workspace_command.parse_file_path(&args.path)?;
    let value = tree.path_value(&path);
    if value.is_absent() {
        let ui_path = workspace_command.format_file_path(&path);
        return Err(user_error(format!("No such path: {ui_path}")));
    }
    ui.request_pager();
    write_tree_entries(ui, &workspace_command, [(&path, value)])?;
    Ok(())
}

fn write_tree_entries<P: AsRef<RepoPath>>(
    ui: &Ui,
    workspace_command: &WorkspaceCommandHelper,
    entries: impl IntoIterator<Item = (P, MergedTreeValue)>,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();
    for (path, value) in entries {
        let materialized = materialize_tree_value(repo.store(), path.as_ref(), value).block_on()?;
        match materialized {
            MaterializedTreeValue::Absent => panic!("absent values should be excluded"),
            MaterializedTreeValue::File { mut reader, .. } => {
                io::copy(&mut reader, &mut ui.stdout_formatter().as_mut())?;
            }
            MaterializedTreeValue::Conflict { contents, .. } => {
                ui.stdout_formatter().write_all(&contents)?;
            }
            MaterializedTreeValue::Symlink { .. }
            | MaterializedTreeValue::Tree(_)
            | MaterializedTreeValue::GitSubmodule(_) => {
                let ui_path = workspace_command.format_file_path(path.as_ref());
                writeln!(
                    ui.warning_default(),
                    "Path exists but is not a file: {ui_path}"
                )?;
            }
        }
    }
    Ok(())
}
