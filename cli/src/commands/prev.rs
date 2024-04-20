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
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};

use crate::cli_util::{short_commit_hash, CommandHelper};
use crate::command_error::{user_error, CommandError};
use crate::commands::next::choose_commit;
use crate::ui::Ui;
/// Change the working copy revision relative to the parent revision
///
/// The command creates a new empty working copy revision that is the child of
/// an ancestor `offset` revisions behind the parent of the current working
/// copy.
///
/// For example, when the offset is 1:
///
/// ```text
/// D @      D
/// |/       |
/// A   =>   A @
/// |        |/
/// B        B
/// ```
///
/// If `--edit` is passed, the working copy revision is changed to the parent of
/// the current working copy revision.
///
/// ```text
/// D @      D
/// |/       |
/// C   =>   @
/// |        |
/// B        B
/// |        |
/// A        A
/// ```
/// If the working copy revision already has visible children, then `--edit` is
/// implied.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct PrevArgs {
    /// How many revisions to move backward. Moves to the parent by default.
    #[arg(default_value = "1")]
    offset: u64,
    /// Edit the parent directly, instead of moving the working-copy commit.
    #[arg(long, short)]
    edit: bool,
}

pub(crate) fn cmd_prev(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PrevArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let current_wc_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let edit = args.edit
        || !workspace_command
            .repo()
            .view()
            .heads()
            .contains(current_wc_id);
    // If we're editing, start at the working-copy commit. Otherwise, start from
    // its direct parent(s).
    let target_revset = if edit {
        RevsetExpression::commit(current_wc_id.clone()).ancestors_at(args.offset)
    } else {
        RevsetExpression::commit(current_wc_id.clone())
            .parents()
            .ancestors_at(args.offset)
    };
    let targets: Vec<_> = target_revset
        .evaluate_programmatic(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    let target = match targets.as_slice() {
        [target] => target,
        [] => {
            return Err(user_error(format!(
                "No ancestor found {} commit{} back",
                args.offset,
                if args.offset > 1 { "s" } else { "" }
            )))
        }
        commits => choose_commit(ui, &workspace_command, "prev", commits)?,
    };

    // Generate a short commit hash, to make it readable in the op log.
    let current_short = short_commit_hash(current_wc_id);
    let target_short = short_commit_hash(target.id());
    // If we're editing, just move to the revision directly.
    if edit {
        // The target must be rewritable if we're editing.
        workspace_command.check_rewritable([target.id()])?;
        let mut tx = workspace_command.start_transaction();
        tx.edit(target)?;
        tx.finish(
            ui,
            format!("prev: {current_short} -> editing {target_short}"),
        )?;
        return Ok(());
    }
    let mut tx = workspace_command.start_transaction();
    tx.check_out(target)?;
    tx.finish(ui, format!("prev: {current_short} -> {target_short}"))?;
    Ok(())
}
