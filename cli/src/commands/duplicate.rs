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

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::rc::Rc;

use clap::ArgGroup;
use indexmap::IndexMap;
use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::commit::{Commit, CommitIteratorExt};
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use tracing::instrument;

use crate::cli_util::{short_commit_hash, CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Create a new change with the same content as an existing one
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("target").args(&["destination", "insert_after", "insert_before"]).multiple(true)))]
pub(crate) struct DuplicateArgs {
    /// The revision(s) to duplicate
    #[arg(default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true, action = clap::ArgAction::Count)]
    unused_revision: u8,
    /// The revision(s) to rebase onto (can be repeated to create a merge
    /// commit)
    #[arg(long, short)]
    destination: Vec<RevisionArg>,
    /// The revision(s) to insert after (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short = 'A',
        visible_alias = "after",
        conflicts_with = "destination"
    )]
    insert_after: Vec<RevisionArg>,
    /// The revision(s) to insert before (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short = 'B',
        visible_alias = "before",
        conflicts_with = "destination"
    )]
    insert_before: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DuplicateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_duplicate: Vec<CommitId> = workspace_command
        .parse_union_revsets(&args.revisions)?
        .evaluate_to_commit_ids()?
        .collect(); // in reverse topological order
    if to_duplicate.is_empty() {
        writeln!(ui.status(), "No revisions to duplicate.")?;
        return Ok(());
    }
    if to_duplicate.last() == Some(workspace_command.repo().store().root_commit_id()) {
        return Err(user_error("Cannot duplicate the root commit"));
    }

    let parent_commit_ids: Vec<CommitId>;
    let children_commit_ids: Vec<CommitId>;

    if !args.insert_before.is_empty() && !args.insert_after.is_empty() {
        let parent_commits = workspace_command
            .resolve_some_revsets_default_single(&args.insert_after)?
            .into_iter()
            .collect_vec();
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        let children_commits = workspace_command
            .resolve_some_revsets_default_single(&args.insert_before)?
            .into_iter()
            .collect_vec();
        children_commit_ids = children_commits.iter().ids().cloned().collect();
        workspace_command.check_rewritable(&children_commit_ids)?;
        let children_expression = RevsetExpression::commits(children_commit_ids.clone());
        let parents_expression = RevsetExpression::commits(parent_commit_ids.clone());
        ensure_no_commit_loop(
            workspace_command.repo(),
            &children_expression,
            &parents_expression,
        )?;
    } else if !args.insert_before.is_empty() {
        let children_commits = workspace_command
            .resolve_some_revsets_default_single(&args.insert_before)?
            .into_iter()
            .collect_vec();
        children_commit_ids = children_commits.iter().ids().cloned().collect();
        workspace_command.check_rewritable(&children_commit_ids)?;
        let children_expression = RevsetExpression::commits(children_commit_ids.clone());
        let parents_expression = children_expression.parents();
        ensure_no_commit_loop(
            workspace_command.repo(),
            &children_expression,
            &parents_expression,
        )?;
        // Manually collect the parent commit IDs to preserve the order of parents.
        parent_commit_ids = children_commits
            .iter()
            .flat_map(|commit| commit.parent_ids())
            .unique()
            .cloned()
            .collect_vec();
    } else if !args.insert_after.is_empty() {
        let parent_commits = workspace_command
            .resolve_some_revsets_default_single(&args.insert_after)?
            .into_iter()
            .collect_vec();
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        let parents_expression = RevsetExpression::commits(parent_commit_ids.clone());
        let children_expression = parents_expression.children();
        children_commit_ids = children_expression
            .evaluate_programmatic(workspace_command.repo().as_ref())?
            .iter()
            .collect();
        workspace_command.check_rewritable(&children_commit_ids)?;
    } else if !args.destination.is_empty() {
        let parent_commits = workspace_command
            .resolve_some_revsets_default_single(&args.destination)?
            .into_iter()
            .collect_vec();
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        children_commit_ids = vec![];
    } else {
        parent_commit_ids = vec![];
        children_commit_ids = vec![];
    };

    let mut duplicated_old_to_new: IndexMap<&CommitId, Commit> = IndexMap::new();
    let mut num_rebased = 0;

    let mut tx = workspace_command.start_transaction();
    let base_repo = tx.base_repo().clone();
    let store = base_repo.store();
    let mut_repo = tx.mut_repo();

    // If there are no parent commits specified, duplicate each commit on top of
    // their parents or other duplicated commits only.
    if parent_commit_ids.is_empty() {
        for original_commit_id in to_duplicate.iter().rev() {
            // Topological order ensures that any parents of `original_commit` are
            // either not in `target_commits` or were already duplicated.
            let original_commit = store.get_commit(original_commit_id)?;
            let new_parents = original_commit
                .parent_ids()
                .iter()
                .map(|id| duplicated_old_to_new.get(id).map_or(id, |c| c.id()).clone())
                .collect();
            let new_commit = mut_repo
                .rewrite_commit(command.settings(), &original_commit)
                .generate_new_change_id()
                .set_parents(new_parents)
                .write()?;
            duplicated_old_to_new.insert(original_commit_id, new_commit);
        }
    } else {
        let target_commits = to_duplicate.clone();

        let connected_target_commits: Vec<_> =
            RevsetExpression::commits(target_commits.iter().cloned().collect_vec())
                .connected()
                .evaluate_programmatic(mut_repo)?
                .iter()
                .commits(store)
                .try_collect()?;

        // If a commit in the target set has a parent which is not in the set, but has
        // an ancestor which is in the set, then the commit will have that ancestor
        // as a parent instead.
        let mut target_commits_internal_parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
        for commit in connected_target_commits.iter().rev() {
            // The roots of the set will not have any parents found, and will be stored as
            // an empty vector.
            let mut new_parents = vec![];
            for old_parent in commit.parent_ids() {
                if target_commits.contains(old_parent) {
                    new_parents.push(old_parent.clone());
                } else if let Some(parents) = target_commits_internal_parents.get(old_parent) {
                    new_parents.extend(parents.iter().cloned());
                }
            }
            target_commits_internal_parents.insert(commit.id().clone(), new_parents);
        }
        target_commits_internal_parents.retain(|id, _| target_commits.contains(id));

        // Compute the roots of `target_commits`.
        let target_roots: HashSet<_> = target_commits_internal_parents
            .iter()
            .filter(|(_, parents)| parents.is_empty())
            .map(|(commit_id, _)| commit_id.clone())
            .collect();

        // Compute the heads of the target set, which will be used as the parents of
        // `children_commits`.
        let target_heads: Vec<CommitId> = if !children_commit_ids.is_empty() {
            let mut target_heads: HashSet<CommitId> = HashSet::new();
            for commit in connected_target_commits.iter().rev() {
                target_heads.insert(commit.id().clone());
                for old_parent in commit.parent_ids() {
                    target_heads.remove(old_parent);
                }
            }
            connected_target_commits
                .iter()
                .rev()
                .filter(|commit| {
                    target_heads.contains(commit.id()) && target_commits.contains(commit.id())
                })
                .map(|commit| commit.id().clone())
                .collect_vec()
        } else {
            vec![]
        };

        for original_commit_id in to_duplicate.iter().rev() {
            // Topological order ensures that any parents of `original_commit` are
            // either not in `target_commits` or were already duplicated.
            let original_commit = store.get_commit(original_commit_id)?;
            let new_parents = if target_roots.contains(original_commit_id) {
                parent_commit_ids.clone()
            } else {
                original_commit
                    .parent_ids()
                    .iter()
                    .filter(|id| {
                        // Filter out parents which are descendants of the children commits.
                        !children_commit_ids.iter().any(|child_commit_id| {
                            mut_repo.index().is_ancestor(child_commit_id, id)
                        })
                    })
                    .flat_map(|id| {
                        // Get the new IDs of the parents of `original_commit`.
                        target_commits_internal_parents
                            .get(id)
                            .map_or_else(|| vec![id.clone()], |parents| parents.clone())
                            .into_iter()
                            // Replace parent IDs with their new IDs if they were duplicated.
                            .map(|id| {
                                duplicated_old_to_new
                                    .get(&id)
                                    .map_or(id, |c| c.id().clone())
                            })
                    })
                    .collect()
            };
            let new_commit = mut_repo
                .rewrite_commit(command.settings(), &original_commit)
                .generate_new_change_id()
                .set_parents(new_parents)
                .write()?;
            duplicated_old_to_new.insert(original_commit_id, new_commit);
        }

        // Replace the original commit IDs in `target_heads` with the duplicated commit
        // IDs.
        let target_heads = target_heads
            .into_iter()
            .map(|commit_id| {
                duplicated_old_to_new
                    .get(&commit_id)
                    .map_or_else(|| commit_id, |c| c.id().clone())
            })
            .collect_vec();

        // Rebase new children onto `target_heads`.
        let children_commit_ids_set: HashSet<CommitId> =
            children_commit_ids.iter().cloned().collect();
        tx.mut_repo().transform_descendants(
            command.settings(),
            children_commit_ids,
            |mut rewriter| {
                if children_commit_ids_set.contains(rewriter.old_commit().id()) {
                    let new_parents: Vec<CommitId> = rewriter
                        .old_commit()
                        .parent_ids()
                        .iter()
                        .filter(|id| !parent_commit_ids.contains(id))
                        .chain(target_heads.iter())
                        .cloned()
                        .collect();
                    rewriter.set_new_parents(new_parents);
                }
                num_rebased += 1;
                rewriter.rebase(command.settings())?.write()?;
                Ok(())
            },
        )?;
    }

    if let Some(mut formatter) = ui.status_formatter() {
        for (old_id, new_commit) in &duplicated_old_to_new {
            write!(formatter, "Duplicated {} as ", short_commit_hash(old_id))?;
            tx.write_commit_summary(formatter.as_mut(), new_commit)?;
            writeln!(formatter)?;
        }
        if num_rebased > 0 {
            writeln!(
                ui.status(),
                "Rebased {num_rebased} commits onto duplicated commits"
            )?;
        }
    }
    tx.finish(ui, format!("duplicate {} commit(s)", to_duplicate.len()))?;
    Ok(())
}

/// Ensure that there is no possible cycle between the potential children and
/// parents of the duplicated commits.
fn ensure_no_commit_loop(
    repo: &ReadonlyRepo,
    children_expression: &Rc<RevsetExpression>,
    parents_expression: &Rc<RevsetExpression>,
) -> Result<(), CommandError> {
    if let Some(commit_id) = children_expression
        .dag_range_to(parents_expression)
        .evaluate_programmatic(repo)?
        .iter()
        .next()
    {
        return Err(user_error(format!(
            "Refusing to create a loop: commit {} would be both an ancestor and a descendant of \
             the duplicated commits",
            short_commit_hash(&commit_id),
        )));
    }
    Ok(())
}
