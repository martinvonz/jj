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
use std::sync::{Arc, Mutex};

use crate::commit::Commit;
use crate::commit_builder::CommitBuilder;
use crate::dag_walk::{bfs, closest_common_node, leaves, walk_ancestors};
use crate::repo::{ReadonlyRepo, Repo};
use crate::repo_path::DirRepoPath;
use crate::rewrite::{merge_commit_trees, rebase_commit};
use crate::settings::UserSettings;
use crate::store::{ChangeId, CommitId};
use crate::store_wrapper::StoreWrapper;
use crate::transaction::{MutableRepo, Transaction};
use crate::trees::merge_trees;
use crate::view::View;

#[derive(Debug, Clone)]
struct State {
    /// Contains all successors whether they have the same change id or not.
    successors: HashMap<CommitId, HashSet<CommitId>>,
    /// Contains the subset of the keys in `successors` for which there is a
    /// successor with the same change id.
    obsolete_commits: HashSet<CommitId>,
    orphan_commits: HashSet<CommitId>,
    divergent_changes: HashMap<ChangeId, HashSet<CommitId>>,
}

impl State {
    fn calculate(store: &StoreWrapper, view: &dyn View) -> State {
        let mut successors = HashMap::new();
        let mut obsolete_commits = HashSet::new();
        let mut orphan_commits = HashSet::new();
        let mut divergent_changes = HashMap::new();
        let mut heads = vec![];
        for commit_id in view.heads() {
            heads.push(store.get_commit(commit_id).unwrap());
        }
        let mut commits = HashSet::new();
        let mut children = HashMap::new();
        let mut change_to_commits = HashMap::new();
        for commit in walk_ancestors(heads) {
            children.insert(commit.id().clone(), HashSet::new());
            change_to_commits
                .entry(commit.change_id().clone())
                .or_insert_with(HashSet::new)
                .insert(commit.id().clone());
            commits.insert(commit);
        }
        // Scan all commits to find obsolete commits and to build a lookup of for
        // children of a commit
        for commit in &commits {
            if commit.is_pruned() {
                obsolete_commits.insert(commit.id().clone());
            }
            for predecessor in commit.predecessors() {
                if !commits.contains(&predecessor) {
                    continue;
                }
                successors
                    .entry(predecessor.id().clone())
                    .or_insert_with(HashSet::new)
                    .insert(commit.id().clone());
                if predecessor.change_id() == commit.change_id() {
                    obsolete_commits.insert(predecessor.id().clone());
                }
            }
            for parent in commit.parents() {
                if let Some(children) = children.get_mut(parent.id()) {
                    children.insert(commit.id().clone());
                }
            }
        }
        // Find divergent commits
        for (change_id, commit_ids) in change_to_commits {
            let divergent: HashSet<CommitId> =
                commit_ids.difference(&obsolete_commits).cloned().collect();
            if divergent.len() > 1 {
                divergent_changes.insert(change_id, divergent);
            }
        }
        // Find orphans by walking to the children of obsolete commits
        let mut work: Vec<CommitId> = obsolete_commits.iter().map(ToOwned::to_owned).collect();
        while !work.is_empty() {
            let commit_id = work.pop().unwrap();
            for child in children.get(&commit_id).unwrap() {
                if orphan_commits.insert(child.clone()) {
                    work.push(child.clone());
                }
            }
        }
        orphan_commits = orphan_commits
            .difference(&obsolete_commits)
            .map(ToOwned::to_owned)
            .collect();

        State {
            successors,
            obsolete_commits,
            orphan_commits,
            divergent_changes,
        }
    }

    fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId> {
        self.successors
            .get(commit_id)
            .cloned()
            .unwrap_or_else(HashSet::new)
    }

    fn is_obsolete(&self, commit_id: &CommitId) -> bool {
        self.obsolete_commits.contains(commit_id)
    }

    fn is_orphan(&self, commit_id: &CommitId) -> bool {
        self.orphan_commits.contains(commit_id)
    }

    fn is_divergent(&self, change_id: &ChangeId) -> bool {
        self.divergent_changes.contains_key(change_id)
    }

