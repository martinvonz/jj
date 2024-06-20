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
use jj_lib::str_util::StringPattern;

use super::make_branch_term;
use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Rename `old` branch name to `new` branch name.
///
/// The new branch name points at the same commit as the old
/// branch name.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchRenameArgs {
    /// The old name of the branch.
    old: String,

    /// The new name of the branch.
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
    tx.finish(
        ui,
        format!(
            "rename {} to {}",
            make_branch_term(&[old_branch]),
            make_branch_term(&[new_branch]),
        ),
    )?;

    let view = workspace_command.repo().view();
    if view
        .remote_branches_matching(
            &StringPattern::exact(old_branch),
            &StringPattern::everything(),
        )
        .any(|(_, remote_ref)| remote_ref.is_tracking())
    {
        writeln!(
            ui.warning_default(),
            "Branch {old_branch} has tracking remote branches which were not renamed."
        )?;
        writeln!(
            ui.hint_default(),
            "to rename the branch on the remote, you can `jj git push --branch {old_branch}` \
             first (to delete it on the remote), and then `jj git push --branch {new_branch}`. \
             `jj git push --all` would also be sufficient."
        )?;
    }

    Ok(())
}
