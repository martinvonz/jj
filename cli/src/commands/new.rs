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

use std::collections::HashSet;
use std::io::Write;

use clap::ArgGroup;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::CommitIteratorExt;
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

    let parent_commits;
    let parent_commit_ids;
    let children_commits;

    if args.insert_before {
        // Instead of having the new commit as a child of the changes given on the
        // command line, add it between the changes' parents and the changes.
        // The parents of the new commit will be the parents of the target commits
        // which are not descendants of other target commits.
        let children_expression = RevsetExpression::commits(target_ids);
        let parents_expression = children_expression.parents();
        if let Some(commit_id) = children_expression
            .dag_range_to(&parents_expression)
            .evaluate_programmatic(workspace_command.repo().as_ref())?
            .iter()
            .next()
        {
            return Err(user_error(format!(
                "Refusing to create a loop: commit {} would be both an ancestor and a descendant \
                 of the new commit",
                short_commit_hash(&commit_id),
            )));
        }
        // Manually collect the parent commit IDs to preserve the order of parents.
        parent_commit_ids = target_commits
            .iter()
            .flat_map(|commit| commit.parent_ids())
            .unique()
            .cloned()
            .collect_vec();
        parent_commits = parent_commit_ids
            .iter()
            .map(|commit_id| workspace_command.repo().store().get_commit(commit_id))
            .try_collect()?;
        children_commits = target_commits;
    } else if args.insert_after {
        parent_commits = target_commits;
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        let parents_expression = RevsetExpression::commits(target_ids);
        // Each child of the targets will be rebased: its set of parents will be updated
        // so that the targets are replaced by the new commit.
        // Exclude children that are ancestors of the new commit
        let children_expression = parents_expression
            .children()
            .minus(&parents_expression.ancestors());
        children_commits = children_expression
            .evaluate_programmatic(workspace_command.repo().as_ref())?
            .iter()
            .commits(workspace_command.repo().store())
            .try_collect()?;
    } else {
        parent_commits = target_commits;
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        children_commits = vec![];
    };
    workspace_command.check_rewritable(children_commits.iter().ids())?;

    let parent_commit_ids_set: HashSet<CommitId> = parent_commit_ids.iter().cloned().collect();

    let mut tx = workspace_command.start_transaction();
    let merged_tree = merge_commit_trees(tx.repo(), &parent_commits)?;
    let new_commit = tx
        .mut_repo()
        .new_commit(command.settings(), parent_commit_ids, merged_tree.id())
        .set_description(join_message_paragraphs(&args.message_paragraphs))
        .write()?;

    let mut num_rebased = 0;
    for child_commit in children_commits {
        let new_parent_ids = child_commit
            .parent_ids()
            .iter()
            .filter(|id| !parent_commit_ids_set.contains(id))
            .cloned()
            .chain(std::iter::once(new_commit.id().clone()))
            .collect_vec();
        rebase_commit(
            command.settings(),
            tx.mut_repo(),
            child_commit,
            new_parent_ids,
        )?;
        num_rebased += 1;
    }
    num_rebased += tx.mut_repo().rebase_descendants(command.settings())?;

    if args.no_edit {
        if let Some(mut formatter) = ui.status_formatter() {
            write!(formatter, "Created new commit ")?;
            tx.write_commit_summary(formatter.as_mut(), &new_commit)?;
            writeln!(formatter)?;
        }
    } else {
        tx.edit(&new_commit)?;
        // The description of the new commit will be printed by tx.finish()
    }
    if num_rebased > 0 {
        writeln!(ui.status(), "Rebased {num_rebased} descendant commits")?;
    }

    // Does nothing if there's no branches to advance.
    if let Some(target) = advance_branches_target {
        tx.advance_branches(advanceable_branches, &target);
    }

    tx.finish(ui, "new empty commit")?;
    Ok(())
}
