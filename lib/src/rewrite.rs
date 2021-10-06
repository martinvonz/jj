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
    // The commit identified by the key has been replaced by all the ones in the value, typically
    // because the key commit was abandoned (the value commits are then the abandoned commit's
    // parents). A child of the key commit should be rebased onto all the value commits. A branch
    // pointing to the key commit should become a conflict pointing to all the value commits.
    new_parents: HashMap<CommitId, Vec<CommitId>>,
    divergent: HashMap<CommitId, Vec<CommitId>>,
    // In reverse order (parents after children), so we can remove the last one to rebase first.
    to_visit: Vec<CommitId>,
    // Commits to visit but skip. These were also in `to_visit` to start with, but we don't
    // want to rebase them. Instead, we record them in `replacements` when we visit them. That way,
    // their descendants will be rebased correctly.
    to_skip: HashSet<CommitId>,
    new_commits: HashSet<CommitId>,
    rebased: HashMap<CommitId, CommitId>,
    // Names of branches where local target includes the commit id in the key.
    branches: HashMap<CommitId, Vec<String>>,
    // Parents of rebased/abandoned commit that should become new heads once their descendants
    // have been rebased.
    heads_to_add: HashSet<CommitId>,
    heads_to_remove: Vec<CommitId>,
}

impl<'settings, 'repo> DescendantRebaser<'settings, 'repo> {
    pub fn new(
        settings: &'settings UserSettings,
        mut_repo: &'repo mut MutableRepo,
        rewritten: HashMap<CommitId, HashSet<CommitId>>,
        abandoned: HashSet<CommitId>,
    ) -> DescendantRebaser<'settings, 'repo> {
        let old_commits_expression = RevsetExpression::commits(rewritten.keys().cloned().collect())
            .union(&RevsetExpression::commits(
                abandoned.iter().cloned().collect(),
            ));
        let heads_to_add_expression = old_commits_expression
            .parents()
            .minus(&old_commits_expression);
        let heads_to_add = heads_to_add_expression
            .evaluate(mut_repo.as_repo_ref())
            .unwrap()
            .iter()
            .commit_ids()
            .collect();

        let to_visit_expression = old_commits_expression.descendants();
        let to_visit_revset = to_visit_expression
            .evaluate(mut_repo.as_repo_ref())
            .unwrap();
        let to_visit = to_visit_revset.iter().commit_ids().collect_vec();
        drop(to_visit_revset);

        let new_commits_expression =
            RevsetExpression::commits(rewritten.values().flatten().cloned().collect());
        let ancestors_expression =
            to_visit_expression.intersection(&new_commits_expression.ancestors());
        let ancestors_revset = ancestors_expression
            .evaluate(mut_repo.as_repo_ref())
            .unwrap();
        let mut to_skip = abandoned;
        to_skip.extend(ancestors_revset.iter().commit_ids());
        drop(ancestors_revset);

        let new_commits = rewritten.values().flatten().cloned().collect();

        let mut new_parents = HashMap::new();
        let mut divergent = HashMap::new();
        for (old_commit, new_commits) in rewritten {
            if new_commits.len() == 1 {
                new_parents.insert(old_commit, vec![new_commits.iter().next().unwrap().clone()]);
            } else {
                // The call to index.heads() is mostly to get a predictable order
                let new_commits = mut_repo.index().heads(&new_commits);
                divergent.insert(old_commit, new_commits);
            }
        }

        // Build a map from commit to branches pointing to it, so we don't need to scan
        // all branches each time we rebase a commit.
        let mut branches: HashMap<_, Vec<_>> = HashMap::new();
        for (branch_name, branch_target) in mut_repo.view().branches() {
            if let Some(local_target) = &branch_target.local_target {
                for commit in local_target.adds() {
                    branches
                        .entry(commit)
                        .or_default()
                        .push(branch_name.clone());
                }
                for commit in local_target.removes() {
                    branches
                        .entry(commit)
                        .or_default()
                        .push(branch_name.clone());
                }
            }
        }

        DescendantRebaser {
            settings,
            mut_repo,
            new_parents,
            divergent,
            to_visit,
            to_skip,
            new_commits,
            rebased: Default::default(),
            branches,
            heads_to_add,
            heads_to_remove: Default::default(),
        }
    }

    /// Returns a map from `CommitId` of old commit to new commit. Includes the
    /// commits rebase so far. Does not include the inputs passed to
    /// `rebase_descendants`.
    pub fn rebased(&self) -> &HashMap<CommitId, CommitId> {
        &self.rebased
    }

