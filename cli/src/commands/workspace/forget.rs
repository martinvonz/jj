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

use itertools::Itertools;
use jj_lib::git::git_worktree_remove;
use jj_lib::git::git_worktree_validate_removal;
use jj_lib::git::WorktreeRemovalValidationError;
use jj_lib::op_store::WorkspaceId;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::user_error_with_hint;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Stop tracking a workspace's working-copy commit in the repo
///
/// The workspace will not be touched on disk. It can be deleted from disk
/// before or after running this command.
#[derive(clap::Args, Clone, Debug)]
pub struct WorkspaceForgetArgs {
    /// Names of the workspaces to forget. By default, forgets only the current
    /// workspace.
    workspaces: Vec<String>,
}

#[instrument(skip_all)]
pub fn cmd_workspace_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let wss: Vec<WorkspaceId> = if args.workspaces.is_empty() {
        vec![workspace_command.workspace_id().clone()]
    } else {
        args.workspaces
            .iter()
            .map(|ws| WorkspaceId::new(ws.to_string()))
            .collect()
    };

    for ws in &wss {
        if workspace_command
            .repo()
            .view()
            .get_wc_commit_id(ws)
            .is_none()
        {
            return Err(user_error(format!("No such workspace: {}", ws.as_str())));
        }
    }

    let git_backend = workspace_command
        .git_backend()
        .map(|backend| backend.git_repo());

    // bundle every workspace forget into a single transaction, so that e.g.
    // undo correctly restores all of them at once.
    let mut tx = workspace_command.start_transaction();

    let mut worktrees_to_remove = vec![];
    wss.iter().try_for_each(|ws| {
        if let Some(git_repo) = git_backend.as_ref() {
            match git_worktree_validate_removal(git_repo, ws.as_str()) {
                Ok(stat) => worktrees_to_remove.push(stat),
                Err(error @ WorktreeRemovalValidationError::NonexistentWorktree(_)) => {
                    // Indistinguishable from a workspace that never had a worktree.
                    tracing::debug!(%error, "Ignoring non-existent worktree");
                }
                Err(e) => {
                    let err = format!("Could not remove Git worktree for workspace {ws}: {e}");
                    return if let Some(hint) = e.hint(git_repo) {
                        Err(user_error_with_hint(err, hint))
                    } else {
                        Err(user_error(err))
                    };
                }
            }
        }
        tx.repo_mut()
            .remove_wc_commit(ws)
            .map_err(CommandError::from)
    })?;

    let description = if let [ws] = wss.as_slice() {
        format!("forget workspace {}", ws.as_str())
    } else {
        format!(
            "forget workspaces {}",
            wss.iter().map(|ws| ws.as_str()).join(", ")
        )
    };

    tx.finish(ui, description)?;

    for validated in worktrees_to_remove {
        git_worktree_remove(validated).map_err(user_error)?;
    }

    Ok(())
}