    pub fn new_parent(&self, store: &StoreWrapper, old_parent_id: &CommitId) -> HashSet<CommitId> {
        let mut new_parents = HashSet::new();
        if let Some(successor_ids) = self.successors.get(old_parent_id) {
            let old_parent = store.get_commit(old_parent_id).unwrap();
            let successors: HashSet<_> = successor_ids
                .iter()
                .map(|id| store.get_commit(id).unwrap())
                .collect();
            let mut children = HashMap::new();
            for successor in &successors {
                for parent in successor.parents() {
                    if let Some(parent) = successors.get(&parent) {
                        children
                            .entry(parent.clone())
                            .or_insert_with(HashSet::new)
                            .insert(successor.clone());
                    }
                }
            }
            let mut all_candidates = HashSet::new();
            for successor in &successors {
                if successor.change_id() != old_parent.change_id() {
                    continue;
                }

                // Start with the successor as candidate.
                let mut candidates = HashSet::new();
                candidates.insert(successor.clone());

                // If the successor has children that are successors of the same
                // commit, we consider the original commit to be a split. We then return
                // the tip-most successor.
                candidates = leaves(
                    candidates,
                    &mut |commit: &Commit| -> HashSet<Commit> {
                        if let Some(children) = children.get(commit) {
                            children.clone()
                        } else {
                            HashSet::new()
                        }
                    },
                    &|commit: &Commit| -> CommitId { commit.id().clone() },
                );

                // If a successor is pruned, use its parent(s) instead.
                candidates = leaves(
                    candidates,
                    &mut |commit: &Commit| -> Vec<Commit> {
                        if commit.is_pruned() {
                            commit.parents()
                        } else {
                            vec![]
                        }
                    },
                    &|commit: &Commit| -> CommitId { commit.id().clone() },
                );

                for candidate in candidates {
                    all_candidates.insert(candidate.clone());
                }
            }

            // Filter out candidates that are ancestors of or other candidates.
            let non_heads: Vec<_> = all_candidates
                .iter()
                .flat_map(|commit| commit.parents())
                .collect();
            for commit in walk_ancestors(non_heads) {
                all_candidates.remove(&commit);
            }

            for candidate in all_candidates {
                // TODO: Make this not recursive
                for effective_successor in self.new_parent(store, candidate.id()) {
                    new_parents.insert(effective_successor);
                }
            }
        }
        if new_parents.is_empty() {
            // TODO: Should we go to the parents here too if the commit is pruned?
            new_parents.insert(old_parent_id.clone());
        }
        new_parents
    }
}

pub trait Evolution {
    fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId>;

    fn is_obsolete(&self, commit_id: &CommitId) -> bool;

    fn is_orphan(&self, commit_id: &CommitId) -> bool;

    fn is_divergent(&self, change_id: &ChangeId) -> bool;

    /// Given a current parent, finds the new parent candidates. If the current
    /// parent is not obsolete, then a singleton set of that commit will be
    /// returned.
    ///
    ///  * If a successor is pruned, its parent(s) will instead be included (or
    ///    their parents if they are also pruned).
    ///
    ///  * If the commit has multiple live successors, the tip-most one(s) of
    ///    them will be chosen.
    ///
    /// The second case is more complex than it probably seems. For example,
    /// let's say commit A was split into B, A', and C (where A' has the same
    /// change id as A). Then C is rebased to somewhere else and becomes C'.
    /// We will choose that C' as effective successor even though it has a
    /// different change id and is not a descendant of one that does.
    fn new_parent(&self, old_parent_id: &CommitId) -> HashSet<CommitId>;
}

pub struct ReadonlyEvolution<'r> {
    repo: &'r ReadonlyRepo,
    state: Mutex<Option<Arc<State>>>,
}

pub trait EvolveListener {
    fn orphan_evolved(&mut self, orphan: &Commit, new_commit: &Commit);
    fn orphan_target_ambiguous(&mut self, orphan: &Commit);
    fn divergent_resolved(&mut self, divergents: &[Commit], resolved: &Commit);
    fn divergent_no_common_predecessor(&mut self, commit1: &Commit, commit2: &Commit);
}

