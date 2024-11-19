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

#![allow(missing_docs)]

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;
use indexmap::IndexMap;
use indexmap::IndexSet;
use itertools::Itertools;
use pollster::FutureExt;
use tracing::instrument;

use crate::backend::BackendError;
use crate::backend::BackendResult;
use crate::backend::CommitId;
use crate::backend::MergedTreeId;
use crate::commit::Commit;
use crate::commit::CommitIteratorExt;
use crate::commit_builder::CommitBuilder;
use crate::dag_walk;
use crate::index::Index;
use crate::matchers::Matcher;
use crate::matchers::Visit;
use crate::merged_tree::MergedTree;
use crate::merged_tree::MergedTreeBuilder;
use crate::merged_tree::TreeDiffEntry;
use crate::repo::MutableRepo;
use crate::repo::Repo;
use crate::repo_path::RepoPath;
use crate::revset::RevsetExpression;
use crate::revset::RevsetIteratorExt;
use crate::settings::UserSettings;
use crate::store::Store;

/// Merges `commits` and tries to resolve any conflicts recursively.
#[instrument(skip(repo))]
pub fn merge_commit_trees(repo: &dyn Repo, commits: &[Commit]) -> BackendResult<MergedTree> {
    if let [commit] = commits {
        commit.tree()
    } else {
        merge_commit_trees_no_resolve_without_repo(repo.store(), repo.index(), commits)?.resolve()
    }
}

/// Merges `commits` without attempting to resolve file conflicts.
#[instrument(skip(index))]
pub fn merge_commit_trees_no_resolve_without_repo(
    store: &Arc<Store>,
    index: &dyn Index,
    commits: &[Commit],
) -> BackendResult<MergedTree> {
    if commits.is_empty() {
        Ok(store.get_root_tree(&store.empty_merged_tree_id())?)
    } else {
        let mut new_tree = commits[0].tree()?;
        let commit_ids = commits
            .iter()
            .map(|commit| commit.id().clone())
            .collect_vec();
        for (i, other_commit) in commits.iter().enumerate().skip(1) {
            let ancestor_ids = index.common_ancestors(&commit_ids[0..i], &commit_ids[i..][..1]);
            let ancestors: Vec<_> = ancestor_ids
                .iter()
                .map(|id| store.get_commit(id))
                .try_collect()?;
            let ancestor_tree =
                merge_commit_trees_no_resolve_without_repo(store, index, &ancestors)?;
            let other_tree = other_commit.tree()?;
            new_tree = new_tree.merge_no_resolve(&ancestor_tree, &other_tree);
        }
        Ok(new_tree)
    }
}

/// Restore matching paths from the source into the destination.
pub fn restore_tree(
    source: &MergedTree,
    destination: &MergedTree,
    matcher: &dyn Matcher,
) -> BackendResult<MergedTreeId> {
    if matcher.visit(RepoPath::root()) == Visit::AllRecursively {
        // Optimization for a common case
        Ok(source.id())
    } else {
        // TODO: We should be able to not traverse deeper in the diff if the matcher
        // matches an entire subtree.
        let mut tree_builder = MergedTreeBuilder::new(destination.id().clone());
        async {
            // TODO: handle copy tracking
            let mut diff_stream = source.diff_stream(destination, matcher);
            while let Some(TreeDiffEntry {
                path: repo_path,
                values,
            }) = diff_stream.next().await
            {
                let (source_value, _destination_value) = values?;
                tree_builder.set_or_remove(repo_path, source_value);
            }
            Ok::<(), BackendError>(())
        }
        .block_on()?;
        tree_builder.write_tree(destination.store())
    }
}

pub fn rebase_commit(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    old_commit: Commit,
    new_parents: Vec<CommitId>,
) -> BackendResult<Commit> {
    let rewriter = CommitRewriter::new(mut_repo, old_commit, new_parents);
    let builder = rewriter.rebase(settings)?;
    builder.write()
}

/// Helps rewrite a commit.
pub struct CommitRewriter<'repo> {
    mut_repo: &'repo mut MutableRepo,
    old_commit: Commit,
    new_parents: Vec<CommitId>,
}

impl<'repo> CommitRewriter<'repo> {
    /// Create a new instance.
    pub fn new(
        mut_repo: &'repo mut MutableRepo,
        old_commit: Commit,
        new_parents: Vec<CommitId>,
    ) -> Self {
        Self {
            mut_repo,
            old_commit,
            new_parents,
        }
    }

    /// Returns the `MutableRepo`.
    pub fn mut_repo(&mut self) -> &mut MutableRepo {
        self.mut_repo
    }

    /// The commit we're rewriting.
    pub fn old_commit(&self) -> &Commit {
        &self.old_commit
    }

    /// Get the old commit's intended new parents.
    pub fn new_parents(&self) -> &[CommitId] {
        &self.new_parents
    }

