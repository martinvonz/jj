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

use std::sync::Arc;

use jj_lib::object_id::ObjectId;
use jj_lib::op_store::OpStoreError;
use jj_lib::operation::Operation;
use jj_lib::repo::{ReadonlyRepo, Repo};
use tracing::instrument;

use crate::cli_util::{
    check_stale_working_copy, print_checkout_stats, short_commit_hash, CommandHelper,
    WorkingCopyFreshness, WorkspaceCommandHelper,
};
use crate::command_error::{internal_error_with_message, user_error, CommandError};
use crate::ui::Ui;

/// Update a workspace that has become stale
///
/// For information about stale working copies, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
pub struct WorkspaceUpdateStaleArgs {}

#[instrument(skip_all)]
pub fn cmd_workspace_update_stale(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceUpdateStaleArgs,
) -> Result<(), CommandError> {
    // Snapshot the current working copy on top of the last known working-copy
    // operation, then merge the concurrent operations. The wc_commit_id of the
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
    match check_stale_working_copy(locked_ws.locked_wc(), &desired_wc_commit, &repo)? {
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

fn create_and_check_out_recovery_commit(
    ui: &mut Ui,
    command: &CommandHelper,
) -> Result<Arc<ReadonlyRepo>, CommandError> {
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;
    let workspace_id = workspace_command.workspace_id().clone();
    let mut tx = workspace_command.start_transaction().into_inner();

    let (mut locked_workspace, commit) =
        workspace_command.unchecked_start_working_copy_mutation()?;
    let commit_id = commit.id();

    let mut_repo = tx.mut_repo();
    let new_commit = mut_repo
        .new_commit(
            command.settings(),
            vec![commit_id.clone()],
            commit.tree_id().clone(),
        )
        .write()?;
    mut_repo.set_wc_commit(workspace_id, new_commit.id().clone())?;
    let repo = tx.commit("recovery commit");

    locked_workspace.locked_wc().recover(&new_commit)?;
    locked_workspace.finish(repo.op_id().clone())?;

    writeln!(
        ui.status(),
        "Created and checked out recovery commit {}",
        short_commit_hash(new_commit.id())
    )?;

    Ok(repo)
}

/// Loads workspace that will diverge from the last working-copy operation.
fn for_stale_working_copy(
    ui: &mut Ui,
    command: &CommandHelper,
) -> Result<(WorkspaceCommandHelper, bool), CommandError> {
    let workspace = command.load_workspace()?;
    let op_store = workspace.repo_loader().op_store();
    let (repo, recovered) = {
        let op_id = workspace.working_copy().operation_id();
        match op_store.read_operation(op_id) {
            Ok(op_data) => (
                workspace.repo_loader().load_at(&Operation::new(
                    op_store.clone(),
                    op_id.clone(),
                    op_data,
                ))?,
                false,
            ),
            Err(e @ OpStoreError::ObjectNotFound { .. }) => {
                writeln!(
                    ui.status(),
                    "Failed to read working copy's current operation; attempting recovery. Error \
                     message from read attempt: {e}"
                )?;
                (create_and_check_out_recovery_commit(ui, command)?, true)
            }
            Err(e) => return Err(e.into()),
        }
    };
    Ok((command.for_workable_repo(ui, workspace, repo)?, recovered))
}