impl Evolution for ReadonlyEvolution<'_> {
    fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId> {
        self.get_state().successors(commit_id)
    }

    fn is_obsolete(&self, commit_id: &CommitId) -> bool {
        self.get_state().is_obsolete(commit_id)
    }

    fn is_orphan(&self, commit_id: &CommitId) -> bool {
        self.get_state().is_orphan(commit_id)
    }

    fn is_divergent(&self, change_id: &ChangeId) -> bool {
        self.get_state().is_divergent(change_id)
    }

    fn new_parent(&self, old_parent_id: &CommitId) -> HashSet<CommitId> {
        self.get_state()
            .new_parent(self.repo.store(), old_parent_id)
    }
}

impl<'r> ReadonlyEvolution<'r> {
    pub fn new(repo: &'r ReadonlyRepo) -> Self {
        ReadonlyEvolution {
            repo,
            state: Mutex::new(None),
        }
    }

    fn get_state(&self) -> Arc<State> {
        let mut locked_state = self.state.lock().unwrap();
        if locked_state.is_none() {
            locked_state.replace(Arc::new(State::calculate(
                self.repo.store(),
                self.repo.view(),
            )));
        }
        locked_state.as_ref().unwrap().clone()
    }

    pub fn start_modification<'m>(&self, repo: &'m MutableRepo<'r>) -> MutableEvolution<'r, 'm> {
        MutableEvolution {
            repo,
            state: self.get_state().as_ref().clone(),
        }
    }
}

pub struct MutableEvolution<'r, 'm: 'r> {
    repo: &'m MutableRepo<'r>,
    state: State,
}

impl Evolution for MutableEvolution<'_, '_> {
    fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId> {
        self.state.successors(commit_id)
    }

    fn is_obsolete(&self, commit_id: &CommitId) -> bool {
        self.state.is_obsolete(commit_id)
    }

    fn is_orphan(&self, commit_id: &CommitId) -> bool {
        self.state.is_orphan(commit_id)
    }

    fn is_divergent(&self, change_id: &ChangeId) -> bool {
        self.state.is_divergent(change_id)
    }

    fn new_parent(&self, old_parent_id: &CommitId) -> HashSet<CommitId> {
        self.state.new_parent(self.repo.store(), old_parent_id)
    }
}

impl MutableEvolution<'_, '_> {
    pub fn invalidate(&mut self) {
        self.state = State::calculate(self.repo.store(), self.repo.view());
    }
}

pub fn evolve(
    user_settings: &UserSettings,
    tx: &mut Transaction,
    listener: &mut dyn EvolveListener,
) {
    let store = tx.store().clone();
    // TODO: update the state in the transaction
    let state = tx.as_repo_mut().evolution_mut().state.clone();

    // Resolving divergence can creates new orphans but not vice versa, so resolve
    // divergence first.
    for commit_ids in state.divergent_changes.values() {
        let commits: HashSet<Commit> = commit_ids
            .iter()
            .map(|id| store.get_commit(&id).unwrap())
            .collect();
        evolve_divergent_change(user_settings, &store, tx, listener, &commits);
    }

    let orphans: HashSet<Commit> = state
        .orphan_commits
        .iter()
        .map(|id| store.get_commit(&id).unwrap())
        .collect();
    let non_heads: HashSet<Commit> = orphans.iter().flat_map(|commit| commit.parents()).collect();
    let orphan_heads: HashSet<Commit> = orphans.difference(&non_heads).cloned().collect();
    let mut orphans_topo_order = vec![];
    for commit in bfs(
        orphan_heads,
        Box::new(|commit| commit.id().clone()),
        Box::new(|commit| {
            commit
                .parents()
                .iter()
                .filter(|commit| state.orphan_commits.contains(commit.id()))
                .cloned()
                .collect::<Vec<_>>()
        }),
    ) {
        orphans_topo_order.push(commit);
    }

    while !orphans_topo_order.is_empty() {
        let orphan = orphans_topo_order.pop().unwrap();
        let old_parents = orphan.parents();
        let mut new_parents = vec![];
        let mut ambiguous_new_parents = false;
        for old_parent in &old_parents {
            let new_parent_candidates = state.new_parent(&store, old_parent.id());
            if new_parent_candidates.len() > 1 {
                ambiguous_new_parents = true;
                break;
            }
            new_parents.push(
                store
                    .get_commit(new_parent_candidates.iter().next().unwrap())
                    .unwrap(),
            );
        }
        if ambiguous_new_parents {
            listener.orphan_target_ambiguous(&orphan);
        } else {
            let new_commit = rebase_commit(user_settings, tx, &orphan, &new_parents);
            listener.orphan_evolved(&orphan, &new_commit);
        }
    }
}

