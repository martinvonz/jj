// Copyright 2020-2023 The Jujutsu Authors
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

use itertools::Itertools as _;
use jj_lib::git;

use super::find_remote_branches;
use crate::cli_util::{CommandHelper, RemoteBranchNamePattern};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Stop tracking given remote branches
///
/// A non-tracking remote branch is just a pointer to the last-fetched remote
/// branch. It won't be imported as a local branch on future pulls.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchUntrackArgs {
    /// Remote branches to untrack
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    ///
    /// Examples: branch@remote, glob:main@*, glob:jjfan-*@upstream
    #[arg(required = true, value_name = "BRANCH@REMOTE")]
    names: Vec<RemoteBranchNamePattern>,
}

pub fn cmd_branch_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchUntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    let mut names = Vec::new();
    for (name, remote_ref) in find_remote_branches(view, &args.names)? {
        if name.remote == git::REMOTE_NAME_FOR_LOCAL_GIT_REPO {
            // This restriction can be lifted if we want to support untracked @git branches.
            writeln!(
                ui.warning_default(),
                "Git-tracking branch cannot be untracked: {name}"
            )?;
        } else if !remote_ref.is_tracking() {
            writeln!(
                ui.warning_default(),
                "Remote branch not tracked yet: {name}"
            )?;
        } else {
            names.push(name);
        }
    }
    let mut tx = workspace_command.start_transaction();
    for name in &names {
        tx.mut_repo()
            .untrack_remote_branch(&name.branch, &name.remote);
    }
    tx.finish(
        ui,
        format!("untrack remote branch {}", names.iter().join(", ")),
    )?;
    if names.len() > 1 {
        writeln!(
            ui.status(),
            "Stopped tracking {} remote branches.",
            names.len()
        )?;
    }
    Ok(())
}