    /// Set the old commit's intended new parents.
    pub fn set_new_parents(&mut self, new_parents: Vec<CommitId>) {
        self.new_parents = new_parents;
    }

    /// Set the old commit's intended new parents to be the rewritten versions
    /// of the given parents.
    pub fn set_new_rewritten_parents(&mut self, unrewritten_parents: &[CommitId]) {
        self.new_parents = self.mut_repo.new_parents(unrewritten_parents);
    }

    /// Update the intended new parents by replacing any occurrence of
    /// `old_parent` by `new_parents`.
    pub fn replace_parent<'a>(
        &mut self,
        old_parent: &CommitId,
        new_parents: impl IntoIterator<Item = &'a CommitId>,
    ) {
        if let Some(i) = self.new_parents.iter().position(|p| p == old_parent) {
            self.new_parents
                .splice(i..i + 1, new_parents.into_iter().cloned());
            let mut unique = HashSet::new();
            self.new_parents.retain(|p| unique.insert(p.clone()));
        }
    }

    /// Checks if the intended new parents are different from the old commit's
    /// parents.
    pub fn parents_changed(&self) -> bool {
        self.new_parents != self.old_commit.parent_ids()
    }

    /// If a merge commit would end up with one parent being an ancestor of the
    /// other, then filter out the ancestor.
    pub fn simplify_ancestor_merge(&mut self) {
        let head_set: HashSet<_> = self
            .mut_repo
            .index()
            .heads(&mut self.new_parents.iter())
            .into_iter()
            .collect();
        self.new_parents.retain(|parent| head_set.contains(parent));
    }

    /// Records the old commit as abandoned with the new parents.
    pub fn abandon(self) {
        let old_commit_id = self.old_commit.id().clone();
        let new_parents = self.new_parents;
        self.mut_repo
            .record_abandoned_commit_with_parents(old_commit_id, new_parents);
    }

    /// Rebase the old commit onto the new parents. Returns a `CommitBuilder`
    /// for the new commit. Returns `None` if the commit was abandoned.
    pub fn rebase_with_empty_behavior(
        self,
        settings: &UserSettings,
        empty: EmptyBehaviour,
    ) -> BackendResult<Option<CommitBuilder<'repo>>> {
        let old_parents: Vec<_> = self.old_commit.parents().try_collect()?;
        let old_parent_trees = old_parents
            .iter()
            .map(|parent| parent.tree_id().clone())
            .collect_vec();
        let new_parents: Vec<_> = self
            .new_parents
            .iter()
            .map(|new_parent_id| self.mut_repo.store().get_commit(new_parent_id))
            .try_collect()?;
        let new_parent_trees = new_parents
            .iter()
            .map(|parent| parent.tree_id().clone())
            .collect_vec();

        let (was_empty, new_tree_id) = if new_parent_trees == old_parent_trees {
            (
                // Optimization: was_empty is only used for newly empty, but when the
                // parents haven't changed it can't be newly empty.
                true,
                // Optimization: Skip merging.
                self.old_commit.tree_id().clone(),
            )
        } else {
            let old_base_tree = merge_commit_trees_no_resolve_without_repo(
                self.mut_repo.store(),
                self.mut_repo.index(),
                &old_parents,
            )?;
            let new_base_tree = merge_commit_trees_no_resolve_without_repo(
                self.mut_repo.store(),
                self.mut_repo.index(),
                &new_parents,
            )?;
            let old_tree = self.old_commit.tree()?;
            (
                old_base_tree.id() == *self.old_commit.tree_id(),
                new_base_tree.merge(&old_base_tree, &old_tree)?.id(),
            )
        };
        // Ensure we don't abandon commits with multiple parents (merge commits), even
        // if they're empty.
        if let [parent] = &new_parents[..] {
            let should_abandon = match empty {
                EmptyBehaviour::Keep => false,
                EmptyBehaviour::AbandonNewlyEmpty => *parent.tree_id() == new_tree_id && !was_empty,
                EmptyBehaviour::AbandonAllEmpty => *parent.tree_id() == new_tree_id,
            };
            if should_abandon {
                self.abandon();
                return Ok(None);
            }
        }

        let builder = self
            .mut_repo
            .rewrite_commit(settings, &self.old_commit)
            .set_parents(self.new_parents)
            .set_tree_id(new_tree_id);
        Ok(Some(builder))
    }

    /// Rebase the old commit onto the new parents. Returns a `CommitBuilder`
    /// for the new commit.
    pub fn rebase(self, settings: &UserSettings) -> BackendResult<CommitBuilder<'repo>> {
        let builder = self.rebase_with_empty_behavior(settings, EmptyBehaviour::Keep)?;
        Ok(builder.unwrap())
    }

    /// Rewrite the old commit onto the new parents without changing its
    /// contents. Returns a `CommitBuilder` for the new commit.
    pub fn reparent(self, settings: &UserSettings) -> BackendResult<CommitBuilder<'repo>> {
        Ok(self
            .mut_repo
            .rewrite_commit(settings, &self.old_commit)
            .set_parents(self.new_parents))
    }
}

