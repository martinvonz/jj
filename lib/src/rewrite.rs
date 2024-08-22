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
use crate::revset::RevsetEvaluationError;
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
    pub fn set_new_rewritten_parents(&mut self, unrewritten_parents: Vec<CommitId>) {
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
            let old_base_tree = merge_commit_trees(self.mut_repo, &old_parents)?;
            let new_base_tree = merge_commit_trees(self.mut_repo, &new_parents)?;
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
    Abandoned { parent: Commit },
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

    // TODO: avoid this lookup by not returning the old parent for
    // RebasedCommit::Abandoned
    let store = rewriter.mut_repo().store().clone();
    let single_parent = match &rewriter.new_parents[..] {
        [parent] => Some(store.get_commit(parent)?),
        _ => None,
    };
    let new_parents = rewriter.new_parents.clone();
    if let Some(builder) = rewriter.rebase_with_empty_behavior(settings, options.empty)? {
        let new_commit = builder.write()?;
        Ok(RebasedCommit::Rewritten(new_commit))
    } else {
        assert_eq!(new_parents.len(), 1);
        Ok(RebasedCommit::Abandoned {
            parent: single_parent.unwrap(),
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

pub(crate) struct DescendantRebaser<'settings, 'repo> {
    settings: &'settings UserSettings,
    mut_repo: &'repo mut MutableRepo,
    // In reverse order (parents after children), so we can remove the last one to rebase first.
    to_visit: Vec<Commit>,
    rebased: HashMap<CommitId, CommitId>,
    // Options to apply during a rebase.
    options: RebaseOptions,
}

impl<'settings, 'repo> DescendantRebaser<'settings, 'repo> {
    /// Panics if any commit is rewritten to its own descendant.
    ///
    /// There should not be any cycles in the `rewritten` map (e.g. A is
    /// rewritten to B, which is rewritten to A). The same commit should not
    /// be rewritten and abandoned at the same time. In either case, panics are
    /// likely when using the DescendantRebaser.
    pub fn new(
        settings: &'settings UserSettings,
        mut_repo: &'repo mut MutableRepo,
        to_visit: Vec<Commit>,
    ) -> DescendantRebaser<'settings, 'repo> {
        DescendantRebaser {
            settings,
            mut_repo,
            to_visit,
            rebased: Default::default(),
            options: Default::default(),
        }
    }

    /// Returns options that can be set.
    pub fn mut_options(&mut self) -> &mut RebaseOptions {
        &mut self.options
    }

    /// Returns a map from `CommitId` of old commit to new commit. Includes the
    /// commits rebase so far. Does not include the inputs passed to
    /// `rebase_descendants`.
    pub fn into_map(self) -> HashMap<CommitId, CommitId> {
        self.rebased
    }

    fn rebase_one(&mut self, old_commit: Commit) -> BackendResult<()> {
        let old_commit_id = old_commit.id().clone();
        let old_parent_ids = old_commit.parent_ids();
        let new_parent_ids = self.mut_repo.new_parents(old_parent_ids.to_vec());
        let rewriter = CommitRewriter::new(self.mut_repo, old_commit, new_parent_ids);
        if !rewriter.parents_changed() {
            // The commit is already in place.
            return Ok(());
        }

        let rebased_commit: RebasedCommit =
            rebase_commit_with_options(self.settings, rewriter, &self.options)?;
        let new_commit = match rebased_commit {
            RebasedCommit::Rewritten(new_commit) => new_commit,
            RebasedCommit::Abandoned { parent } => parent,
        };
        self.rebased
            .insert(old_commit_id.clone(), new_commit.id().clone());
        Ok(())
    }

    pub fn rebase_all(&mut self) -> BackendResult<()> {
        while let Some(old_commit) = self.to_visit.pop() {
            self.rebase_one(old_commit)?;
        }
        self.mut_repo.update_rewritten_references(self.settings)
    }
}

pub struct MoveCommitsStats {
    /// The number of commits in the target set which were rebased.
    pub num_rebased_targets: u32,
    /// The number of descendant commits which were rebased.
    pub num_rebased_descendants: u32,
    /// The number of commits for which rebase was skipped, due to the commit
    /// already being in place.
    pub num_skipped_rebases: u32,
}

/// Moves `target_commits` from their current location to a new location in the
/// graph, given by the set of `new_parent_ids` and `new_children`.
/// The roots of `target_commits` are rebased onto the new parents, while the
/// new children are rebased onto the heads of `target_commits`.
/// This assumes that `target_commits` and `new_children` can be rewritten, and
/// there will be no cycles in the resulting graph.
/// `target_commits` should be in reverse topological order.
pub fn move_commits(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    new_parent_ids: &[CommitId],
    new_children: &[Commit],
    target_commits: &[Commit],
) -> BackendResult<MoveCommitsStats> {
    if target_commits.is_empty() {
        return Ok(MoveCommitsStats {
            num_rebased_targets: 0,
            num_rebased_descendants: 0,
            num_skipped_rebases: 0,
        });
    }

    let target_commit_ids: HashSet<_> = target_commits.iter().ids().cloned().collect();

    let connected_target_commits: Vec<_> =
        RevsetExpression::commits(target_commits.iter().ids().cloned().collect_vec())
            .connected()
            .evaluate_programmatic(mut_repo)
            .map_err(|err| match err {
                RevsetEvaluationError::StoreError(err) => err,
                RevsetEvaluationError::Other(_) => panic!("Unexpected revset error: {err}"),
            })?
            .iter()
            .commits(mut_repo.store())
            .try_collect()?;

    // Compute the parents of all commits in the connected target set, allowing only
    // commits in the target set as parents. The parents of each commit are
    // identical to the ones found using a preorder DFS of the node's ancestors,
    // starting from the node itself, and avoiding traversing an edge if the
    // parent is in the target set.
    let mut connected_target_commits_internal_parents: HashMap<CommitId, Vec<CommitId>> =
        HashMap::new();
    for commit in connected_target_commits.iter().rev() {
        // The roots of the set will not have any parents found in
        // `connected_target_commits_internal_parents`, and will be stored as an empty
        // vector.
        let mut new_parents = vec![];
        for old_parent in commit.parent_ids() {
            if target_commit_ids.contains(old_parent) {
                new_parents.push(old_parent.clone());
            } else if let Some(parents) = connected_target_commits_internal_parents.get(old_parent)
            {
                new_parents.extend(parents.iter().cloned());
            }
        }
        connected_target_commits_internal_parents.insert(commit.id().clone(), new_parents);
    }

    // Compute the roots of `target_commits`.
    let target_roots: HashSet<_> = connected_target_commits_internal_parents
        .iter()
        .filter(|(commit_id, parents)| target_commit_ids.contains(commit_id) && parents.is_empty())
        .map(|(commit_id, _)| commit_id.clone())
        .collect();

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
                [parent_id.clone()].to_vec()
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
                .evaluate_programmatic(mut_repo)
                .map_err(|err| match err {
                    RevsetEvaluationError::StoreError(err) => err,
                    RevsetEvaluationError::Other(_) => panic!("Unexpected revset error: {err}"),
                })?
                .iter()
                .commits(mut_repo.store())
                .try_collect()?;

        // For all commits in the target set, compute its transitive descendant commits
        // which are outside of the target set by up to 1 generation.
        let mut target_commit_external_descendants: HashMap<CommitId, IndexSet<Commit>> =
            HashMap::new();
        // Iterate through all descendants of the target set, going through children
        // before parents.
        for commit in target_commits_descendants.iter() {
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
                    [child.clone()].to_vec()
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
        let mut target_heads: HashSet<CommitId> = HashSet::new();
        for commit in connected_target_commits.iter().rev() {
            target_heads.insert(commit.id().clone());
            for old_parent in commit.parent_ids() {
                target_heads.remove(old_parent);
            }
        }
        let target_heads = connected_target_commits
            .iter()
            .rev()
            .filter(|commit| {
                target_heads.contains(commit.id()) && target_commit_ids.contains(commit.id())
            })
            .map(|commit| commit.id().clone())
            .collect_vec();

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
        .evaluate_programmatic(mut_repo)
        .map_err(|err| match err {
            RevsetEvaluationError::StoreError(err) => err,
            RevsetEvaluationError::Other(_) => panic!("Unexpected revset error: {err}"),
        })?
        .iter()
        .commits(mut_repo.store())
        .try_collect()?;
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

    // Rebase each commit onto its new parents in the reverse topological order
    // computed above.
    // TODO(ilyagr): Consider making it possible for descendants of the target set
    // to become emptied, like --skip-empty. This would require writing careful
    // tests.
    while let Some(old_commit_id) = to_visit.pop() {
        let old_commit = to_visit_commits.get(&old_commit_id).unwrap();
        let parent_ids = to_visit_commits_new_parents
            .get(&old_commit_id)
            .cloned()
            .unwrap();
        let new_parent_ids = mut_repo.new_parents(parent_ids);
        let rewriter = CommitRewriter::new(mut_repo, old_commit.clone(), new_parent_ids);
        if rewriter.parents_changed() {
            rewriter.rebase(settings)?.write()?;
            if target_commit_ids.contains(&old_commit_id) {
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
    })
}
