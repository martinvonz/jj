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

use jj_lib::op_store::WorkspaceId;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Renames the current workspace
#[derive(clap::Args, Clone, Debug)]
pub struct WorkspaceRenameArgs {
    /// The name of the workspace to update to.
    new_workspace_name: String,
}

#[instrument(skip_all)]
pub fn cmd_workspace_rename(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceRenameArgs,
) -> Result<(), CommandError> {
    if args.new_workspace_name.is_empty() {
        return Err(user_error("New workspace name cannot be empty"));
    }

    let mut workspace_command = command.workspace_helper(ui)?;

    let old_workspace_id = workspace_command.working_copy().workspace_id().clone();
    let new_workspace_id = WorkspaceId::new(args.new_workspace_name.clone());
    if new_workspace_id == old_workspace_id {
        writeln!(ui.status(), "Nothing changed.")?;
        return Ok(());
    }

    if workspace_command
        .repo()
        .view()
        .get_wc_commit_id(&old_workspace_id)
        .is_none()
    {
        return Err(user_error(format!(
            "The current workspace '{}' is not tracked in the repo.",
            old_workspace_id.as_str()
        )));
    }

    let mut tx = workspace_command.start_transaction().into_inner();
    let (mut locked_ws, _wc_commit) = workspace_command.start_working_copy_mutation()?;

    locked_ws
        .locked_wc()
        .rename_workspace(new_workspace_id.clone());

    tx.repo_mut()
        .rename_workspace(&old_workspace_id, new_workspace_id)?;
    let repo = tx.commit(format!(
        "Renamed workspace '{}' to '{}'",
        old_workspace_id.as_str(),
        args.new_workspace_name
    ));
    locked_ws.finish(repo.op_id().clone())?;

    Ok(())
}