    fn new_parents(&self, old_ids: &[CommitId]) -> Vec<CommitId> {
        let mut new_ids = vec![];
        for old_id in old_ids {
            if let Some(new_parent_ids) = self.new_parents.get(old_id) {
                new_ids.extend(new_parent_ids.clone());
            } else if let Some(new_parent_id) = self.rebased.get(old_id) {
                new_ids.push(new_parent_id.clone());
            } else {
                new_ids.push(old_id.clone());
            };
        }
        new_ids
    }

    fn ref_target_update(old_id: CommitId, new_ids: Vec<CommitId>) -> (RefTarget, RefTarget) {
        let old_ids = std::iter::repeat(old_id).take(new_ids.len()).collect_vec();
        (
            RefTarget::Conflict {
                removes: vec![],
                adds: old_ids,
            },
            RefTarget::Conflict {
                removes: vec![],
                adds: new_ids,
            },
        )
    }

    fn update_references(&mut self, old_commit_id: CommitId, new_commit_ids: Vec<CommitId>) {
        if *self.mut_repo.view().checkout() == old_commit_id {
            // We arbitrarily pick a new checkout among the candidates.
            let new_commit_id = new_commit_ids[0].clone();
            let new_commit = self.mut_repo.store().get_commit(&new_commit_id).unwrap();
            self.mut_repo.check_out(self.settings, &new_commit);
        }

        if let Some(branch_names) = self.branches.get(&old_commit_id) {
            let view = self.mut_repo.view();
            let mut branch_updates = vec![];
            for branch_name in branch_names {
                let local_target = view.get_local_branch(branch_name).unwrap();
                for old_add in local_target.adds() {
                    if old_add == old_commit_id {
                        branch_updates.push((branch_name.clone(), true));
                    }
                }
                for old_add in local_target.removes() {
                    if old_add == old_commit_id {
                        // Arguments reversed for removes
                        branch_updates.push((branch_name.clone(), false));
                    }
                }
            }
            let (old_target, new_target) =
                DescendantRebaser::ref_target_update(old_commit_id.clone(), new_commit_ids);
            for (branch_name, is_add) in branch_updates {
                if is_add {
                    self.mut_repo.merge_single_ref(
                        &RefName::LocalBranch(branch_name),
                        Some(&old_target),
                        Some(&new_target),
                    );
                } else {
                    // Arguments reversed for removes
                    self.mut_repo.merge_single_ref(
                        &RefName::LocalBranch(branch_name),
                        Some(&new_target),
                        Some(&old_target),
                    );
                }
            }
        }

        self.heads_to_add.remove(&old_commit_id);
        if !self.new_commits.contains(&old_commit_id) {
            self.heads_to_remove.push(old_commit_id);
        }
    }

    // TODO: Perhaps the interface since it's not just about rebasing commits.
    pub fn rebase_next(&mut self) -> Option<RebasedDescendant> {
        while let Some(old_commit_id) = self.to_visit.pop() {
            if let Some(new_parent_ids) = self.new_parents.get(&old_commit_id).cloned() {
                // This is a commit that had already been rebased before `self` was created
                // (i.e. it's part of the input for this rebase). We don't need
                // to rebase it, but we still want to update branches pointing
                // to the old commit.
                self.update_references(old_commit_id, new_parent_ids);
                continue;
            }
            if let Some(divergent_ids) = self.divergent.get(&old_commit_id).cloned() {
                // Leave divergent commits in place. Don't update `new_parents` since we don't
                // want to rebase descendants either.
                self.update_references(old_commit_id, divergent_ids);
                continue;
            }
            let old_commit = self.mut_repo.store().get_commit(&old_commit_id).unwrap();
            let old_parent_ids = old_commit.parent_ids();
            let new_parent_ids = self.new_parents(&old_parent_ids);
            if self.to_skip.contains(&old_commit_id) {
                // Update the `new_parents` map so descendants are rebased correctly.
                self.new_parents
                    .insert(old_commit_id.clone(), new_parent_ids.clone());
                self.update_references(old_commit_id, new_parent_ids);
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
            let new_parents = new_parent_ids
                .iter()
                .filter(|new_parent| head_set.contains(new_parent))
                .map(|new_parent_id| self.mut_repo.store().get_commit(new_parent_id).unwrap())
                .collect_vec();
            let new_commit = rebase_commit(self.settings, self.mut_repo, &old_commit, &new_parents);
            self.update_references(old_commit_id.clone(), vec![new_commit.id().clone()]);
            self.rebased.insert(old_commit_id, new_commit.id().clone());
            return Some(RebasedDescendant {
                old_commit,
                new_commit,
            });
        }
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
