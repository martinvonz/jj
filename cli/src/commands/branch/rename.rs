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

use jj_lib::op_store::RefTarget;

use super::has_tracked_remote_branches;
use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Rename `old` branch name to `new` branch name
///
/// The new branch name points at the same commit as the old branch name.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchRenameArgs {
    /// The old name of the branch
    old: String,

    /// The new name of the branch
    new: String,
}

pub fn cmd_branch_rename(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchRenameArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    let old_branch = &args.old;
    let ref_target = view.get_local_branch(old_branch).clone();
    if ref_target.is_absent() {
        return Err(user_error(format!("No such branch: {old_branch}")));
    }

    let new_branch = &args.new;
    if view.get_local_branch(new_branch).is_present() {
        return Err(user_error(format!("Branch already exists: {new_branch}")));
    }

    let mut tx = workspace_command.start_transaction();
    tx.mut_repo()
        .set_local_branch_target(new_branch, ref_target);
    tx.mut_repo()
        .set_local_branch_target(old_branch, RefTarget::absent());
    tx.finish(ui, format!("rename branch {old_branch} to {new_branch}"))?;

    let view = workspace_command.repo().view();
    if has_tracked_remote_branches(view, old_branch) {
        writeln!(
            ui.warning_default(),
            "Tracked remote branches for branch {old_branch} were not renamed.",
        )?;
        writeln!(
            ui.hint_default(),
            "To rename the branch on the remote, you can `jj git push --branch {old_branch}` \
             first (to delete it on the remote), and then `jj git push --branch {new_branch}`. \
             `jj git push --all` would also be sufficient."
        )?;
    }
    if has_tracked_remote_branches(view, new_branch) {
        // This isn't an error because branch renaming can't be propagated to
        // the remote immediately. "rename old new && rename new old" should be
        // allowed even if the original old branch had tracked remotes.
        writeln!(
            ui.warning_default(),
            "Tracked remote branches for branch {new_branch} exist."
        )?;
        writeln!(
            ui.hint_default(),
            "Run `jj branch untrack 'glob:{new_branch}@*'` to disassociate them."
        )?;
    }

    Ok(())
}
