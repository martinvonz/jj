// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use itertools::Itertools;
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};

use crate::cli_util::{short_commit_hash, user_error, CommandError, CommandHelper};
use crate::commands::next::choose_commit;
use crate::ui::Ui;

/// Move the working copy commit to the parent of the current revision.
///
///
/// The command moves you to the parent in a linear fashion.
///
/// ```text
/// D @  D
/// |/   |
/// A => A @
/// |    |/
/// B    B
/// ```
///
/// If `--edit` is passed, it will move the working copy commit
/// directly to the parent.
///
/// ```text
/// D @  D
/// |/   |
/// C => @
/// |    |
/// B    B
/// |    |
/// A    A
///
/// If your working-copy commit already has visible children, then `--edit` is
/// implied.
/// ```
// TODO(#2126): Handle multiple parents, e.g merges.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct PrevArgs {
    /// How many revisions to move backward. By default moves to the parent.
    #[arg(default_value = "1")]
    amount: u64,
    /// Edit the parent directly, instead of moving the working-copy commit.
    #[arg(long)]
    edit: bool,
}

pub(crate) fn cmd_prev(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PrevArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let amount = args.amount;
    let current_wc_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let edit = args.edit
        || !workspace_command
            .repo()
            .view()
            .heads()
            .contains(current_wc_id);
    let current_wc = workspace_command.repo().store().get_commit(current_wc_id)?;
    let start_id = if edit {
        current_wc_id
    } else {
        match current_wc.parent_ids() {
            [parent_id] => parent_id,
            _ => return Err(user_error("Cannot run `jj prev` on a merge commit")),
        }
    };
    let ancestor_expression = RevsetExpression::commit(start_id.clone()).ancestors_at(amount);
    let target_revset = if edit {
        ancestor_expression
    } else {
        // Jujutsu will always create a new commit for prev, even where Mercurial cannot
        // and fails. The decision and all discussion around it are available
        // here: https://github.com/martinvonz/jj/pull/1200#discussion_r1298623933
        //
        // If users ever request erroring out, add `.ancestors()` to the revset below.
        ancestor_expression.minus(&RevsetExpression::commit(current_wc_id.clone()))
    };
    let targets: Vec<_> = target_revset
        .evaluate_programmatic(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .take(2)
        .try_collect()?;
    let target = match targets.as_slice() {
        [target] => target,
        [] => {
            return Err(user_error(format!(
                "No ancestor found {amount} commit{} back",
                if amount > 1 { "s" } else { "" }
            )))
        }
        commits => choose_commit(ui, &workspace_command, "prev", commits)?,
    };
    // Generate a short commit hash, to make it readable in the op log.
    let current_short = short_commit_hash(current_wc.id());
    let target_short = short_commit_hash(target.id());
    // If we're editing, just move to the revision directly.
    if edit {
        // The target must be rewritable if we're editing.
        workspace_command.check_rewritable([target])?;
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