pub enum RebasedCommit {
    Rewritten(Commit),
    Abandoned { parent_id: CommitId },
}

pub fn rebase_commit_with_options(
    settings: &UserSettings,
    mut rewriter: CommitRewriter<'_>,
    options: &RebaseOptions,
) -> BackendResult<RebasedCommit> {
    // If specified, don't create commit where one parent is an ancestor of another.
    if options.simplify_ancestor_merge {
        rewriter.simplify_ancestor_merge();
    }

    let single_parent = match &rewriter.new_parents[..] {
        [parent_id] => Some(parent_id.clone()),
        _ => None,
    };
    let new_parents_len = rewriter.new_parents.len();
    if let Some(builder) = rewriter.rebase_with_empty_behavior(settings, options.empty)? {
        let new_commit = builder.write()?;
        Ok(RebasedCommit::Rewritten(new_commit))
    } else {
        assert_eq!(new_parents_len, 1);
        Ok(RebasedCommit::Abandoned {
            parent_id: single_parent.unwrap(),
        })
    }
}

/// Moves changes from `sources` to the `destination` parent, returns new tree.
pub fn rebase_to_dest_parent(
    repo: &dyn Repo,
    sources: &[Commit],
    destination: &Commit,
) -> BackendResult<MergedTree> {
    if let [source] = sources {
        if source.parent_ids() == destination.parent_ids() {
            return source.tree();
        }
    }
    sources.iter().try_fold(
        destination.parent_tree(repo)?,
        |destination_tree, source| {
            let source_parent_tree = source.parent_tree(repo)?;
            let source_tree = source.tree()?;
            destination_tree.merge(&source_parent_tree, &source_tree)
        },
    )
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum EmptyBehaviour {
    /// Always keep empty commits
    #[default]
    Keep,
    /// Skips commits that would be empty after the rebase, but that were not
    /// originally empty.
    /// Will never skip merge commits with multiple non-empty parents.
    AbandonNewlyEmpty,
    /// Skips all empty commits, including ones that were empty before the
    /// rebase.
    /// Will never skip merge commits with multiple non-empty parents.
    AbandonAllEmpty,
}

/// Controls the configuration of a rebase.
// If we wanted to add a flag similar to `git rebase --ignore-date`, then this
// makes it much easier by ensuring that the only changes required are to
// change the RebaseOptions construction in the CLI, and changing the
// rebase_commit function to actually use the flag, and ensure we don't need to
// plumb it in.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct RebaseOptions {
    pub empty: EmptyBehaviour,
    /// If a merge commit would end up with one parent being an ancestor of the
    /// other, then filter out the ancestor.
    pub simplify_ancestor_merge: bool,
}

#[derive(Default)]
pub struct MoveCommitsStats {
    /// The number of commits in the target set which were rebased.
    pub num_rebased_targets: u32,
    /// The number of descendant commits which were rebased.
    pub num_rebased_descendants: u32,
    /// The number of commits for which rebase was skipped, due to the commit
    /// already being in place.
    pub num_skipped_rebases: u32,
    /// The number of commits which were abandoned.
    pub num_abandoned: u32,
}

pub enum MoveCommitsTarget {
    /// The commits to be moved. Commits should be mutable and in reverse
    /// topological order.
    Commits(Vec<Commit>),
    /// The root commits to be moved, along with all their descendants.
    Roots(Vec<Commit>),
}

