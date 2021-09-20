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
use crate::op_store::RefTarget;
use crate::repo::{MutableRepo, RepoRef};
use crate::repo_path::RepoPath;
use crate::revset::RevsetExpression;
use crate::settings::UserSettings;
use crate::tree::{merge_trees, Tree};
use crate::view::RefName;

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
    replacements: HashMap<CommitId, Vec<CommitId>>,
    // In reverse order, so we can remove the last one to rebase first.
    to_visit: Vec<CommitId>,
    // Ancestors of the destinations. These were also in `to_visit` to start with, but we don't
    // want to rebase them. Instead, we record them in `replacements` when we visit them. That way,
    // their descendants will be rebased correctly.
    ancestors: HashSet<CommitId>,
    rebased: HashMap<CommitId, CommitId>,
}

impl<'settings, 'repo> DescendantRebaser<'settings, 'repo> {
    pub fn new(
        settings: &'settings UserSettings,
        mut_repo: &'repo mut MutableRepo,
        replacements: HashMap<CommitId, Vec<CommitId>>,
    ) -> DescendantRebaser<'settings, 'repo> {
        let old_commits_expression =
            RevsetExpression::commits(replacements.keys().cloned().collect());
        let new_commits_expression =
            RevsetExpression::commits(replacements.values().flatten().cloned().collect());

        let to_visit_expression = old_commits_expression
            .descendants(&RevsetExpression::all_non_obsolete_heads())
            .minus(&old_commits_expression);
        let to_visit_revset = to_visit_expression
            .evaluate(mut_repo.as_repo_ref())
            .unwrap();
        let mut to_visit = vec![];
        for index_entry in to_visit_revset.iter() {
            to_visit.push(index_entry.commit_id());
        }
        drop(to_visit_revset);

        let ancestors_expression =
            to_visit_expression.intersection(&new_commits_expression.ancestors());
        let ancestors_revset = ancestors_expression
            .evaluate(mut_repo.as_repo_ref())
            .unwrap();
        let mut ancestors = HashSet::new();
        for index_entry in ancestors_revset.iter() {
            ancestors.insert(index_entry.commit_id());
        }
        drop(ancestors_revset);

        DescendantRebaser {
            settings,
            mut_repo,
            replacements,
            to_visit,
            ancestors,
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
        while let Some(old_commit_id) = self.to_visit.pop() {
            let old_commit = self.mut_repo.store().get_commit(&old_commit_id).unwrap();
            let mut new_parent_ids = vec![];
            let old_parent_ids = old_commit.parent_ids();
            for old_parent_id in &old_parent_ids {
                if let Some(replacements) = self.replacements.get(old_parent_id) {
                    new_parent_ids.extend(replacements.clone());
                } else if let Some(new_parent_id) = self.rebased.get(old_parent_id) {
                    new_parent_ids.push(new_parent_id.clone());
                } else {
                    new_parent_ids.push(old_parent_id.clone());
                };
            }
            if self.ancestors.contains(&old_commit_id) {
                // Update the `replacements` map so descendants are rebased correctly.
                self.replacements.insert(old_commit_id, new_parent_ids);
                continue;
            } else if new_parent_ids == old_parent_ids {
                // The commit is already in place.
                continue;
            }

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
            let new_commit = rebase_commit(self.settings, self.mut_repo, &old_commit, &new_parents);
            self.rebased.insert(old_commit_id, new_commit.id().clone());
            return Some(RebasedDescendant {
                old_commit,
                new_commit,
            });
        }
        None
    }

    pub fn rebase_all(&mut self) {
        while self.rebase_next().is_some() {}
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RebasedDescendant {
    pub old_commit: Commit,
    pub new_commit: Commit,
}

pub fn update_branches_after_rewrite(mut_repo: &mut MutableRepo) {
    let new_evolution = mut_repo.evolution();
    let base_repo = mut_repo.base_repo();
    let old_evolution = base_repo.evolution();
    let mut updates = vec![];
    let index = mut_repo.index().as_index_ref();

    let ref_target_update = |old_id: CommitId| -> Option<(RefTarget, RefTarget)> {
        if new_evolution.is_obsolete(&old_id) && !old_evolution.is_obsolete(&old_id) {
            // The call to index.heads() is mostly to get a predictable order
            let new_ids = index.heads(&new_evolution.new_parent(mut_repo.as_repo_ref(), &old_id));
            let old_ids = std::iter::repeat(old_id).take(new_ids.len()).collect_vec();
            Some((
                RefTarget::Conflict {
                    removes: vec![],
                    adds: old_ids,
                },
                RefTarget::Conflict {
                    removes: vec![],
                    adds: new_ids,
                },
            ))
        } else {
            None
        }
    };

    for (branch_name, branch_target) in mut_repo.view().branches() {
        if let Some(old_target) = &branch_target.local_target {
            for old_add in old_target.adds() {
                if let Some((old_target, new_target)) = ref_target_update(old_add) {
                    updates.push((branch_name.clone(), old_target, new_target));
                }
            }
            for old_remove in old_target.removes() {
                if let Some((old_target, new_target)) = ref_target_update(old_remove) {
                    // Arguments reversed for removes
                    updates.push((branch_name.clone(), new_target, old_target));
                }
            }
        }
    }

    for (branch_name, old_target, new_target) in updates {
        mut_repo.merge_single_ref(
            &RefName::LocalBranch(branch_name),
            Some(&old_target),
            Some(&new_target),
        );
    }
}
