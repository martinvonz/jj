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

use crate::backend::{BackendError, BackendResult, CommitId, MergedTreeId};
use crate::commit::Commit;
use crate::dag_walk;
use crate::index::Index;
use crate::matchers::{Matcher, Visit};
use crate::merged_tree::{MergedTree, MergedTreeBuilder};
use crate::object_id::ObjectId;
use crate::op_store::RefTarget;
use crate::repo::{MutableRepo, Repo, RewriteType};
use crate::repo_path::RepoPath;
use crate::revset::{RevsetExpression, RevsetIteratorExt};
use crate::settings::UserSettings;
use crate::store::Store;

#[instrument(skip(repo))]
pub fn merge_commit_trees(repo: &dyn Repo, commits: &[Commit]) -> BackendResult<MergedTree> {
    merge_commit_trees_without_repo(repo.store(), repo.index(), commits)
}

#[instrument(skip(index))]
pub fn merge_commit_trees_without_repo(
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
    if matcher.visit(RepoPath::root()) == Visit::AllRecursively {
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
) -> BackendResult<Commit> {
    let rebased_commit = rebase_commit_with_options(
        settings,
        mut_repo,
        old_commit,
        new_parents,
        &Default::default(),
    )?;
    match rebased_commit {
        RebasedCommit::Rewritten(new_commit) => Ok(new_commit),
        RebasedCommit::Abandoned { parent: _ } => panic!("Commit was unexpectedly abandoned"),
    }
}

pub enum RebasedCommit {
    Rewritten(Commit),
    Abandoned { parent: Commit },
}

pub fn rebase_commit_with_options(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    old_commit: &Commit,
    new_parents: &[Commit],
    options: &RebaseOptions,
) -> BackendResult<RebasedCommit> {
    // If specified, don't create commit where one parent is an ancestor of another.
    let simplified_new_parents;
    let new_parents = if options.simplify_ancestor_merge {
        let mut new_parent_ids = new_parents.iter().map(|commit| commit.id());
        let head_set: HashSet<_> = mut_repo
            .index()
            .heads(&mut new_parent_ids)
            .into_iter()
            .collect();
        simplified_new_parents = new_parents
            .iter()
            .filter(|commit| head_set.contains(commit.id()))
            .cloned()
            .collect_vec();
        &simplified_new_parents[..]
    } else {
        new_parents
    };

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
        let should_abandon = match options.empty {
            EmptyBehaviour::Keep => false,
            EmptyBehaviour::AbandonNewlyEmpty => {
                *parent.tree_id() == new_tree_id
                    && old_base_tree_id.map_or(false, |id| id != *old_commit.tree_id())
            }
            EmptyBehaviour::AbandonAllEmpty => *parent.tree_id() == new_tree_id,
        };
        if should_abandon {
            mut_repo.record_abandoned_commit_with_parents(
                old_commit.id().clone(),
                std::iter::once(parent.id().clone()),
            );
            return Ok(RebasedCommit::Abandoned {
                parent: parent.clone(),
            });
        }
    }
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    let new_commit = mut_repo
        .rewrite_commit(settings, old_commit)
        .set_parents(new_parent_ids)
        .set_tree_id(new_tree_id)
        .write()?;
    Ok(RebasedCommit::Rewritten(new_commit))
}