fn evolve_divergent_change(
    user_settings: &UserSettings,
    store: &Arc<StoreWrapper>,
    tx: &mut Transaction,
    listener: &mut dyn EvolveListener,
    commits: &HashSet<Commit>,
) {
    // Resolve divergence pair-wise, starting with the two oldest commits.
    let mut commits: Vec<Commit> = commits.iter().cloned().collect();
    commits.sort_by(|a: &Commit, b: &Commit| a.committer().timestamp.cmp(&b.committer().timestamp));
    commits.reverse();

    // Create a copy to pass to the listener
    let sources = commits.clone();

    while commits.len() > 1 {
        let commit2 = commits.pop().unwrap();
        let commit1 = commits.pop().unwrap();

        let common_predecessor = closest_common_node(
            vec![commit1.clone()],
            vec![commit2.clone()],
            &|commit: &Commit| commit.predecessors(),
            &|commit: &Commit| commit.id().clone(),
        );
        match common_predecessor {
            None => {
                listener.divergent_no_common_predecessor(&commit1, &commit2);
                return;
            }
            Some(common_predecessor) => {
                let resolved_commit = evolve_two_divergent_commits(
                    user_settings,
                    store,
                    tx,
                    &common_predecessor,
                    &commit1,
                    &commit2,
                );
                commits.push(resolved_commit);
            }
        }
    }

    let resolved = commits.pop().unwrap();
    listener.divergent_resolved(&sources, &resolved);
}

fn evolve_two_divergent_commits(
    user_settings: &UserSettings,
    store: &Arc<StoreWrapper>,
    tx: &mut Transaction,
    common_predecessor: &Commit,
    commit1: &Commit,
    commit2: &Commit,
) -> Commit {
    let new_parents = commit1.parents();
    let rebased_tree2 = if commit2.parents() == new_parents {
        commit2.tree()
    } else {
        let old_base_tree = merge_commit_trees(store, &commit2.parents());
        let new_base_tree = merge_commit_trees(store, &new_parents);
        let tree_id = merge_trees(&new_base_tree, &old_base_tree, &commit2.tree()).unwrap();
        store.get_tree(&DirRepoPath::root(), &tree_id).unwrap()
    };
    let rebased_predecessor_tree = if common_predecessor.parents() == new_parents {
        common_predecessor.tree()
    } else {
        let old_base_tree = merge_commit_trees(store, &common_predecessor.parents());
        let new_base_tree = merge_commit_trees(store, &new_parents);
        let tree_id =
            merge_trees(&new_base_tree, &old_base_tree, &common_predecessor.tree()).unwrap();
        store.get_tree(&DirRepoPath::root(), &tree_id).unwrap()
    };

    let resolved_tree =
        merge_trees(&commit1.tree(), &rebased_predecessor_tree, &rebased_tree2).unwrap();

    // TODO: Merge commit description and other commit metadata. How do we deal with
    // conflicts? It's probably best to interactively ask the caller (which
    // might ask the user in interactive use).
    CommitBuilder::for_rewrite_from(user_settings, store, &commit1)
        .set_tree(resolved_tree)
        .set_predecessors(vec![commit1.id().clone(), commit2.id().clone()])
        .write_to_transaction(tx)
}