/// Moves `target_commits` from their current location to a new location in the
/// graph.
///
/// Commits in `target` are rebased onto the new parents given by
/// `new_parent_ids`, while the `new_children` commits are rebased onto the
/// heads of the commits in `targets`. This assumes that commits in `target` and
/// `new_children` can be rewritten, and there will be no cycles in the
/// resulting graph. Commits in `target` should be in reverse topological order.
pub fn move_commits(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    new_parent_ids: &[CommitId],
    new_children: &[Commit],
    target: &MoveCommitsTarget,
    options: &RebaseOptions,
) -> BackendResult<MoveCommitsStats> {
    let target_commits: Vec<Commit>;
    let target_commit_ids: HashSet<_>;
    let connected_target_commits: Vec<Commit>;
    let connected_target_commits_internal_parents: HashMap<CommitId, Vec<CommitId>>;
    let target_roots: HashSet<CommitId>;

    match target {
        MoveCommitsTarget::Commits(commits) => {
            if commits.is_empty() {
                return Ok(MoveCommitsStats::default());
            }

            target_commits = commits.clone();
            target_commit_ids = target_commits.iter().ids().cloned().collect();

            connected_target_commits =
                RevsetExpression::commits(target_commits.iter().ids().cloned().collect_vec())
                    .connected()
                    .evaluate(mut_repo)
                    .map_err(|err| err.expect_backend_error())?
                    .iter()
                    .commits(mut_repo.store())
                    .try_collect()
                    // TODO: Return evaluation error to caller
                    .map_err(|err| err.expect_backend_error())?;
            connected_target_commits_internal_parents =
                compute_internal_parents_within(&target_commit_ids, &connected_target_commits);

            target_roots = connected_target_commits_internal_parents
                .iter()
                .filter(|(commit_id, parents)| {
                    target_commit_ids.contains(commit_id) && parents.is_empty()
                })
                .map(|(commit_id, _)| commit_id.clone())
                .collect();
        }
        MoveCommitsTarget::Roots(roots) => {
            if roots.is_empty() {
                return Ok(MoveCommitsStats::default());
            }

            target_commits = RevsetExpression::commits(roots.iter().ids().cloned().collect_vec())
                .descendants()
                .evaluate(mut_repo)
                .map_err(|err| err.expect_backend_error())?
                .iter()
                .commits(mut_repo.store())
                .try_collect()
                // TODO: Return evaluation error to caller
                .map_err(|err| err.expect_backend_error())?;
            target_commit_ids = target_commits.iter().ids().cloned().collect();

            connected_target_commits = target_commits.iter().cloned().collect_vec();
            // We don't have to compute the internal parents for the connected target set,
            // since the connected target set is the same as the target set.
            connected_target_commits_internal_parents = HashMap::new();
            target_roots = roots.iter().ids().cloned().collect();
        }
    }

    // If a commit outside the target set has a commit in the target set as a
    // parent, then - after the transformation - it should have that commit's
    // ancestors which are not in the target set as parents.
    let mut target_commits_external_parents: HashMap<CommitId, IndexSet<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = IndexSet::new();
        for old_parent in commit.parent_ids() {
            if let Some(parents) = target_commits_external_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            } else {
                new_parents.insert(old_parent.clone());
            }
        }
        target_commits_external_parents.insert(commit.id().clone(), new_parents);
    }

    // If the new parents include a commit in the target set, replace it with the
    // commit's ancestors which are outside the set.
    // e.g. `jj rebase -r A --before A`
    let new_parent_ids: Vec<_> = new_parent_ids
        .iter()
        .flat_map(|parent_id| {
            if let Some(parent_ids) = target_commits_external_parents.get(parent_id) {
                parent_ids.iter().cloned().collect_vec()
            } else {
                vec![parent_id.clone()]
            }
        })
        .collect();

    // If the new children include a commit in the target set, replace it with the
    // commit's descendants which are outside the set.
    // e.g. `jj rebase -r A --after A`
    let new_children: Vec<_> = if new_children
        .iter()
        .any(|child| target_commit_ids.contains(child.id()))
    {
        let target_commits_descendants: Vec<_> =
            RevsetExpression::commits(target_commit_ids.iter().cloned().collect_vec())
                .union(
                    &RevsetExpression::commits(target_commit_ids.iter().cloned().collect_vec())
                        .children(),
                )
                .evaluate(mut_repo)
                .map_err(|err| err.expect_backend_error())?
                .iter()
                .commits(mut_repo.store())
                .try_collect()
                // TODO: Return evaluation error to caller
                .map_err(|err| err.expect_backend_error())?;

        // For all commits in the target set, compute its transitive descendant commits
        // which are outside of the target set by up to 1 generation.
        let mut target_commit_external_descendants: HashMap<CommitId, IndexSet<Commit>> =
            HashMap::new();
        // Iterate through all descendants of the target set, going through children
        // before parents.
        for commit in &target_commits_descendants {
            if !target_commit_external_descendants.contains_key(commit.id()) {
                let children = if target_commit_ids.contains(commit.id()) {
                    IndexSet::new()
                } else {
                    IndexSet::from([commit.clone()])
                };
                target_commit_external_descendants.insert(commit.id().clone(), children);
            }

            let children = target_commit_external_descendants
                .get(commit.id())
                .unwrap()
                .iter()
                .cloned()
                .collect_vec();
            for parent_id in commit.parent_ids() {
                if target_commit_ids.contains(parent_id) {
                    if let Some(target_children) =
                        target_commit_external_descendants.get_mut(parent_id)
                    {
                        target_children.extend(children.iter().cloned());
                    } else {
                        target_commit_external_descendants
                            .insert(parent_id.clone(), children.iter().cloned().collect());
                    }
                };
            }
        }

        new_children
            .iter()
            .flat_map(|child| {
                if let Some(children) = target_commit_external_descendants.get(child.id()) {
                    children.iter().cloned().collect_vec()
                } else {
                    vec![child.clone()]
                }
            })
            .collect()
    } else {
        new_children.to_vec()
    };

    // Compute the parents of the new children, which will include the heads of the
    // target set.
    let new_children_parents: HashMap<_, _> = if !new_children.is_empty() {
        // Compute the heads of the target set, which will be used as the parents of
        // `new_children`.
        let target_heads = compute_commits_heads(&target_commit_ids, &connected_target_commits);

        new_children
            .iter()
            .map(|child_commit| {
                let mut new_child_parent_ids = IndexSet::new();
                for old_child_parent_id in child_commit.parent_ids() {
                    // Replace target commits with their parents outside the target set.
                    let old_child_parent_ids = if let Some(parents) =
                        target_commits_external_parents.get(old_child_parent_id)
                    {
                        parents.iter().collect_vec()
                    } else {
                        vec![old_child_parent_id]
                    };

                    // If the original parents of the new children are the new parents of the
                    // `target_heads`, replace them with the target heads since we are "inserting"
                    // the target commits in between the new parents and the new children.
                    for id in old_child_parent_ids {
                        if new_parent_ids
                            .iter()
                            .any(|new_parent_id| *new_parent_id == *id)
                        {
                            new_child_parent_ids.extend(target_heads.clone());
                        } else {
                            new_child_parent_ids.insert(id.clone());
                        };
                    }
                }

                // If not already present, add `target_heads` as parents of the new child
                // commit.
                new_child_parent_ids.extend(target_heads.clone());

                (
                    child_commit.id().clone(),
                    new_child_parent_ids.into_iter().collect_vec(),
                )
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Compute the set of commits to visit, which includes the target commits, the
    // new children commits (if any), and their descendants.
    let mut roots = target_roots.iter().cloned().collect_vec();
    roots.extend(new_children.iter().ids().cloned());
    let to_visit_expression = RevsetExpression::commits(roots).descendants();
    let to_visit: Vec<_> = to_visit_expression
        .evaluate(mut_repo)
        .map_err(|err| err.expect_backend_error())?
        .iter()
        .commits(mut_repo.store())
        .try_collect()
        // TODO: Return evaluation error to caller
        .map_err(|err| err.expect_backend_error())?;
    let to_visit_commits: IndexMap<_, _> = to_visit
        .into_iter()
        .map(|commit| (commit.id().clone(), commit))
        .collect();

    let to_visit_commits_new_parents: HashMap<_, _> = to_visit_commits
        .iter()
        .map(|(commit_id, commit)| {
            let new_parents =
            // New child of the rebased target commits.
            if let Some(new_child_parents) = new_children_parents.get(commit_id) {
                new_child_parents.clone()
            }
            // Commit is in the target set.
            else if target_commit_ids.contains(commit_id) {
                // If the commit is a root of the target set, it should be rebased onto the new destination.
                if target_roots.contains(commit_id) {
                    new_parent_ids.clone()
                }
                // Otherwise:
                // 1. Keep parents which are within the target set.
                // 2. Replace parents which are outside the target set but are part of the
                //    connected target set with their ancestor commits which are in the target
                //    set.
                // 3. Keep other parents outside the target set if they are not descendants of the
                //    new children of the target set.
                else {
                    let mut new_parents = vec![];
                    for parent_id in commit.parent_ids() {
                        if target_commit_ids.contains(parent_id) {
                            new_parents.push(parent_id.clone());
                        } else if let Some(parents) =
                                connected_target_commits_internal_parents.get(parent_id) {
                            new_parents.extend(parents.iter().cloned());
                        } else if !new_children.iter().any(|new_child| {
                                mut_repo.index().is_ancestor(new_child.id(), parent_id) }) {
                            new_parents.push(parent_id.clone());
                        }
                    }
                   new_parents
                }
            }
            // Commits outside the target set should have references to commits inside the set
            // replaced.
            else if commit
                .parent_ids()
                .iter()
                .any(|id| target_commits_external_parents.contains_key(id))
            {
                let mut new_parents = vec![];
                for parent in commit.parent_ids() {
                    if let Some(parents) = target_commits_external_parents.get(parent) {
                        new_parents.extend(parents.iter().cloned());
                    } else {
                        new_parents.push(parent.clone());
                    }
                }
                new_parents
            } else {
                commit.parent_ids().iter().cloned().collect_vec()
            };

            (commit_id.clone(), new_parents)
        })
        .collect();

    // Re-compute the order of commits to visit, such that each commit's new parents
    // must be visited first.
    let mut visited: HashSet<CommitId> = HashSet::new();
    let mut to_visit = dag_walk::topo_order_reverse(
        to_visit_commits.keys().cloned().collect_vec(),
        |commit_id| commit_id.clone(),
        |commit_id| -> Vec<CommitId> {
            visited.insert(commit_id.clone());
            to_visit_commits_new_parents
                .get(commit_id)
                .cloned()
                .unwrap()
                .iter()
                // Only add parents which are in the set to be visited and have not already been
                // visited.
                .filter(|&id| to_visit_commits.contains_key(id) && !visited.contains(id))
                .cloned()
                .collect()
        },
    );

    let mut num_rebased_targets = 0;
    let mut num_rebased_descendants = 0;
    let mut num_skipped_rebases = 0;
    let mut num_abandoned = 0;

    // Always keep empty commits when rebasing descendants.
    let rebase_descendant_options = &RebaseOptions {
        empty: EmptyBehaviour::Keep,
        simplify_ancestor_merge: options.simplify_ancestor_merge,
    };

    // Rebase each commit onto its new parents in the reverse topological order
    // computed above.
    while let Some(old_commit_id) = to_visit.pop() {
        let old_commit = to_visit_commits.get(&old_commit_id).unwrap();
        let parent_ids = to_visit_commits_new_parents.get(&old_commit_id).unwrap();
        let new_parent_ids = mut_repo.new_parents(parent_ids);
        let rewriter = CommitRewriter::new(mut_repo, old_commit.clone(), new_parent_ids);
        if rewriter.parents_changed() {
            let is_target_commit = target_commit_ids.contains(&old_commit_id);
            let rebased_commit = rebase_commit_with_options(
                settings,
                rewriter,
                if is_target_commit {
                    options
                } else {
                    rebase_descendant_options
                },
            )?;
            if let RebasedCommit::Abandoned { .. } = rebased_commit {
                num_abandoned += 1;
            } else if is_target_commit {
                num_rebased_targets += 1;
            } else {
                num_rebased_descendants += 1;
            }
        } else {
            num_skipped_rebases += 1;
        }
    }
    mut_repo.update_rewritten_references(settings)?;

    Ok(MoveCommitsStats {
        num_rebased_targets,
        num_rebased_descendants,
        num_skipped_rebases,
        num_abandoned,
    })
}

#[derive(Default)]
pub struct DuplicateCommitsStats {
    /// Map of original commit ID to newly duplicated commit.
    pub duplicated_commits: IndexMap<CommitId, Commit>,
    /// The number of descendant commits which were rebased onto the duplicated
    /// commits.
    pub num_rebased: u32,
}

/// Duplicates the given `target_commits` onto a new location in the graph.
///
/// The roots of `target_commits` are duplicated on top of the new
/// `parent_commit_ids`, whilst other commits in `target_commits` are duplicated
/// on top of the newly duplicated commits in the target set. If
/// `children_commit_ids` is not empty, the `children_commit_ids` will be
/// rebased onto the heads of the duplicated target commits.
///
/// This assumes that commits in `children_commit_ids` can be rewritten. There
/// should also be no cycles in the resulting graph, i.e. `children_commit_ids`
/// should not be ancestors of `parent_commit_ids`. Commits in `target_commits`
/// should be in reverse topological order (children before parents).
pub fn duplicate_commits(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    target_commits: &[CommitId],
    parent_commit_ids: &[CommitId],
    children_commit_ids: &[CommitId],
) -> BackendResult<DuplicateCommitsStats> {
    if target_commits.is_empty() {
        return Ok(DuplicateCommitsStats::default());
    }

    let mut duplicated_old_to_new: IndexMap<CommitId, Commit> = IndexMap::new();
    let mut num_rebased = 0;

    let target_commit_ids: HashSet<_> = target_commits.iter().cloned().collect();

    let connected_target_commits: Vec<_> =
        RevsetExpression::commits(target_commit_ids.iter().cloned().collect_vec())
            .connected()
            .evaluate(mut_repo)
            .map_err(|err| err.expect_backend_error())?
            .iter()
            .commits(mut_repo.store())
            .try_collect()
            // TODO: Return evaluation error to caller
            .map_err(|err| err.expect_backend_error())?;

    // Commits in the target set should only have other commits in the set as
    // parents, except the roots of the set, which persist their original
    // parents.
    // If a commit in the target set has a parent which is not in the set, but has
    // an ancestor which is in the set, then the commit will have that ancestor
    // as a parent instead.
    let target_commits_internal_parents = {
        let mut target_commits_internal_parents =
            compute_internal_parents_within(&target_commit_ids, &connected_target_commits);
        target_commits_internal_parents.retain(|id, _| target_commit_ids.contains(id));
        target_commits_internal_parents
    };

    // Compute the roots of `target_commits`.
    let target_root_ids: HashSet<_> = target_commits_internal_parents
        .iter()
        .filter(|(_, parents)| parents.is_empty())
        .map(|(commit_id, _)| commit_id.clone())
        .collect();

    // Compute the heads of the target set, which will be used as the parents of
    // the children commits.
    let target_head_ids = if !children_commit_ids.is_empty() {
        compute_commits_heads(&target_commit_ids, &connected_target_commits)
    } else {
        vec![]
    };

    // Topological order ensures that any parents of the original commit are
    // either not in `target_commits` or were already duplicated.
    for original_commit_id in target_commits.iter().rev() {
        let original_commit = mut_repo.store().get_commit(original_commit_id)?;
        let new_parent_ids = if target_root_ids.contains(original_commit_id) {
            parent_commit_ids.to_vec()
        } else {
            target_commits_internal_parents
                .get(original_commit_id)
                .unwrap()
                .iter()
                // Replace parent IDs with their new IDs if they were duplicated.
                .map(|id| {
                    duplicated_old_to_new
                        .get(id)
                        .map_or(id, |commit| commit.id())
                        .clone()
                })
                .collect()
        };
        let new_commit = CommitRewriter::new(mut_repo, original_commit, new_parent_ids)
            .rebase(settings)?
            .generate_new_change_id()
            .write()?;
        duplicated_old_to_new.insert(original_commit_id.clone(), new_commit);
    }

    // Replace the original commit IDs in `target_head_ids` with the duplicated
    // commit IDs.
    let target_head_ids = target_head_ids
        .into_iter()
        .map(|commit_id| {
            duplicated_old_to_new
                .get(&commit_id)
                .map_or(commit_id, |commit| commit.id().clone())
        })
        .collect_vec();

    // Rebase new children onto the target heads.
    let children_commit_ids_set: HashSet<CommitId> = children_commit_ids.iter().cloned().collect();
    mut_repo.transform_descendants(settings, children_commit_ids.to_vec(), |mut rewriter| {
        if children_commit_ids_set.contains(rewriter.old_commit().id()) {
            let mut child_new_parent_ids = IndexSet::new();
            for old_parent_id in rewriter.old_commit().parent_ids() {
                // If the original parents of the new children are the new parents of
                // `target_head_ids`, replace them with `target_head_ids` since we are
                // "inserting" the target commits in between the new parents and the new
                // children.
                if parent_commit_ids.contains(old_parent_id) {
                    child_new_parent_ids.extend(target_head_ids.clone());
                } else {
                    child_new_parent_ids.insert(old_parent_id.clone());
                }
            }
            // If not already present, add `target_head_ids` as parents of the new child
            // commit.
            child_new_parent_ids.extend(target_head_ids.clone());
            rewriter.set_new_parents(child_new_parent_ids.into_iter().collect());
        }
        num_rebased += 1;
        rewriter.rebase(settings)?.write()?;
        Ok(())
    })?;

    Ok(DuplicateCommitsStats {
        duplicated_commits: duplicated_old_to_new,
        num_rebased,
    })
}

/// Duplicates the given `target_commits` onto their original parents or other
/// duplicated commits.
///
/// Commits in `target_commits` should be in reverse topological order (children
/// before parents).
pub fn duplicate_commits_onto_parents(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    target_commits: &[CommitId],
) -> BackendResult<DuplicateCommitsStats> {
    if target_commits.is_empty() {
        return Ok(DuplicateCommitsStats::default());
    }

    let mut duplicated_old_to_new: IndexMap<CommitId, Commit> = IndexMap::new();

    // Topological order ensures that any parents of the original commit are
    // either not in `target_commits` or were already duplicated.
    for original_commit_id in target_commits.iter().rev() {
        let original_commit = mut_repo.store().get_commit(original_commit_id)?;
        let new_parent_ids = original_commit
            .parent_ids()
            .iter()
            .map(|id| {
                duplicated_old_to_new
                    .get(id)
                    .map_or(id, |commit| commit.id())
                    .clone()
            })
            .collect();
        let new_commit = mut_repo
            .rewrite_commit(settings, &original_commit)
            .generate_new_change_id()
            .set_parents(new_parent_ids)
            .write()?;
        duplicated_old_to_new.insert(original_commit_id.clone(), new_commit);
    }

    Ok(DuplicateCommitsStats {
        duplicated_commits: duplicated_old_to_new,
        num_rebased: 0,
    })
}

/// Computes the internal parents of all commits in a connected commit graph,
/// allowing only commits in the target set as parents.
///
/// The parents of each commit are identical to the ones found using a preorder
/// DFS of the node's ancestors, starting from the node itself, and avoiding
/// traversing an edge if the parent is in the target set. `graph_commits`
/// should be in reverse topological order.
fn compute_internal_parents_within(
    target_commit_ids: &HashSet<CommitId>,
    graph_commits: &[Commit],
) -> HashMap<CommitId, Vec<CommitId>> {
    let mut internal_parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
    for commit in graph_commits.iter().rev() {
        // The roots of the set will not have any parents found in `internal_parents`,
        // and will be stored as an empty vector.
        let mut new_parents = vec![];
        for old_parent in commit.parent_ids() {
            if target_commit_ids.contains(old_parent) {
                new_parents.push(old_parent.clone());
            } else if let Some(parents) = internal_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            }
        }
        internal_parents.insert(commit.id().clone(), new_parents);
    }
    internal_parents
}

/// Computes the heads of commits in the target set, given the list of
/// `target_commit_ids` and a connected graph of commits.
///
/// `connected_target_commits` should be in reverse topological order (children
/// before parents).
fn compute_commits_heads(
    target_commit_ids: &HashSet<CommitId>,
    connected_target_commits: &[Commit],
) -> Vec<CommitId> {
    let mut target_head_ids: HashSet<CommitId> = HashSet::new();
    for commit in connected_target_commits.iter().rev() {
        target_head_ids.insert(commit.id().clone());
        for old_parent in commit.parent_ids() {
            target_head_ids.remove(old_parent);
        }
    }
    connected_target_commits
        .iter()
        .rev()
        .filter(|commit| {
            target_head_ids.contains(commit.id()) && target_commit_ids.contains(commit.id())
        })
        .map(|commit| commit.id().clone())
        .collect_vec()
}

pub struct CommitToSquash {
    pub commit: Commit,
    pub selected_tree: MergedTree,
    pub parent_tree: MergedTree,
}

impl CommitToSquash {
    /// Returns true if the selection contains all changes in the commit.
    fn is_full_selection(&self) -> bool {
        &self.selected_tree.id() == self.commit.tree_id()
    }

    /// Returns true if the selection matches the parent tree (contains no
    /// changes from the commit).
    ///
    /// Both `is_full_selection()` and `is_empty_selection()`
    /// can be true if the commit is itself empty.
    fn is_empty_selection(&self) -> bool {
        self.selected_tree.id() == self.parent_tree.id()
    }
}

#[derive(Clone, Debug)]
pub enum SquashResult {
    /// No inputs contained actual changes.
    NoChanges,
    /// Destination was rewritten.
    NewCommit(Commit),
}

/// Squash `sources` into `destination` and return a CommitBuilder for the
/// resulting commit. Caller is responsible for setting the description and
/// finishing the commit.
pub fn squash_commits<E>(
    settings: &UserSettings,
    repo: &mut MutableRepo,
    sources: &[CommitToSquash],
    destination: &Commit,
    keep_emptied: bool,
    description_fn: impl FnOnce(&[&CommitToSquash]) -> Result<String, E>,
) -> Result<SquashResult, E>
where
    E: From<BackendError>,
{
    struct SourceCommit<'a> {
        commit: &'a CommitToSquash,
        abandon: bool,
    }
    let mut source_commits = vec![];
    for source in sources {
        let abandon = !keep_emptied && source.is_full_selection();
        if !abandon && source.is_empty_selection() {
            // Nothing selected from this commit. If it's abandoned (i.e. already empty), we
            // still include it so `jj squash` can be used for abandoning an empty commit in
            // the middle of a stack.
            continue;
        }

        // TODO: Do we want to optimize the case of moving to the parent commit (`jj
        // squash -r`)? The source tree will be unchanged in that case.
        source_commits.push(SourceCommit {
            commit: source,
            abandon,
        });
    }

    if source_commits.is_empty() {
        return Ok(SquashResult::NoChanges);
    }

    let mut abandoned_commits = vec![];
    for source in &source_commits {
        if source.abandon {
            repo.record_abandoned_commit(source.commit.commit.id().clone());
            abandoned_commits.push(source.commit);
        } else {
            let source_tree = source.commit.commit.tree()?;
            // Apply the reverse of the selected changes onto the source
            let new_source_tree =
                source_tree.merge(&source.commit.selected_tree, &source.commit.parent_tree)?;
            repo.rewrite_commit(settings, &source.commit.commit)
                .set_tree_id(new_source_tree.id().clone())
                .write()?;
        }
    }

    let mut rewritten_destination = destination.clone();
    if sources.iter().any(|source| {
        repo.index()
            .is_ancestor(source.commit.id(), destination.id())
    }) {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten sources. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let rebase_map =
            repo.rebase_descendants_with_options_return_map(settings, Default::default())?;
        let rebased_destination_id = rebase_map.get(destination.id()).unwrap().clone();
        rewritten_destination = repo.store().get_commit(&rebased_destination_id)?;
    }
    // Apply the selected changes onto the destination
    let mut destination_tree = rewritten_destination.tree()?;
    for source in &source_commits {
        destination_tree =
            destination_tree.merge(&source.commit.parent_tree, &source.commit.selected_tree)?;
    }
    let mut predecessors = vec![destination.id().clone()];
    predecessors.extend(
        source_commits
            .iter()
            .map(|source| source.commit.commit.id().clone()),
    );

    let destination = repo
        .rewrite_commit(settings, &rewritten_destination)
        .set_tree_id(destination_tree.id().clone())
        .set_predecessors(predecessors)
        .set_description(description_fn(&abandoned_commits)?)
        .write()?;

    Ok(SquashResult::NewCommit(destination))
}
