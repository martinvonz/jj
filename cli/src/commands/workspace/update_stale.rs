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

use jj_lib::object_id::ObjectId;
use jj_lib::op_store::OpStoreError;
use jj_lib::repo::Repo;
use jj_lib::working_copy::WorkingCopyFreshness;
use tracing::instrument;

use crate::cli_util::print_checkout_stats;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::internal_error_with_message;
use crate::command_error::user_error;
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
    let known_wc_commit = {
        let (mut workspace_command, recovered) = for_stale_working_copy(ui, command)?;
        workspace_command.maybe_snapshot(ui)?;

        if recovered {
            // We have already recovered from the situation that prompted the user to run
            // this command, and it is known that the workspace is not stale
            // (since we just updated it), so we can return early.
            return Ok(());
        }

        let wc_commit_id = workspace_command.get_wc_commit_id().unwrap();
        workspace_command.repo().store().get_commit(wc_commit_id)?
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
            // The same check as start_working_copy_mutation(), but with the stale
            // working-copy commit.
            if known_wc_commit.tree_id() != locked_ws.locked_wc().old_tree_id() {
                return Err(user_error("Concurrent working copy operation. Try again."));
            }
            let stats = locked_ws
                .locked_wc()
                .check_out(&desired_wc_commit)
                .map_err(|err| {
                    internal_error_with_message(
                        format!(
                            "Failed to check out commit {}",
                            desired_wc_commit.id().hex()
                        ),
                        err,
                    )
                })?;
            locked_ws.finish(repo.op_id().clone())?;
            if let Some(mut formatter) = ui.status_formatter() {
                write!(formatter, "Working copy now at: ")?;
                formatter.with_label("working_copy", |fmt| {
                    workspace_command.write_commit_summary(fmt, &desired_wc_commit)
                })?;
                writeln!(formatter)?;
            }
            print_checkout_stats(ui, stats, &desired_wc_commit)?;
        }
    }
    Ok(())
}

/// Loads workspace that will diverge from the last working-copy operation.
fn for_stale_working_copy(
    ui: &mut Ui,
    command: &CommandHelper,
) -> Result<(WorkspaceCommandHelper, bool), CommandError> {
    let workspace = command.load_workspace()?;
    let (repo, recovered) = {
        let op_id = workspace.working_copy().operation_id();
        match workspace.repo_loader().load_operation(op_id) {
            Ok(op) => (workspace.repo_loader().load_at(&op)?, false),
            Err(e @ OpStoreError::ObjectNotFound { .. }) => {
                writeln!(
                    ui.status(),
                    "Failed to read working copy's current operation; attempting recovery. Error \
                     message from read attempt: {e}"
                )?;

                let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;
                workspace_command.create_and_check_out_recovery_commit(ui)?;
                (workspace_command.repo().clone(), true)
            }
            Err(e) => return Err(e.into()),
        }
    };
    Ok((command.for_workable_repo(ui, workspace, repo)?, recovered))
}
