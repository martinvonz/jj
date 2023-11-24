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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::StreamExt;
use itertools::Itertools;
use pollster::FutureExt;
use tracing::instrument;

use crate::backend::{BackendError, BackendResult, CommitId, MergedTreeId, ObjectId};
use crate::commit::Commit;
use crate::dag_walk;
use crate::index::Index;
use crate::matchers::{Matcher, Visit};
use crate::merged_tree::{MergedTree, MergedTreeBuilder};
use crate::op_store::RefTarget;
use crate::repo::{MutableRepo, Repo};
use crate::repo_path::RepoPath;
use crate::revset::{RevsetExpression, RevsetIteratorExt};
use crate::settings::UserSettings;
use crate::store::Store;
use crate::tree::TreeMergeError;

#[instrument(skip(repo))]
pub fn merge_commit_trees(
    repo: &dyn Repo,
    commits: &[Commit],
) -> Result<MergedTree, TreeMergeError> {
    merge_commit_trees_without_repo(repo.store(), repo.index(), commits)
}

#[instrument(skip(index))]
pub fn merge_commit_trees_without_repo(
    store: &Arc<Store>,
    index: &dyn Index,
    commits: &[Commit],
) -> Result<MergedTree, TreeMergeError> {
    if commits.is_empty() {
        Ok(store.get_root_tree(&store.empty_merged_tree_id())?)
    } else {
        let mut new_tree = commits[0].tree()?;
        let commit_ids = commits
            .iter()
            .map(|commit| commit.id().clone())
            .collect_vec();
        for (i, other_commit) in commits.iter().enumerate().skip(1) {
            let ancestor_ids = index.common_ancestors(&commit_ids[0..i], &[commit_ids[i].clone()]);
            let ancestors: Vec<_> = ancestor_ids
                .iter()
                .map(|id| store.get_commit(id))
                .try_collect()?;
            let ancestor_tree = merge_commit_trees_without_repo(store, index, &ancestors)?;
            let other_tree = other_commit.tree()?;
            new_tree = new_tree.merge(&ancestor_tree, &other_tree)?;
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
    if matcher.visit(&RepoPath::root()) == Visit::AllRecursively {
        // Optimization for a common case
        Ok(source.id())
    } else {
        // TODO: We should be able to not traverse deeper in the diff if the matcher
        // matches an entire subtree.
        let mut tree_builder = MergedTreeBuilder::new(destination.id().clone());
        async {
            let mut diff_stream = source.diff_stream(destination, matcher);
            while let Some((repo_path, diff)) = diff_stream.next().await {
                let (source_value, _destination_value) = diff?;
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
    old_commit: &Commit,
    new_parents: &[Commit],
) -> Result<Commit, TreeMergeError> {
    rebase_commit_with_options(
        settings,
        mut_repo,
        old_commit,
        new_parents,
        &Default::default(),
    )
}

pub fn rebase_commit_with_options(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    old_commit: &Commit,
    new_parents: &[Commit],
    options: &RebaseOptions,
) -> Result<Commit, TreeMergeError> {
    let old_parents = old_commit.parents();
    let old_parent_trees = old_parents
        .iter()
        .map(|parent| parent.store_commit().root_tree.clone())
        .collect_vec();
    let new_parent_trees = new_parents
        .iter()
        .map(|parent| parent.store_commit().root_tree.clone())
        .collect_vec();

    let (old_base_tree_id, new_tree_id) = if new_parent_trees == old_parent_trees {
        (
            // Optimization: old_base_tree_id is only used for newly empty, but when the parents
            // haven't changed it can't be newly empty.
            None,
            // Optimization: Skip merging.
            old_commit.tree_id().clone(),
        )
    } else {
        let old_base_tree = merge_commit_trees(mut_repo, &old_parents)?;
        let new_base_tree = merge_commit_trees(mut_repo, new_parents)?;
        let old_tree = old_commit.tree()?;
        (
            Some(old_base_tree.id()),
            new_base_tree.merge(&old_base_tree, &old_tree)?.id(),
        )
    };
    // Ensure we don't abandon commits with multiple parents (merge commits), even
    // if they're empty.
    if let [parent] = new_parents {
        match options.empty {
            EmptyBehaviour::AbandonNewlyEmpty | EmptyBehaviour::AbandonAllEmpty => {
                if *parent.tree_id() == new_tree_id
                    && (options.empty == EmptyBehaviour::AbandonAllEmpty
                        || old_base_tree_id != Some(old_commit.tree_id().clone()))
                {
                    mut_repo.record_abandoned_commit(old_commit.id().clone());
                    // Record old_commit as being succeeded by the parent for the purposes of
                    // the rebase.
                    // This ensures that when we stack commits, the second commit knows to
                    // rebase on top of the parent commit, rather than the abandoned commit.
                    return Ok(parent.clone());
                }
            }
            EmptyBehaviour::Keep => {}
        }
    }
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    Ok(mut_repo
        .rewrite_commit(settings, old_commit)
        .set_parents(new_parent_ids)
        .set_tree_id(new_tree_id)
        .write()?)
}

pub fn rebase_to_dest_parent(
    repo: &dyn Repo,
    source: &Commit,
    destination: &Commit,
) -> Result<MergedTree, TreeMergeError> {
    if source.parent_ids() == destination.parent_ids() {
        Ok(source.tree()?)
    } else {
        let destination_parent_tree = merge_commit_trees(repo, &destination.parents())?;
        let source_parent_tree = merge_commit_trees(repo, &source.parents())?;
        let source_tree = source.tree()?;
        let rebased_tree = destination_parent_tree.merge(&source_parent_tree, &source_tree)?;
        Ok(rebased_tree)
    }
}

pub fn back_out_commit(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    old_commit: &Commit,
    new_parents: &[Commit],
) -> Result<Commit, TreeMergeError> {
    let old_base_tree = merge_commit_trees(mut_repo, &old_commit.parents())?;
    let new_base_tree = merge_commit_trees(mut_repo, new_parents)?;
    let old_tree = old_commit.tree()?;
    let new_tree = new_base_tree.merge(&old_tree, &old_base_tree)?;
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    // TODO: i18n the description based on repo language
    Ok(mut_repo
        .new_commit(settings, new_parent_ids, new_tree.id())
        .set_description(format!("backout of commit {}", &old_commit.id().hex()))
        .write()?)
}

#[derive(Clone, Default, PartialEq)]
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
#[derive(Clone, Default)]
pub struct RebaseOptions {
    pub empty: EmptyBehaviour,
}

/// Rebases descendants of a commit onto a new commit (or several).
pub struct DescendantRebaser<'settings, 'repo> {
    settings: &'settings UserSettings,
    mut_repo: &'repo mut MutableRepo,
    // The commit identified by the key has been replaced by all the ones in the value, typically
    // because the key commit was abandoned (the value commits are then the abandoned commit's
    // parents). A child of the key commit should be rebased onto all the value commits. A branch
    // pointing to the key commit should become a conflict pointing to all the value commits.
    new_parents: HashMap<CommitId, Vec<CommitId>>,
    divergent: HashMap<CommitId, Vec<CommitId>>,
    // In reverse order (parents after children), so we can remove the last one to rebase first.
    to_visit: Vec<Commit>,
    // Commits to visit but skip. These were also in `to_visit` to start with, but we don't
    // want to rebase them. Instead, we record them in `replacements` when we visit them. That way,
    // their descendants will be rebased correctly.
    abandoned: HashSet<CommitId>,
    new_commits: HashSet<CommitId>,
    rebased: HashMap<CommitId, CommitId>,
    // Names of branches where local target includes the commit id in the key.
    branches: HashMap<CommitId, HashSet<String>>,
    // Parents of rebased/abandoned commit that should become new heads once their descendants
    // have been rebased.
    heads_to_add: HashSet<CommitId>,
    heads_to_remove: Vec<CommitId>,

    // Options to apply during a rebase.
    options: RebaseOptions,
}

impl<'settings, 'repo> DescendantRebaser<'settings, 'repo> {
    pub fn new(
        settings: &'settings UserSettings,
        mut_repo: &'repo mut MutableRepo,
        rewritten: HashMap<CommitId, HashSet<CommitId>>,
        abandoned: HashSet<CommitId>,
    ) -> DescendantRebaser<'settings, 'repo> {
        let store = mut_repo.store();
        let root_commit_id = store.root_commit_id();
        assert!(!abandoned.contains(root_commit_id));
        assert!(!rewritten.contains_key(root_commit_id));
        let old_commits_expression = RevsetExpression::commits(rewritten.keys().cloned().collect())
            .union(&RevsetExpression::commits(
                abandoned.iter().cloned().collect(),
            ));
        let heads_to_add_expression = old_commits_expression
            .parents()
            .minus(&old_commits_expression);
        let heads_to_add = heads_to_add_expression
            .resolve_programmatic(mut_repo)
            .unwrap()
            .evaluate(mut_repo)
            .unwrap()
            .iter()
            .collect();

        let to_visit_expression = old_commits_expression.descendants();
        let to_visit_revset = to_visit_expression
            .resolve_programmatic(mut_repo)
            .unwrap()
            .evaluate(mut_repo)
            .unwrap();
        let to_visit: Vec<_> = to_visit_revset.iter().commits(store).try_collect().unwrap();
        drop(to_visit_revset);
        let to_visit_set: HashSet<CommitId> =
            to_visit.iter().map(|commit| commit.id().clone()).collect();
        let mut visited = HashSet::new();
        // Calculate an order where we rebase parents first, but if the parents were
        // rewritten, make sure we rebase the rewritten parent first.
        let to_visit = dag_walk::topo_order_reverse(
            to_visit,
            |commit| commit.id().clone(),
            |commit| {
                visited.insert(commit.id().clone());
                let mut dependents = vec![];
                for parent in commit.parents() {
                    if let Some(targets) = rewritten.get(parent.id()) {
                        for target in targets {
                            if to_visit_set.contains(target) && !visited.contains(target) {
                                dependents.push(store.get_commit(target).unwrap());
                            }
                        }
                    }
                    if to_visit_set.contains(parent.id()) {
                        dependents.push(parent);
                    }
                }
                dependents
            },
        );

        let new_commits = rewritten.values().flatten().cloned().collect();

        let mut new_parents = HashMap::new();
        let mut divergent = HashMap::new();
        for (old_commit, new_commits) in rewritten {
            if new_commits.len() == 1 {
                new_parents.insert(old_commit, vec![new_commits.iter().next().unwrap().clone()]);
            } else {
                // The call to index.heads() is mostly to get a predictable order
                let new_commits = mut_repo.index().heads(&mut new_commits.iter());
                divergent.insert(old_commit, new_commits);
            }
        }

        // Build a map from commit to branches pointing to it, so we don't need to scan
        // all branches each time we rebase a commit.
        let mut branches: HashMap<_, HashSet<_>> = HashMap::new();
        for (branch_name, target) in mut_repo.view().local_branches() {
            for commit in target.added_ids() {
                branches
                    .entry(commit.clone())
                    .or_default()
                    .insert(branch_name.to_owned());
            }
        }

        DescendantRebaser {
            settings,
            mut_repo,
            new_parents,
            divergent,
            to_visit,
            abandoned,
            new_commits,
            rebased: Default::default(),
            branches,
            heads_to_add,
            heads_to_remove: Default::default(),
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
    pub fn rebased(&self) -> &HashMap<CommitId, CommitId> {
        &self.rebased
    }

    fn new_parents(&self, old_ids: &[CommitId]) -> Vec<CommitId> {
        // This should be a set, but performance of a vec is much better since we expect
        // 99% of commits to have <= 2 parents.
        let mut new_ids = vec![];
        let mut add_parent = |id: &CommitId| {
            // This can trigger if we abandon an empty commit, as both the empty commit and
            // its parent are succeeded by the same commit.
            if !new_ids.contains(id) {
                new_ids.push(id.clone());
            }
        };
        for old_id in old_ids {
            if let Some(new_parent_ids) = self.new_parents.get(old_id) {
                for new_parent_id in new_parent_ids {
                    // The new parent may itself have been rebased earlier in the process
                    if let Some(newer_parent_id) = self.rebased.get(new_parent_id) {
                        add_parent(newer_parent_id);
                    } else {
                        add_parent(new_parent_id);
                    }
                }
            } else if let Some(new_parent_id) = self.rebased.get(old_id) {
                add_parent(new_parent_id);
            } else {
                add_parent(old_id);
            };
        }
        new_ids
    }

    fn ref_target_update(old_id: CommitId, new_ids: Vec<CommitId>) -> (RefTarget, RefTarget) {
        let old_ids = std::iter::repeat(old_id).take(new_ids.len());
        (
            RefTarget::from_legacy_form([], old_ids),
            RefTarget::from_legacy_form([], new_ids),
        )
    }

    fn update_references(
        &mut self,
        old_commit_id: CommitId,
        new_commit_ids: Vec<CommitId>,
        edit: bool,
    ) -> Result<(), BackendError> {
        // We arbitrarily pick a new working-copy commit among the candidates.
        self.update_wc_commits(&old_commit_id, &new_commit_ids[0], edit)?;

        if let Some(branch_names) = self.branches.get(&old_commit_id).cloned() {
            let mut branch_updates = vec![];
            for branch_name in &branch_names {
                for new_commit_id in &new_commit_ids {
                    self.branches
                        .entry(new_commit_id.clone())
                        .or_default()
                        .insert(branch_name.clone());
                }
                let local_target = self.mut_repo.get_local_branch(branch_name);
                for old_add in local_target.added_ids() {
                    if *old_add == old_commit_id {
                        branch_updates.push(branch_name.clone());
                    }
                }
            }
            let (old_target, new_target) =
                DescendantRebaser::ref_target_update(old_commit_id.clone(), new_commit_ids);
            for branch_name in &branch_updates {
                self.mut_repo
                    .merge_local_branch(branch_name, &old_target, &new_target);
            }
        }

        self.heads_to_add.remove(&old_commit_id);
        if !self.new_commits.contains(&old_commit_id) || self.rebased.contains_key(&old_commit_id) {
            self.heads_to_remove.push(old_commit_id);
        }
        Ok(())
    }

    fn update_wc_commits(
        &mut self,
        old_commit_id: &CommitId,
        new_commit_id: &CommitId,
        edit: bool,
    ) -> Result<(), BackendError> {
        let workspaces_to_update = self
            .mut_repo
            .view()
            .workspaces_for_wc_commit_id(old_commit_id);
        if workspaces_to_update.is_empty() {
            return Ok(());
        }

        let new_commit = self.mut_repo.store().get_commit(new_commit_id)?;
        let new_wc_commit = if edit {
            new_commit
        } else {
            self.mut_repo
                .new_commit(
                    self.settings,
                    vec![new_commit.id().clone()],
                    new_commit.tree_id().clone(),
                )
                .write()?
        };
        for workspace_id in workspaces_to_update.into_iter() {
            self.mut_repo.edit(workspace_id, &new_wc_commit).unwrap();
        }
        Ok(())
    }

    // TODO: Perhaps change the interface since it's not just about rebasing
    // commits.
    pub fn rebase_next(&mut self) -> Result<Option<RebasedDescendant>, TreeMergeError> {
        while let Some(old_commit) = self.to_visit.pop() {
            let old_commit_id = old_commit.id().clone();
            if let Some(new_parent_ids) = self.new_parents.get(&old_commit_id).cloned() {
                // This is a commit that had already been rebased before `self` was created
                // (i.e. it's part of the input for this rebase). We don't need
                // to rebase it, but we still want to update branches pointing
                // to the old commit.
                self.update_references(old_commit_id, new_parent_ids, true)?;
                continue;
            }
            if let Some(divergent_ids) = self.divergent.get(&old_commit_id).cloned() {
                // Leave divergent commits in place. Don't update `new_parents` since we don't
                // want to rebase descendants either.
                self.update_references(old_commit_id, divergent_ids, true)?;
                continue;
            }
            let old_parent_ids = old_commit.parent_ids();
            let new_parent_ids = self.new_parents(old_parent_ids);
            if self.abandoned.contains(&old_commit_id) {
                // Update the `new_parents` map so descendants are rebased correctly.
                self.new_parents
                    .insert(old_commit_id.clone(), new_parent_ids.clone());
                self.update_references(old_commit_id, new_parent_ids, false)?;
                continue;
            } else if new_parent_ids == old_parent_ids {
                // The commit is already in place.
                continue;
            }

            // Don't create commit where one parent is an ancestor of another.
            let head_set: HashSet<_> = self
                .mut_repo
                .index()
                .heads(&mut new_parent_ids.iter())
                .into_iter()
                .collect();
            let new_parents: Vec<_> = new_parent_ids
                .iter()
                .filter(|new_parent| head_set.contains(new_parent))
                .map(|new_parent_id| self.mut_repo.store().get_commit(new_parent_id))
                .try_collect()?;
            let new_commit = rebase_commit_with_options(
                self.settings,
                self.mut_repo,
                &old_commit,
                &new_parents,
                &self.options,
            )?;
            self.rebased
                .insert(old_commit_id.clone(), new_commit.id().clone());
            self.update_references(old_commit_id, vec![new_commit.id().clone()], true)?;
            return Ok(Some(RebasedDescendant {
                old_commit,
                new_commit,
            }));
        }
        // TODO: As the TODO above says, we should probably change the API. Even if we
        // don't, we should at least make this code not do any work if you call
        // rebase_next() after we've returned None.
        let mut view = self.mut_repo.view().store_view().clone();
        for commit_id in &self.heads_to_remove {
            view.head_ids.remove(commit_id);
        }
        for commit_id in &self.heads_to_add {
            view.head_ids.insert(commit_id.clone());
        }
        self.heads_to_remove.clear();
        self.heads_to_add.clear();
        self.mut_repo.set_view(view);
        self.mut_repo.clear_rewritten_commits();
        self.mut_repo.clear_abandoned_commits();
        Ok(None)
    }

    pub fn rebase_all(&mut self) -> Result<(), TreeMergeError> {
        while self.rebase_next()?.is_some() {}
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RebasedDescendant {
    pub old_commit: Commit,
    pub new_commit: Commit,
}
