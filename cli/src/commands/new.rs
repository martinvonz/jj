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

use clap::ArgGroup;
use itertools::Itertools;
use jj_lib::commit::{Commit, CommitIteratorExt};
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use jj_lib::rewrite::{merge_commit_trees, rebase_commit};
use tracing::instrument;

use crate::cli_util::{short_commit_hash, CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::description_util::join_message_paragraphs;
use crate::ui::Ui;

/// Create a new, empty change and (by default) edit it in the working copy
///
/// By default, `jj` will edit the new change, making the working copy represent
/// the new commit. This can be avoided with `--no-edit`.
///
/// Note that you can create a merge commit by specifying multiple revisions as
/// argument. For example, `jj new main @` will create a new commit with the
/// `main` branch and the working copy as parents.
///
/// For more information, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("order").args(&["insert_after", "insert_before"])))]
pub(crate) struct NewArgs {
    /// Parent(s) of the new change
    #[arg(default_value = "@")]
    pub(crate) revisions: Vec<RevisionArg>,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true, action = clap::ArgAction::Count)]
    unused_revision: u8,
    /// The change description to use
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Deprecated. Please prefix the revset with `all:` instead.
    #[arg(long, short = 'L', hide = true)]
    allow_large_revsets: bool,
    /// Do not edit the newly created change
    #[arg(long, conflicts_with = "_edit")]
    no_edit: bool,
    /// No-op flag to pair with --no-edit
    #[arg(long, hide = true)]
    _edit: bool,
    /// Insert the new change between the target commit(s) and their children
    //
    // Repeating this flag is allowed, but has no effect.
    #[arg(
        long,
        short = 'A',
        visible_alias = "after",
        overrides_with = "insert_after"
    )]
    insert_after: bool,
    /// Insert the new change between the target commit(s) and their parents
    //
    // Repeating this flag is allowed, but has no effect.
    #[arg(
        long,
        short = 'B',
        visible_alias = "before",
        overrides_with = "insert_before"
    )]
    insert_before: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_new(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &NewArgs,
) -> Result<(), CommandError> {
    if args.allow_large_revsets {
        return Err(user_error(
            "--allow-large-revsets has been deprecated.
Please use `jj new 'all:x|y'` instead of `jj new --allow-large-revsets x y`.",
        ));
    }
    let mut workspace_command = command.workspace_helper(ui)?;
    let target_commits = workspace_command
        .resolve_some_revsets_default_single(&args.revisions)?
        .into_iter()
        .collect_vec();
    let target_ids = target_commits.iter().ids().cloned().collect_vec();

    let should_advance_branches =
        target_commits.len() == 1 && !args.insert_before && !args.insert_after;
    let (advance_branches_target, advanceable_branches) = if should_advance_branches {
        let ab_target = target_ids[0].clone();
        let ab_branches =
            workspace_command.get_advanceable_branches(target_commits[0].parent_ids())?;
        (Some(ab_target), ab_branches)
    } else {
        (None, Vec::new())
    };

    let mut tx = workspace_command.start_transaction();
    let mut num_rebased;
    let new_commit;
    if args.insert_before {
        // Instead of having the new commit as a child of the changes given on the
        // command line, add it between the changes' parents and the changes.
        // The parents of the new commit will be the parents of the target commits
        // which are not descendants of other target commits.
        tx.base_workspace_helper().check_rewritable(&target_ids)?;
        let new_children = RevsetExpression::commits(target_ids.clone());
        let new_parents = new_children.parents();
        if let Some(commit_id) = new_children
            .dag_range_to(&new_parents)
            .evaluate_programmatic(tx.repo())?
            .iter()
            .next()
        {
            return Err(user_error(format!(
                "Refusing to create a loop: commit {} would be both an ancestor and a descendant \
                 of the new commit",
                short_commit_hash(&commit_id),
            )));
        }
        let new_parents_commits: Vec<Commit> = new_parents
            .evaluate_programmatic(tx.repo())?
            .iter()
            .commits(tx.repo().store())
            .try_collect()?;
        let merged_tree = merge_commit_trees(tx.repo(), &new_parents_commits)?;
        let new_parents_commit_ids = new_parents_commits.iter().ids().cloned().collect();
        new_commit = tx
            .mut_repo()
            .new_commit(command.settings(), new_parents_commit_ids, merged_tree.id())
            .set_description(join_message_paragraphs(&args.message_paragraphs))
            .write()?;
        num_rebased = target_ids.len();
        for child_commit in target_commits {
            rebase_commit(
                command.settings(),
                tx.mut_repo(),
                child_commit,
                vec![new_commit.id().clone()],
            )?;
        }
    } else {
        let old_parents = RevsetExpression::commits(target_ids.clone());
        let commits_to_rebase: Vec<Commit> = if args.insert_after {
            // Each child of the targets will be rebased: its set of parents will be updated
            // so that the targets are replaced by the new commit.
            // Exclude children that are ancestors of the new commit
            let to_rebase = old_parents.children().minus(&old_parents.ancestors());
            to_rebase
                .evaluate_programmatic(tx.base_repo().as_ref())?
                .iter()
                .commits(tx.base_repo().store())
                .try_collect()?
        } else {
            vec![]
        };
        tx.base_workspace_helper()
            .check_rewritable(commits_to_rebase.iter().ids())?;
        let merged_tree = merge_commit_trees(tx.repo(), &target_commits)?;
        new_commit = tx
            .mut_repo()
            .new_commit(command.settings(), target_ids.clone(), merged_tree.id())
            .set_description(join_message_paragraphs(&args.message_paragraphs))
            .write()?;
        num_rebased = commits_to_rebase.len();
        for child_commit in commits_to_rebase {
            let commit_parents = RevsetExpression::commits(child_commit.parent_ids().to_owned());
            let new_parents = commit_parents.minus(&old_parents);
            let mut new_parent_ids = new_parents
                .evaluate_programmatic(tx.base_repo().as_ref())?
                .iter()
                .collect_vec();
            new_parent_ids.push(new_commit.id().clone());
            rebase_commit(
                command.settings(),
                tx.mut_repo(),
                child_commit,
                new_parent_ids,
            )?;
        }
    }
    num_rebased += tx.mut_repo().rebase_descendants(command.settings())?;
    if args.no_edit {
        if let Some(mut formatter) = ui.status_formatter() {
            write!(formatter, "Created new commit ")?;
            tx.write_commit_summary(formatter.as_mut(), &new_commit)?;
            writeln!(formatter)?;
        }
    } else {
        tx.edit(&new_commit).unwrap();
        // The description of the new commit will be printed by tx.finish()
    }
    if num_rebased > 0 {
        writeln!(ui.status(), "Rebased {num_rebased} descendant commits")?;
    }

    // Does nothing if there's no branches to advance.
    if let Some(target) = advance_branches_target {
        tx.advance_branches(ui, advanceable_branches, &target);
    }

    tx.finish(ui, "new empty commit")?;
    Ok(())
}
