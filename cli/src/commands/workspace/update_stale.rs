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

use jj_lib::working_copy::WorkingCopyFreshness;
use tracing::instrument;

use crate::cli_util::update_stale_working_copy;
use crate::cli_util::CommandHelper;
use crate::cli_util::StaleWorkingCopy;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Update a workspace that has become stale
///
/// For information about stale working copies, see
/// https://martinvonz.github.io/jj/latest/working-copy/.
#[derive(clap::Args, Clone, Debug)]
pub struct WorkspaceUpdateStaleArgs {}

#[instrument(skip_all)]
pub fn cmd_workspace_update_stale(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceUpdateStaleArgs,
) -> Result<(), CommandError> {
    // Snapshot the current working copy on top of the last known working-copy
    // operation, then merge the divergent operations. The wc_commit_id of the
    // merged repo wouldn't change because the old one wins, but it's probably
    // fine if we picked the new wc_commit_id.
    let known_wc_commit = match command.load_stale_working_copy_commit(ui)? {
        StaleWorkingCopy::Recovered(_) => {
            // We have already recovered from the situation that prompted the user to run
            // this command, and it is known that the workspace is not stale
            // (since we just updated it), so we can return early.
            return Ok(());
        }
        StaleWorkingCopy::Snapshotted((_repo, commit)) => commit,
    };
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;

    let repo = workspace_command.repo().clone();
    let (mut locked_ws, desired_wc_commit) =
        workspace_command.unchecked_start_working_copy_mutation()?;
    match WorkingCopyFreshness::check_stale(locked_ws.locked_wc(), &desired_wc_commit, &repo)? {
        WorkingCopyFreshness::Fresh | WorkingCopyFreshness::Updated(_) => {
            writeln!(
                ui.status(),
                "Nothing to do (the working copy is not stale)."
            )?;
        }
        WorkingCopyFreshness::WorkingCopyStale | WorkingCopyFreshness::SiblingOperation => {
            let stats = update_stale_working_copy(
                locked_ws,
                repo.op_id().clone(),
                &known_wc_commit,
                &desired_wc_commit,
            )?;

            workspace_command.write_stale_commit_stats(ui, &desired_wc_commit, stats)?;
        }
    }
    Ok(())
}
