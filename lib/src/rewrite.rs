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
use crate::index::Index;
use crate::matchers::{Matcher, Visit};
use crate::merged_tree::{MergedTree, MergedTreeBuilder};
use crate::object_id::ObjectId;
use crate::repo::{MutableRepo, Repo};
use crate::repo_path::RepoPath;
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
    old_commit: Commit,
    new_parents: Vec<CommitId>,
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
    old_commit: Commit,
    mut new_parent_ids: Vec<CommitId>,
    options: &RebaseOptions,
) -> BackendResult<RebasedCommit> {
    // If specified, don't create commit where one parent is an ancestor of another.
    if options.simplify_ancestor_merge {
        let head_set: HashSet<_> = mut_repo
            .index()
            .heads(&mut new_parent_ids.iter())
            .into_iter()
            .collect();
        new_parent_ids.retain(|id| head_set.contains(id))
    };
    let new_parents: Vec<Commit> = new_parent_ids
        .iter()
        .map(|id| mut_repo.store().get_commit(id))
        .try_collect()?;

    let old_parents = old_commit.parents();
    let old_parent_trees = old_parents
        .iter()
        .map(|parent| parent.tree_id().clone())
        .collect_vec();
    let new_parent_trees = new_parents
        .iter()
        .map(|parent| parent.tree_id().clone())
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
        let new_base_tree = merge_commit_trees(mut_repo, &new_parents)?;
        let old_tree = old_commit.tree()?;
        (
            Some(old_base_tree.id()),
            new_base_tree.merge(&old_base_tree, &old_tree)?.id(),
        )
    };
    // Ensure we don't abandon commits with multiple parents (merge commits), even
    // if they're empty.
    if let [parent] = &new_parents[..] {
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
        .rewrite_commit(settings, &old_commit)
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
        let new_parent_ids = self.mut_repo.new_parents(old_parent_ids);
        if new_parent_ids == old_parent_ids {
            // The commit is already in place.
            return Ok(());
        }

        let rebased_commit: RebasedCommit = rebase_commit_with_options(
            self.settings,
            self.mut_repo,
            old_commit,
            new_parent_ids,
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

    pub fn rebase_all(&mut self) -> BackendResult<()> {
        while let Some(old_commit) = self.to_visit.pop() {
            self.rebase_one(old_commit)?;
        }
        self.mut_repo.update_rewritten_references(self.settings)
    }
}
