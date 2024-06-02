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
use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};

use crate::cli_util::{choose_commit, short_commit_hash, CommandHelper};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Move the working-copy commit to the child revision
///
/// The command creates a new empty working copy revision that is the child of a
/// descendant `offset` revisions ahead of the parent of the current working
/// copy.
///
/// For example, when the offset is 1:
///
/// ```text
/// D        D @
/// |        |/
/// C @  =>  C
/// |/       |
/// B        B
/// ```
///
/// If `--edit` is passed, the working copy revision is changed to the child of
/// the current working copy revision.
///
/// ```text
/// D        D
/// |        |
/// C        C
/// |        |
/// B   =>   @
/// |        |
/// @        A
/// ```
/// If your working-copy commit already has visible children, then `--edit` is
/// implied.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct NextArgs {
    /// How many revisions to move forward. Advances to the next child by
    /// default.
    #[arg(default_value = "1")]
    offset: u64,
    /// Instead of creating a new working-copy commit on top of the target
    /// commit (like `jj new`), edit the target commit directly (like `jj
    /// edit`).
    #[arg(long, short)]
    edit: bool,
}

pub(crate) fn cmd_next(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &NextArgs,
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
    let wc_revset = RevsetExpression::commit(current_wc_id.clone());
    // If we're editing, start at the working-copy commit. Otherwise, start from
    // its direct parent(s).
    let target_revset = if edit {
        wc_revset.descendants_at(args.offset)
    } else {
        wc_revset
            .parents()
            .descendants_at(args.offset)
            // In previous versions we subtracted `wc_revset.descendants()`. That's
            // unnecessary now that --edit is implied if `@` has descendants.
            .minus(&wc_revset)
    };
    let targets: Vec<Commit> = target_revset
        .evaluate_programmatic(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    let target = match targets.as_slice() {
        [target] => target,
        [] => {
            // We found no descendant.
            return Err(user_error(format!(
                "No descendant found {} commit{} forward",
                args.offset,
                if args.offset > 1 { "s" } else { "" }
            )));
        }
        commits => choose_commit(
            ui,
            &workspace_command,
            "ambiguous next commit, choose one to target",
            commits,
        )?,
    };
    let current_short = short_commit_hash(current_wc_id);
    let target_short = short_commit_hash(target.id());
    // We're editing, just move to the target commit.
    if edit {
        // We're editing, the target must be rewritable.
        workspace_command.check_rewritable([target.id()])?;
        let mut tx = workspace_command.start_transaction();
        tx.edit(target)?;
        tx.finish(
            ui,
            format!("next: {current_short} -> editing {target_short}"),
        )?;
        return Ok(());
    }
    let mut tx = workspace_command.start_transaction();
    // Move the working-copy commit to the new parent.
    tx.check_out(target)?;
    tx.finish(ui, format!("next: {current_short} -> {target_short}"))?;
    Ok(())
}
