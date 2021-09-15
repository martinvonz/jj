// Copyright 2020 Google LLC
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

use itertools::Itertools;

use crate::backend::CommitId;
use crate::commit::Commit;
use crate::commit_builder::CommitBuilder;
use crate::repo::{MutableRepo, RepoRef};
use crate::repo_path::RepoPath;
use crate::revset::RevsetExpression;
use crate::settings::UserSettings;
use crate::tree::{merge_trees, Tree};

pub fn merge_commit_trees(repo: RepoRef, commits: &[Commit]) -> Tree {
    let store = repo.store();
    if commits.is_empty() {
        store
            .get_tree(&RepoPath::root(), store.empty_tree_id())
            .unwrap()
    } else {
        let index = repo.index();
        let mut new_tree = commits[0].tree();
        let commit_ids = commits
            .iter()
            .map(|commit| commit.id().clone())
            .collect_vec();
        for (i, other_commit) in commits.iter().enumerate().skip(1) {
            let ancestor_ids = index.common_ancestors(&commit_ids[0..i], &[commit_ids[i].clone()]);
            let ancestors = ancestor_ids
                .iter()
                .map(|id| store.get_commit(id).unwrap())
                .collect_vec();
            let ancestor_tree = merge_commit_trees(repo, &ancestors);
            let new_tree_id = merge_trees(&new_tree, &ancestor_tree, &other_commit.tree()).unwrap();
            new_tree = store.get_tree(&RepoPath::root(), &new_tree_id).unwrap();
        }
        new_tree
    }
}

pub fn rebase_commit(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    old_commit: &Commit,
    new_parents: &[Commit],
) -> Commit {
    let store = mut_repo.store();
    let old_base_tree = merge_commit_trees(mut_repo.as_repo_ref(), &old_commit.parents());
    let new_base_tree = merge_commit_trees(mut_repo.as_repo_ref(), new_parents);
    // TODO: pass in labels for the merge parts
    let new_tree_id = merge_trees(&new_base_tree, &old_base_tree, &old_commit.tree()).unwrap();
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    CommitBuilder::for_rewrite_from(settings, store, old_commit)
        .set_parents(new_parent_ids)
        .set_tree(new_tree_id)
        .write_to_repo(mut_repo)
}

pub fn back_out_commit(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    old_commit: &Commit,
    new_parents: &[Commit],
) -> Commit {
    let store = mut_repo.store();
    let old_base_tree = merge_commit_trees(mut_repo.as_repo_ref(), &old_commit.parents());
    let new_base_tree = merge_commit_trees(mut_repo.as_repo_ref(), new_parents);
    // TODO: pass in labels for the merge parts
    let new_tree_id = merge_trees(&new_base_tree, &old_commit.tree(), &old_base_tree).unwrap();
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    // TODO: i18n the description based on repo language
    CommitBuilder::for_new_commit(settings, store, new_tree_id)
        .set_parents(new_parent_ids)
        .set_description(format!(
            "backout of commit {}",
            hex::encode(&old_commit.id().0)
        ))
        .write_to_repo(mut_repo)
}

/// Rebases descendants of a commit onto a new commit (or several).
// TODO: Should there be an option to drop empty commits (and/or an option to
// drop empty commits only if they weren't already empty)? Or maybe that
// shouldn't be this type's job.
pub struct DescendantRebaser<'settings, 'repo> {
    settings: &'settings UserSettings,
    mut_repo: &'repo mut MutableRepo,
    old_parent_id: CommitId,
    new_parent_ids: Vec<CommitId>,
    // In reverse order, so we can remove the last one to rebase first.
    to_rebase: Vec<CommitId>,
    rebased: HashMap<CommitId, CommitId>,
}

impl<'settings, 'repo> DescendantRebaser<'settings, 'repo> {
    pub fn new(
        settings: &'settings UserSettings,
        mut_repo: &'repo mut MutableRepo,
        old_parent_id: CommitId,
        new_parent_ids: Vec<CommitId>,
    ) -> DescendantRebaser<'settings, 'repo> {
        let expression = RevsetExpression::commit(old_parent_id.clone())
            .descendants(&RevsetExpression::all_non_obsolete_heads())
            .minus(
                &RevsetExpression::commit(old_parent_id.clone())
                    .union(&RevsetExpression::commits(new_parent_ids.clone()))
                    .ancestors(),
            );
        let revset = expression.evaluate(mut_repo.as_repo_ref()).unwrap();
        let mut to_rebase = vec![];
        for index_entry in revset.iter() {
            to_rebase.push(index_entry.commit_id());
        }
        drop(revset);

        DescendantRebaser {
            settings,
            mut_repo,
            old_parent_id,
            new_parent_ids,
            to_rebase,
            rebased: Default::default(),
        }
    }

    /// Returns a map from `CommitId` of old commit to new commit. Includes the
    /// commits rebase so far. Does not include the inputs passed to
    /// `rebase_descendants`.
    pub fn rebased(&self) -> &HashMap<CommitId, CommitId> {
        &self.rebased
    }

    pub fn rebase_next(&mut self) -> Option<RebasedDescendant> {
        self.to_rebase.pop().map(|old_commit_id| {
            let old_commit = self.mut_repo.store().get_commit(&old_commit_id).unwrap();
            let mut new_parent_ids = vec![];
            let old_parent_ids = old_commit.parent_ids();
            for old_parent_id in &old_parent_ids {
                if old_parent_id == &self.old_parent_id {
                    new_parent_ids.extend(self.new_parent_ids.clone());
                } else if let Some(new_parent_id) = self.rebased.get(old_parent_id) {
                    new_parent_ids.push(new_parent_id.clone());
                } else {
                    new_parent_ids.push(old_parent_id.clone());
                };
            }
            if new_parent_ids == old_parent_ids {
                RebasedDescendant::AlreadyInPlace(old_commit)
            } else {
                // Don't create commit where one parent is an ancestor of another.
                let head_set: HashSet<_> = self
                    .mut_repo
                    .index()
                    .heads(&new_parent_ids)
                    .iter()
                    .cloned()
                    .collect();
                let new_parent_ids = new_parent_ids
                    .into_iter()
                    .filter(|new_parent| head_set.contains(new_parent))
                    .collect_vec();
                let new_parents = new_parent_ids
                    .iter()
                    .map(|new_parent_id| self.mut_repo.store().get_commit(new_parent_id).unwrap())
                    .collect_vec();
                let new_commit =
                    rebase_commit(self.settings, self.mut_repo, &old_commit, &new_parents);
                self.rebased.insert(old_commit_id, new_commit.id().clone());
                RebasedDescendant::Rebased {
                    old_commit,
                    new_commit,
                }
            }
        })
    }

    pub fn rebase_all(&mut self) {
        while self.rebase_next().is_some() {}
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RebasedDescendant {
    AlreadyInPlace(Commit),
    Rebased {
        old_commit: Commit,
        new_commit: Commit,
    },
}
