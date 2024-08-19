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

use std::io::Write;
use std::rc::Rc;

use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetFilterPredicate, RevsetIteratorExt};

use crate::cli_util::{short_commit_hash, CommandHelper, WorkspaceCommandHelper};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MovementArgs {
    pub offset: u64,
    pub edit: bool,
    pub no_edit: bool,
    pub conflict: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MovementArgsInternal {
    offset: u64,
    should_edit: bool,
    conflict: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    Next,
    Prev,
}

impl Direction {
    fn cmd(&self) -> &'static str {
        match self {
            Direction::Next => "next",
            Direction::Prev => "prev",
        }
    }

    fn target_not_found_message(&self, change_offset: u64) -> String {
        match self {
            Direction::Next => format!("No descendant found {} commit(s) forward", change_offset),
            Direction::Prev => format!("No ancestor found {} commit(s) back", change_offset),
        }
    }

    fn get_target_revset(
        &self,
        working_commit_id: &CommitId,
        args: &MovementArgsInternal,
    ) -> Result<Rc<RevsetExpression>, CommandError> {
        let wc_revset = RevsetExpression::commit(working_commit_id.clone());
        // If we're editing, start at the working-copy commit. Otherwise, start from
        // its direct parent(s).
        let start_revset = if args.should_edit {
            wc_revset.clone()
        } else {
            wc_revset.parents()
        };

        let target_revset = match self {
            Direction::Next => if args.conflict {
                start_revset
                    .children()
                    .descendants()
                    .filtered(RevsetFilterPredicate::HasConflict)
                    .roots()
            } else {
                start_revset.descendants_at(args.offset)
            }
            .minus(&wc_revset),

            Direction::Prev => {
                if args.conflict {
                    // If people desire to move to the root conflict, replace the `heads()` below
                    // with `roots(). But let's wait for feedback.
                    start_revset
                        .parents()
                        .ancestors()
                        .filtered(RevsetFilterPredicate::HasConflict)
                        .heads()
                } else {
                    start_revset.ancestors_at(args.offset)
                }
            }
        };

        Ok(target_revset)
    }
}

fn get_target_commit(
    ui: &mut Ui,
    workspace_command: &WorkspaceCommandHelper,
    direction: Direction,
    working_commit_id: &CommitId,
    args: &MovementArgsInternal,
) -> Result<Commit, CommandError> {
    let target_revset = direction.get_target_revset(working_commit_id, args)?;
    let targets: Vec<Commit> = target_revset
        .evaluate_programmatic(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;

    let target = match targets.as_slice() {
        [target] => target,
        [] => {
            // We found no ancestor/descendant.
            return Err(user_error(direction.target_not_found_message(args.offset)));
        }
        commits => choose_commit(ui, workspace_command, direction, commits)?,
    };

    Ok(target.clone())
}

fn choose_commit<'a>(
    ui: &mut Ui,
    workspace_command: &WorkspaceCommandHelper,
    direction: Direction,
    commits: &'a [Commit],
) -> Result<&'a Commit, CommandError> {
    writeln!(
        ui.stdout(),
        "ambiguous {} commit, choose one to target:",
        direction.cmd()
    )?;
    let mut formatter = ui.stdout_formatter();
    let template = workspace_command.commit_summary_template();
    let mut choices: Vec<String> = Default::default();
    for (i, commit) in commits.iter().enumerate() {
        write!(formatter, "{}: ", i + 1)?;
        template.format(commit, formatter.as_mut())?;
        writeln!(formatter)?;
        choices.push(format!("{}", i + 1));
    }
    writeln!(formatter, "q: quit the prompt")?;
    choices.push("q".to_string());
    drop(formatter);

    let choice = ui.prompt_choice(
        "enter the index of the commit you want to target",
        &choices,
        None,
    )?;
    if choice == "q" {
        return Err(user_error("ambiguous target commit"));
    }

    Ok(&commits[choice.parse::<usize>().unwrap() - 1])
}

pub(crate) fn move_to_commit(
    ui: &mut Ui,
    command: &CommandHelper,
    direction: Direction,
    args: &MovementArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let current_wc_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;

    let config_edit_flag = command.settings().config().get_bool("ui.movement.edit")?;
    let args = MovementArgsInternal {
        should_edit: args.edit || (!args.no_edit && config_edit_flag),
        offset: args.offset,
        conflict: args.conflict,
    };

    let target = get_target_commit(ui, &workspace_command, direction, current_wc_id, &args)?;
    let current_short = short_commit_hash(current_wc_id);
    let target_short = short_commit_hash(target.id());
    let cmd = direction.cmd();
    // We're editing, just move to the target commit.
    if args.should_edit {
        // We're editing, the target must be rewritable.
        workspace_command.check_rewritable([target.id()])?;
        let mut tx = workspace_command.start_transaction();
        tx.edit(&target)?;
        tx.finish(
            ui,
            format!("{cmd}: {current_short} -> editing {target_short}"),
        )?;
        return Ok(());
    }
    let mut tx = workspace_command.start_transaction();
    // Move the working-copy commit to the new parent.
    tx.check_out(&target)?;
    tx.finish(ui, format!("{cmd}: {current_short} -> {target_short}"))?;
    Ok(())
}
