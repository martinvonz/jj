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

mod create;
mod delete;
mod forget;
mod list;
mod r#move;
mod rename;
mod set;
mod track;
mod untrack;

use std::fmt;

use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::op_store::{RefTarget, RemoteRef};
use jj_lib::repo::Repo;
use jj_lib::str_util::StringPattern;
use jj_lib::view::View;

use self::create::{cmd_branch_create, BranchCreateArgs};
use self::delete::{cmd_branch_delete, BranchDeleteArgs};
use self::forget::{cmd_branch_forget, BranchForgetArgs};
use self::list::{cmd_branch_list, BranchListArgs};
use self::r#move::{cmd_branch_move, BranchMoveArgs};
use self::rename::{cmd_branch_rename, BranchRenameArgs};
use self::set::{cmd_branch_set, BranchSetArgs};
use self::track::{cmd_branch_track, BranchTrackArgs};
use self::untrack::{cmd_branch_untrack, BranchUntrackArgs};
use crate::cli_util::{CommandHelper, RemoteBranchName, RemoteBranchNamePattern};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Manage branches.
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum BranchCommand {
    #[command(visible_alias("c"))]
    Create(BranchCreateArgs),
    #[command(visible_alias("d"))]
    Delete(BranchDeleteArgs),
    #[command(visible_alias("f"))]
    Forget(BranchForgetArgs),
    #[command(visible_alias("l"))]
    List(BranchListArgs),
    #[command(visible_alias("m"))]
    Move(BranchMoveArgs),
    #[command(visible_alias("r"))]
    Rename(BranchRenameArgs),
    #[command(visible_alias("s"))]
    Set(BranchSetArgs),
    #[command(visible_alias("t"))]
    Track(BranchTrackArgs),
    Untrack(BranchUntrackArgs),
}

fn make_branch_term(branch_names: &[impl fmt::Display]) -> String {
    match branch_names {
        [branch_name] => format!("branch {}", branch_name),
        branch_names => format!("branches {}", branch_names.iter().join(", ")),
    }
}

pub fn cmd_branch(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BranchCommand,
) -> Result<(), CommandError> {
    match subcommand {
        BranchCommand::Create(sub_args) => cmd_branch_create(ui, command, sub_args),
        BranchCommand::Rename(sub_args) => cmd_branch_rename(ui, command, sub_args),
        BranchCommand::Set(sub_args) => cmd_branch_set(ui, command, sub_args),
        BranchCommand::Move(sub_args) => cmd_branch_move(ui, command, sub_args),
        BranchCommand::Delete(sub_args) => cmd_branch_delete(ui, command, sub_args),
        BranchCommand::Forget(sub_args) => cmd_branch_forget(ui, command, sub_args),
        BranchCommand::Track(sub_args) => cmd_branch_track(ui, command, sub_args),
        BranchCommand::Untrack(sub_args) => cmd_branch_untrack(ui, command, sub_args),
        BranchCommand::List(sub_args) => cmd_branch_list(ui, command, sub_args),
    }
}

fn find_local_branches(
    view: &View,
    name_patterns: &[StringPattern],
) -> Result<Vec<String>, CommandError> {
    find_branches_with(name_patterns, |pattern| {
        view.local_branches_matching(pattern)
            .map(|(name, _)| name.to_owned())
    })
}

fn find_branches_with<'a, I: Iterator<Item = String>>(
    name_patterns: &'a [StringPattern],
    mut find_matches: impl FnMut(&'a StringPattern) -> I,
) -> Result<Vec<String>, CommandError> {
    let mut matching_branches: Vec<String> = vec![];
    let mut unmatched_patterns = vec![];
    for pattern in name_patterns {
        let mut names = find_matches(pattern).peekable();
        if names.peek().is_none() {
            unmatched_patterns.push(pattern);
        }
        matching_branches.extend(names);
    }
    match &unmatched_patterns[..] {
        [] => {
            matching_branches.sort_unstable();
            matching_branches.dedup();
            Ok(matching_branches)
        }
        [pattern] if pattern.is_exact() => Err(user_error(format!("No such branch: {pattern}"))),
        patterns => Err(user_error(format!(
            "No matching branches for patterns: {}",
            patterns.iter().join(", ")
        ))),
    }
}

fn find_remote_branches<'a>(
    view: &'a View,
    name_patterns: &[RemoteBranchNamePattern],
) -> Result<Vec<(RemoteBranchName, &'a RemoteRef)>, CommandError> {
    let mut matching_branches = vec![];
    let mut unmatched_patterns = vec![];
    for pattern in name_patterns {
        let mut matches = view
            .remote_branches_matching(&pattern.branch, &pattern.remote)
            .map(|((branch, remote), remote_ref)| {
                let name = RemoteBranchName {
                    branch: branch.to_owned(),
                    remote: remote.to_owned(),
                };
                (name, remote_ref)
            })
            .peekable();
        if matches.peek().is_none() {
            unmatched_patterns.push(pattern);
        }
        matching_branches.extend(matches);
    }
    match &unmatched_patterns[..] {
        [] => {
            matching_branches.sort_unstable_by(|(name1, _), (name2, _)| name1.cmp(name2));
            matching_branches.dedup_by(|(name1, _), (name2, _)| name1 == name2);
            Ok(matching_branches)
        }
        [pattern] if pattern.is_exact() => {
            Err(user_error(format!("No such remote branch: {pattern}")))
        }
        patterns => Err(user_error(format!(
            "No matching remote branches for patterns: {}",
            patterns.iter().join(", ")
        ))),
    }
}

fn is_fast_forward(repo: &dyn Repo, old_target: &RefTarget, new_target_id: &CommitId) -> bool {
    if old_target.is_present() {
        // Strictly speaking, "all" old targets should be ancestors, but we allow
        // conflict resolution by setting branch to "any" of the old target descendants.
        old_target
            .added_ids()
            .any(|old| repo.index().is_ancestor(old, new_target_id))
    } else {
        true
    }
}
