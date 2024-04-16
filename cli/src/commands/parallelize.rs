// Copyright 2024 The Jujutsu Authors
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

use indexmap::IndexSet;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::{Commit, CommitIteratorExt};
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Parallelize revisions by making them siblings
///
/// Running `jj parallelize 1::2` will transform the history like this:
/// ```text
/// 3
/// |             3
/// 2            / \
/// |    ->     1   2
/// 1            \ /
/// |             0
/// 0
/// ```
///
/// Each of the target revisions is rebased onto the parents of the root(s) of
/// the target revset (not to be confused with the repo root). The children of
/// the head(s) of the target revset are rebased onto the target revisions.
///
/// The target revset is the union of the `revisions` arguments and must satisfy
/// several conditions, otherwise the command will fail.
///
/// 1. The heads of the target revset must have either the same children as the
///    other heads or none.
/// 2. The roots of the target revset have the same parents.
/// 3. The parents of all target revisions except the roots must also be
///    parallelized. This means that the target revisions must be connected.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct ParallelizeArgs {
    /// Revisions to parallelize
    revisions: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_parallelize(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ParallelizeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    // The target commits are the commits being parallelized. They are ordered
    // here with children before parents.
    let target_commits: Vec<Commit> = workspace_command
        .parse_union_revsets(&args.revisions)?
        .evaluate_to_commits()?
        .try_collect()?;
    if target_commits.len() < 2 {
        writeln!(ui.status(), "Nothing changed.")?;
        return Ok(());
    }
    workspace_command.check_rewritable(target_commits.iter().ids())?;

    let mut tx = workspace_command.start_transaction();
    let target_revset =
        RevsetExpression::commits(target_commits.iter().ids().cloned().collect_vec());
    // TODO: The checks are now unnecessary, so drop them
    let _new_parents =
        check_preconditions_and_get_new_parents(&target_revset, &target_commits, tx.repo())?;

    // New parents for commits in the target set. Since commits in the set are now
    // supposed to be independent, they inherit the parent's non-target parents,
    // recursively.
    let mut new_target_parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = vec![];
        for old_parent in commit.parent_ids() {
            if let Some(grand_parents) = new_target_parents.get(old_parent) {
                new_parents.extend_from_slice(grand_parents);
            } else {
                new_parents.push(old_parent.clone());
            }
        }
        new_target_parents.insert(commit.id().clone(), new_parents);
    }

    // If a commit outside the target set has a commit in the target set as parent,
    // then - after the transformation - it should also have that commit's
    // parents as direct parents, if those commits are also in the target set.
    let mut new_child_parents: HashMap<CommitId, IndexSet<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = IndexSet::new();
        for old_parent in commit.parent_ids() {
            if let Some(parents) = new_child_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            }
        }
        new_parents.insert(commit.id().clone());
        new_child_parents.insert(commit.id().clone(), new_parents);
    }

    tx.mut_repo().transform_descendants(
        command.settings(),
        target_commits.iter().ids().cloned().collect_vec(),
        |mut rewriter| {
            // Commits in the target set do not depend on each other but they still depend
            // on other parents
            if let Some(new_parents) = new_target_parents.get(rewriter.old_commit().id()) {
                rewriter.set_new_rewritten_parents(new_parents.clone());
            } else if rewriter
                .old_commit()
                .parent_ids()
                .iter()
                .any(|id| new_child_parents.contains_key(id))
            {
                let mut new_parents = vec![];
                for parent in rewriter.old_commit().parent_ids() {
                    if let Some(parents) = new_child_parents.get(parent) {
                        new_parents.extend(parents.iter().cloned());
                    } else {
                        new_parents.push(parent.clone());
                    }
                }
                rewriter.set_new_rewritten_parents(new_parents);
            }
            if rewriter.parents_changed() {
                let builder = rewriter.rebase(command.settings())?;
                builder.write()?;
            }
            Ok(())
        },
    )?;

    tx.finish(ui, format!("parallelize {} commits", target_commits.len()))
}

/// Returns the new parents of the parallelized commits or an error if any of
/// the following preconditions are not met:
///
/// 1. If the heads of the target revset must not have different children.
/// 2. The roots of the target revset must not have different parents.
/// 3. The parents of all target revisions except the roots must also be
///    parallelized. This means that the target revisions must be connected.
///
/// The `target_revset` must evaluate to the commits in `target_commits` when
/// the provided `repo` is used.
fn check_preconditions_and_get_new_parents(
    target_revset: &Rc<RevsetExpression>,
    target_commits: &[Commit],
    repo: &dyn Repo,
) -> Result<Vec<Commit>, CommandError> {
    check_target_heads(target_revset, repo)?;
    let target_roots = check_target_roots(target_revset, repo)?;
    check_target_commits_are_connected(&target_roots, target_commits)?;

    // We already verified that the roots have the same parents, so we can just
    // use the first root.
    Ok(target_roots[0].parents())
}

/// Returns an error if the heads of the target revset have children which are
/// different.
fn check_target_heads(
    target_revset: &Rc<RevsetExpression>,
    repo: &dyn Repo,
) -> Result<(), CommandError> {
    let target_heads = target_revset
        .heads()
        .evaluate_programmatic(repo)?
        .iter()
        .sorted()
        .collect_vec();
    if target_heads.len() == 1 {
        return Ok(());
    }
    let all_children: Vec<Commit> = target_revset
        .heads()
        .children()
        .evaluate_programmatic(repo)?
        .iter()
        .commits(repo.store())
        .try_collect()?;
    // Every child must have every target head as a parent, otherwise it means
    // that the target heads have different children.
    for child in all_children {
        let parents = child.parent_ids().iter().sorted();
        if !parents.eq(target_heads.iter()) {
            return Err(user_error(
                "All heads of the target revisions must have the same children.",
            ));
        }
    }
    Ok(())
}

/// Returns the roots of the target revset or an error if their parents are
/// different.
fn check_target_roots(
    target_revset: &Rc<RevsetExpression>,
    repo: &dyn Repo,
) -> Result<Vec<Commit>, CommandError> {
    let target_roots: Vec<Commit> = target_revset
        .roots()
        .evaluate_programmatic(repo)?
        .iter()
        .commits(repo.store())
        .try_collect()?;
    let expected_parents = target_roots[0].parent_ids().iter().sorted().collect_vec();
    for root in target_roots[1..].iter() {
        let root_parents = root.parent_ids().iter().sorted();
        if !root_parents.eq(expected_parents.iter().copied()) {
            return Err(user_error(
                "All roots of the target revisions must have the same parents.",
            ));
        }
    }
    Ok(target_roots)
}

/// The target commits must be connected. The parents of every target commit
/// except the root commit must also be target commits. Returns an error if this
/// requirement is not met.
fn check_target_commits_are_connected(
    target_roots: &[Commit],
    target_commits: &[Commit],
) -> Result<(), CommandError> {
    let target_commit_ids: HashSet<CommitId> = target_commits.iter().ids().cloned().collect();
    for target_commit in target_commits.iter() {
        if target_roots.iter().ids().contains(target_commit.id()) {
            continue;
        }
        for parent in target_commit.parent_ids() {
            if !target_commit_ids.contains(parent) {
                // We check this condition to return a more useful error to the user.
                if target_commit.parent_ids().len() == 1 {
                    return Err(user_error(
                        "Cannot parallelize since the target revisions are not connected.",
                    ));
                }
                return Err(user_error(
                    "Only the roots of the target revset are allowed to have parents which are \
                     not being parallelized.",
                ));
            }
        }
    }
    Ok(())
}