pub fn rebase_to_dest_parent(
    repo: &dyn Repo,
    source: &Commit,
    destination: &Commit,
) -> BackendResult<MergedTree> {
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
) -> BackendResult<Commit> {
    let old_base_tree = merge_commit_trees(mut_repo, &old_commit.parents())?;
    let new_base_tree = merge_commit_trees(mut_repo, new_parents)?;
    let old_tree = old_commit.tree()?;
    let new_tree = new_base_tree.merge(&old_tree, &old_base_tree)?;
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    // TODO: i18n the description based on repo language
    mut_repo
        .new_commit(settings, new_parent_ids, new_tree.id())
        .set_description(format!("backout of commit {}", &old_commit.id().hex()))
        .write()
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
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
    ) -> DescendantRebaser<'settings, 'repo> {
        let store = mut_repo.store();
        let old_commits_expression =
            RevsetExpression::commits(mut_repo.parent_mapping.keys().cloned().collect());
        let to_visit_expression = old_commits_expression
            .descendants()
            .minus(&old_commits_expression);
        let to_visit_revset = to_visit_expression.evaluate_programmatic(mut_repo).unwrap();
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
                    if let Some((_, targets)) = mut_repo.parent_mapping.get(parent.id()) {
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
    ) -> Result<(), BackendError> {
        // We arbitrarily pick a new working-copy commit among the candidates.
        let abandoned_old_commit = matches!(
            self.mut_repo.parent_mapping.get(&old_commit_id),
            Some((RewriteType::Abandoned, _))
        );
        self.update_wc_commits(&old_commit_id, &new_commit_ids[0], abandoned_old_commit)?;

        // Build a map from commit to branches pointing to it, so we don't need to scan
        // all branches each time we rebase a commit.
        // TODO: We no longer need to do this now that we update branches for all
        // commits at once.
        let mut branches: HashMap<_, HashSet<_>> = HashMap::new();
        for (branch_name, target) in self.mut_repo.view().local_branches() {
            for commit in target.added_ids() {
                branches
                    .entry(commit.clone())
                    .or_default()
                    .insert(branch_name.to_owned());
            }
        }

        if let Some(branch_names) = branches.get(&old_commit_id).cloned() {
            let mut branch_updates = vec![];
            for branch_name in &branch_names {
                let local_target = self.mut_repo.get_local_branch(branch_name);
                for old_add in local_target.added_ids() {
                    if *old_add == old_commit_id {
                        branch_updates.push(branch_name.clone());
                    }
                }
            }
            let (old_target, new_target) =
                DescendantRebaser::ref_target_update(old_commit_id, new_commit_ids);
            for branch_name in &branch_updates {
                self.mut_repo
                    .merge_local_branch(branch_name, &old_target, &new_target);
            }
        }

        Ok(())
    }

    fn update_wc_commits(
        &mut self,
        old_commit_id: &CommitId,
        new_commit_id: &CommitId,
        abandoned_old_commit: bool,
    ) -> Result<(), BackendError> {
        let workspaces_to_update = self
            .mut_repo
            .view()
            .workspaces_for_wc_commit_id(old_commit_id);
        if workspaces_to_update.is_empty() {
            return Ok(());
        }

        let new_commit = self.mut_repo.store().get_commit(new_commit_id)?;
        let new_wc_commit = if !abandoned_old_commit {
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

    fn rebase_one(&mut self, old_commit: Commit) -> BackendResult<()> {
        let old_commit_id = old_commit.id().clone();
        assert!(!self.mut_repo.parent_mapping.contains_key(&old_commit_id));
        let old_parent_ids = old_commit.parent_ids();
        let new_parent_ids = self.mut_repo.new_parents(old_parent_ids);
        if new_parent_ids == old_parent_ids {
            // The commit is already in place.
            return Ok(());
        }
        assert_eq!(
            (
                self.rebased.get(&old_commit_id),
                self.mut_repo.parent_mapping.get(&old_commit_id)
            ),
            (None, None),
            "Trying to rebase the same commit {old_commit_id:?} in two different ways",
        );

        let new_parents: Vec<_> = new_parent_ids
            .iter()
            .map(|new_parent_id| self.mut_repo.store().get_commit(new_parent_id))
            .try_collect()?;
        let rebased_commit: RebasedCommit = rebase_commit_with_options(
            self.settings,
            self.mut_repo,
            &old_commit,
            &new_parents,
            &self.options,
        )?;
        let new_commit = match rebased_commit {
            RebasedCommit::Rewritten(new_commit) => new_commit,
            RebasedCommit::Abandoned { parent } => parent,
        };
        self.rebased
            .insert(old_commit_id.clone(), new_commit.id().clone());
        Ok(())
    }

    fn update_all_references(&mut self) -> Result<(), BackendError> {
        for (old_parent_id, (_, new_parent_ids)) in self.mut_repo.parent_mapping.clone() {
            // Call `new_parents()` here since `parent_mapping` only contains direct
            // mappings, not transitive ones.
            // TODO: keep parent_mapping updated with transitive mappings so we don't need
            // to call `new_parents()` here.
            let new_parent_ids = self.mut_repo.new_parents(&new_parent_ids);
            self.update_references(old_parent_id, new_parent_ids)?;
        }
        Ok(())
    }

    fn update_heads(&mut self) {
        let old_commits_expression =
            RevsetExpression::commits(self.mut_repo.parent_mapping.keys().cloned().collect());
        let heads_to_add_expression = old_commits_expression
            .parents()
            .minus(&old_commits_expression);
        let heads_to_add = heads_to_add_expression
            .evaluate_programmatic(self.mut_repo)
            .unwrap()
            .iter();

        let mut view = self.mut_repo.view().store_view().clone();
        for commit_id in self.mut_repo.parent_mapping.keys() {
            view.head_ids.remove(commit_id);
        }
        view.head_ids.extend(heads_to_add);
        self.mut_repo.set_view(view);
    }

    pub fn rebase_all(&mut self) -> BackendResult<()> {
        while let Some(old_commit) = self.to_visit.pop() {
            self.rebase_one(old_commit)?;
        }
        self.update_all_references()?;
        self.update_heads();

        Ok(())
    }
}
