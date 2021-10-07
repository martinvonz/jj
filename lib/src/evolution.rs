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
use std::sync::Arc;

use itertools::Itertools;

use crate::backend::{ChangeId, CommitId};
use crate::commit::Commit;
use crate::dag_walk::{bfs, leaves};
use crate::repo::{MutableRepo, ReadonlyRepo, RepoRef};

// TODO: Combine some maps/sets and use a struct as value instead.
// TODO: Move some of this into the index?
#[derive(Debug, Clone, Default)]
struct State {
    children: HashMap<CommitId, HashSet<CommitId>>,
    /// Contains all successors whether they have the same change id or not.
    successors: HashMap<CommitId, HashSet<CommitId>>,
    /// Contains the subset of the keys in `successors` for which there is a
    /// successor with the same change id.
    obsolete_commits: HashSet<CommitId>,
    pruned_commits: HashSet<CommitId>,
    orphan_commits: HashSet<CommitId>,
    /// If there's more than one element in the value, then the change is
    /// divergent.
    non_obsoletes_by_changeid: HashMap<ChangeId, HashSet<CommitId>>,
}

impl State {
    fn calculate(repo: RepoRef) -> State {
        let view = repo.view();
        let index = repo.index();
        let mut state = State::default();
        let head_ids = view.heads().iter().cloned().collect_vec();
        let mut change_to_commits = HashMap::new();
        for head_id in &head_ids {
            state.children.insert(head_id.clone(), HashSet::new());
        }
        for entry in index.walk_revs(&head_ids, &[]) {
            let commit_id = entry.commit_id();
            change_to_commits
                .entry(entry.change_id().clone())
                .or_insert_with(HashSet::new)
                .insert(commit_id.clone());
            if entry.is_pruned() {
                state.pruned_commits.insert(commit_id.clone());
            }
            for parent_entry in entry.parents() {
                let parent_id = parent_entry.commit_id();
                state
                    .children
                    .entry(parent_id.clone())
                    .or_insert_with(HashSet::new)
                    .insert(commit_id.clone());
            }
            for predecessor_entry in entry.predecessors() {
                let predecessor_id = predecessor_entry.commit_id();
                state
                    .successors
                    .entry(predecessor_id.clone())
                    .or_insert_with(HashSet::new)
                    .insert(commit_id.clone());
                if predecessor_entry.change_id() == entry.change_id() {
                    state.obsolete_commits.insert(predecessor_id.clone());
                }
            }
        }

        // Find non-obsolete commits by change id (potentially divergent commits)
        for (change_id, commit_ids) in change_to_commits {
            let non_obsoletes: HashSet<CommitId> = commit_ids
                .difference(&state.obsolete_commits)
                .cloned()
                .collect();
            state
                .non_obsoletes_by_changeid
                .insert(change_id, non_obsoletes);
        }
        // Find orphans by walking to the children of obsolete commits
        let mut work = state.obsolete_commits.iter().cloned().collect_vec();
        work.extend(state.pruned_commits.iter().cloned());
        while !work.is_empty() {
            let commit_id = work.pop().unwrap();
            if let Some(children) = state.children.get(&commit_id) {
                for child in children {
                    if state.orphan_commits.insert(child.clone()) {
                        work.push(child.clone());
                    }
                }
            }
        }
        state.orphan_commits = state
            .orphan_commits
            .iter()
            .filter(|commit_id| {
                !(state.obsolete_commits.contains(commit_id)
                    || state.pruned_commits.contains(commit_id))
            })
            .cloned()
            .collect();

        state
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
        self.non_obsoletes_by_changeid
            .get(change_id)
            .map_or(false, |non_obsoletes| non_obsoletes.len() > 1)
    }

    fn non_obsoletes(&self, change_id: &ChangeId) -> HashSet<CommitId> {
        self.non_obsoletes_by_changeid
            .get(change_id)
            .cloned()
            .unwrap_or_else(HashSet::new)
    }

    fn add_commit(&mut self, commit: &Commit) {
        self.add_commit_data(
            commit.id(),
            commit.change_id(),
            &commit.parent_ids(),
            &commit.predecessor_ids(),
            commit.is_pruned(),
        );
    }

    fn add_commit_data(
        &mut self,
        commit_id: &CommitId,
        change_id: &ChangeId,
        parents: &[CommitId],
        predecessors: &[CommitId],
        is_pruned: bool,
    ) {
        // TODO: Error out (or ignore?) if the root id is a predecessor or divergent
        // (adding the root once should be fine). Perhaps this is not the right
        // place to do that (we don't know what the root id is here).
        for parent in parents {
            self.children
                .entry(parent.clone())
                .or_default()
                .insert(commit_id.clone());
        }
        if is_pruned {
            self.pruned_commits.insert(commit_id.clone());
        }
        // Update the non_obsoletes_by_changeid by adding the new commit and removing
        // the predecessors.
        self.non_obsoletes_by_changeid
            .entry(change_id.clone())
            .or_default()
            .insert(commit_id.clone());
        for predecessor in predecessors {
            self.successors
                .entry(predecessor.clone())
                .or_default()
                .insert(commit_id.clone());
            let became_obsolete = self
                .non_obsoletes_by_changeid
                .get_mut(change_id)
                .unwrap()
                .remove(predecessor);
            // Mark descendants as orphans if the predecessor just became obsolete.
            if became_obsolete {
                assert!(self.obsolete_commits.insert(predecessor.clone()));

                let mut descendants = HashSet::new();
                for descendant in bfs(
                    vec![predecessor.clone()],
                    Box::new(|commit_id| commit_id.clone()),
                    Box::new(|commit_id| {
                        self.children
                            .get(commit_id)
                            .cloned()
                            .unwrap_or_else(HashSet::new)
                    }),
                ) {
                    descendants.insert(descendant);
                }
                descendants.remove(predecessor);
                descendants = descendants
                    .iter()
                    .filter(|commit_id| {
                        !(self.obsolete_commits.contains(commit_id)
                            || self.pruned_commits.contains(commit_id))
                    })
                    .cloned()
                    .collect();
                self.orphan_commits.extend(descendants);
            }
        }
        // Mark the new commit an orphan if any of its parents are obsolete, pruned, or
        // orphans. Note that this has to be done late, in case a parent just got marked
        // as obsolete or orphan above.
        let is_orphan = parents.iter().any(|parent| {
            self.obsolete_commits.contains(parent)
                || self.pruned_commits.contains(parent)
                || self.orphan_commits.contains(commit_id)
        });
        if is_orphan {
            self.orphan_commits.insert(commit_id.clone());
        }
    }

    pub fn new_parent(&self, repo: RepoRef, old_parent_id: &CommitId) -> HashSet<CommitId> {
        let store = repo.store();
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
                    all_candidates.insert(candidate.id().clone());
                }
            }

            // Filter out candidates that are ancestors of other candidates.
            let all_candidates = repo
                .index()
                .heads(all_candidates.iter())
                .into_iter()
                .collect_vec();

            for candidate in all_candidates {
                // TODO: Make this not recursive
                for effective_successor in self.new_parent(repo, &candidate) {
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

pub enum EvolutionRef<'a> {
    Readonly(Arc<ReadonlyEvolution>),
    Mutable(&'a MutableEvolution),
}

impl EvolutionRef<'_> {
    pub fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId> {
        match self {
            EvolutionRef::Readonly(evolution) => evolution.successors(commit_id),
            EvolutionRef::Mutable(evolution) => evolution.successors(commit_id),
        }
    }

    pub fn is_obsolete(&self, commit_id: &CommitId) -> bool {
        match self {
            EvolutionRef::Readonly(evolution) => evolution.is_obsolete(commit_id),
            EvolutionRef::Mutable(evolution) => evolution.is_obsolete(commit_id),
        }
    }

    pub fn is_orphan(&self, commit_id: &CommitId) -> bool {
        match self {
            EvolutionRef::Readonly(evolution) => evolution.is_orphan(commit_id),
            EvolutionRef::Mutable(evolution) => evolution.is_orphan(commit_id),
        }
    }

    pub fn is_divergent(&self, change_id: &ChangeId) -> bool {
        match self {
            EvolutionRef::Readonly(evolution) => evolution.is_divergent(change_id),
            EvolutionRef::Mutable(evolution) => evolution.is_divergent(change_id),
        }
    }

    pub fn non_obsoletes(&self, change_id: &ChangeId) -> HashSet<CommitId> {
        match self {
            EvolutionRef::Readonly(evolution) => evolution.non_obsoletes(change_id),
            EvolutionRef::Mutable(evolution) => evolution.non_obsoletes(change_id),
        }
    }
}

pub struct ReadonlyEvolution {
    state: State,
}

impl ReadonlyEvolution {
    pub fn new(repo: &ReadonlyRepo) -> ReadonlyEvolution {
        ReadonlyEvolution {
            state: State::calculate(repo.as_repo_ref()),
        }
    }

    pub fn start_modification(&self) -> MutableEvolution {
        MutableEvolution {
            state: self.state.clone(),
        }
    }

    pub fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId> {
        self.state.successors(commit_id)
    }

    pub fn is_obsolete(&self, commit_id: &CommitId) -> bool {
        self.state.is_obsolete(commit_id)
    }

    pub fn is_orphan(&self, commit_id: &CommitId) -> bool {
        self.state.is_orphan(commit_id)
    }

    pub fn is_divergent(&self, change_id: &ChangeId) -> bool {
        self.state.is_divergent(change_id)
    }

    pub fn non_obsoletes(&self, change_id: &ChangeId) -> HashSet<CommitId> {
        self.state.non_obsoletes(change_id)
    }

    pub fn new_parent(&self, repo: RepoRef, old_parent_id: &CommitId) -> HashSet<CommitId> {
        self.state.new_parent(repo, old_parent_id)
    }
}

pub struct MutableEvolution {
    state: State,
}

impl MutableEvolution {
    pub fn new(repo: &MutableRepo) -> MutableEvolution {
        MutableEvolution {
            state: State::calculate(repo.as_repo_ref()),
        }
    }

    pub fn successors(&self, commit_id: &CommitId) -> HashSet<CommitId> {
        self.state.successors(commit_id)
    }

    pub fn is_obsolete(&self, commit_id: &CommitId) -> bool {
        self.state.is_obsolete(commit_id)
    }

    pub fn is_orphan(&self, commit_id: &CommitId) -> bool {
        self.state.is_orphan(commit_id)
    }

    pub fn is_divergent(&self, change_id: &ChangeId) -> bool {
        self.state.is_divergent(change_id)
    }

    pub fn non_obsoletes(&self, change_id: &ChangeId) -> HashSet<CommitId> {
        self.state.non_obsoletes(change_id)
    }

    pub fn new_parent(&self, repo: RepoRef, old_parent_id: &CommitId) -> HashSet<CommitId> {
        self.state.new_parent(repo, old_parent_id)
    }

    pub fn add_commit(&mut self, commit: &Commit) {
        self.state.add_commit(commit);
    }

    pub fn freeze(self) -> ReadonlyEvolution {
        ReadonlyEvolution { state: self.state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_commit_data_initial() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        assert!(!state.is_obsolete(&initial_commit));
        assert!(!state.is_orphan(&initial_commit));
        assert!(!state.is_divergent(&initial_change));
    }

    #[test]
    fn add_commit_data_pruned() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], true);
        assert!(!state.is_obsolete(&initial_commit));
        assert!(!state.is_orphan(&initial_commit));
        assert!(!state.is_divergent(&initial_change));
    }

    #[test]
    fn add_commit_data_creating_orphan() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let orphan_commit1 = CommitId::from_hex("bbb111");
        let orphan_change1 = ChangeId::from_hex("bbb111");
        let orphan_commit2 = CommitId::from_hex("ccc111");
        let orphan_change2 = ChangeId::from_hex("ccc111");
        let obsolete_orphan_commit = CommitId::from_hex("ddd111");
        let obsolete_orphan_change = ChangeId::from_hex("ddd111");
        let pruned_orphan_commit = CommitId::from_hex("eee111");
        let rewritten_commit = CommitId::from_hex("aaa222");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &orphan_commit1,
            &orphan_change1,
            &[initial_commit.clone()],
            &[],
            false,
        );
        state.add_commit_data(
            &orphan_commit2,
            &orphan_change2,
            &[orphan_commit1.clone()],
            &[],
            false,
        );
        state.add_commit_data(
            &obsolete_orphan_commit,
            &obsolete_orphan_change,
            &[initial_commit.clone()],
            &[],
            false,
        );
        state.add_commit_data(
            &pruned_orphan_commit,
            &obsolete_orphan_change,
            &[initial_commit.clone()],
            &[obsolete_orphan_commit.clone()],
            true,
        );
        state.add_commit_data(
            &rewritten_commit,
            &initial_change,
            &[],
            &[initial_commit],
            false,
        );
        assert!(state.is_orphan(&orphan_commit1));
        assert!(state.is_orphan(&orphan_commit2));
        assert!(!state.is_orphan(&obsolete_orphan_commit));
        assert!(!state.is_orphan(&pruned_orphan_commit));
        assert!(!state.is_obsolete(&orphan_commit1));
        assert!(!state.is_obsolete(&orphan_commit2));
        assert!(state.is_obsolete(&obsolete_orphan_commit));
        assert!(!state.is_obsolete(&pruned_orphan_commit));
    }

    #[test]
    fn add_commit_data_new_commit_on_obsolete() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_commit = CommitId::from_hex("aaa222");
        let new_commit = CommitId::from_hex("bbb111");
        let new_change = ChangeId::from_hex("bbb111");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &rewritten_commit,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        state.add_commit_data(&new_commit, &new_change, &[initial_commit], &[], false);
        assert!(state.is_orphan(&new_commit));
    }

    #[test]
    fn add_commit_data_new_commit_on_orphan() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_commit = CommitId::from_hex("aaa222");
        let orphan_commit = CommitId::from_hex("bbb111");
        let orphan_change = ChangeId::from_hex("bbb111");
        let new_commit = CommitId::from_hex("bbb111");
        let new_change = ChangeId::from_hex("bbb111");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &rewritten_commit,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        state.add_commit_data(
            &orphan_commit,
            &orphan_change,
            &[initial_commit],
            &[],
            false,
        );
        state.add_commit_data(&new_commit, &new_change, &[orphan_commit], &[], false);
        assert!(state.is_orphan(&new_commit));
    }

    #[test]
    fn add_commit_data_new_commit_on_pruned() {
        let mut state = State::default();

        let pruned_commit = CommitId::from_hex("aaa111");
        let pruned_change = ChangeId::from_hex("aaa111");
        let new_commit = CommitId::from_hex("bbb111");
        let new_change = ChangeId::from_hex("bbb111");

        state.add_commit_data(&pruned_commit, &pruned_change, &[], &[], true);
        state.add_commit_data(&new_commit, &new_change, &[pruned_commit], &[], false);
        assert!(state.is_orphan(&new_commit));
    }

    #[test]
    fn add_commit_data_rewrite_as_child() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_commit = CommitId::from_hex("aaa222");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        // The new commit is both a child and a successor of the initial commit
        state.add_commit_data(
            &rewritten_commit,
            &initial_change,
            &[initial_commit.clone()],
            &[initial_commit.clone()],
            false,
        );
        assert!(state.is_obsolete(&initial_commit));
        assert!(!state.is_obsolete(&rewritten_commit));
        assert!(!state.is_orphan(&initial_commit));
        assert!(state.is_orphan(&rewritten_commit));
        assert!(!state.is_divergent(&initial_change));
    }

    #[test]
    fn add_commit_data_duplicates() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let duplicate_commit1 = CommitId::from_hex("bbb111");
        let duplicate_change1 = ChangeId::from_hex("bbb111");
        let duplicate_commit2 = CommitId::from_hex("ccc111");
        let duplicate_change2 = ChangeId::from_hex("ccc111");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &duplicate_commit1,
            &duplicate_change1,
            &[],
            &[initial_commit.clone()],
            false,
        );
        state.add_commit_data(
            &duplicate_commit2,
            &duplicate_change2,
            &[],
            &[initial_commit.clone()],
            false,
        );
        assert!(!state.is_obsolete(&initial_commit));
        assert!(!state.is_obsolete(&duplicate_commit1));
        assert!(!state.is_obsolete(&duplicate_commit2));
        assert!(!state.is_divergent(&initial_change));
        assert!(!state.is_divergent(&duplicate_change1));
        assert!(!state.is_divergent(&duplicate_change2));
        assert_eq!(
            state.successors(&initial_commit),
            hashset!(duplicate_commit1, duplicate_commit2)
        );
    }

    #[test]
    fn add_commit_data_divergent() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_commit1 = CommitId::from_hex("aaa222");
        let rewritten_commit2 = CommitId::from_hex("aaa333");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &rewritten_commit1,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        state.add_commit_data(
            &rewritten_commit2,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        assert!(state.is_obsolete(&initial_commit));
        assert!(!state.is_obsolete(&rewritten_commit1));
        assert!(!state.is_obsolete(&rewritten_commit2));
        assert!(state.is_divergent(&initial_change));
        assert_eq!(
            state.successors(&initial_commit),
            hashset!(rewritten_commit1, rewritten_commit2)
        );
    }

    #[test]
    fn add_commit_data_divergent_pruned() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_pruned = CommitId::from_hex("aaa222");
        let rewritten_non_pruned = CommitId::from_hex("aaa333");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &rewritten_pruned,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            true,
        );
        state.add_commit_data(
            &rewritten_non_pruned,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        assert!(state.is_obsolete(&initial_commit));
        assert!(!state.is_obsolete(&rewritten_pruned));
        assert!(!state.is_obsolete(&rewritten_non_pruned));
        // It's still divergent even if one side is pruned
        assert!(state.is_divergent(&initial_change));
        assert_eq!(
            state.successors(&initial_commit),
            hashset!(rewritten_pruned, rewritten_non_pruned)
        );
    }

    #[test]
    fn add_commit_data_divergent_unrelated() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_commit = CommitId::from_hex("aaa222");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        // Same change id as the initial commit but no predecessor relationship to it
        state.add_commit_data(&rewritten_commit, &initial_change, &[], &[], false);
        assert!(!state.is_obsolete(&initial_commit));
        assert!(!state.is_obsolete(&rewritten_commit));
        assert!(state.is_divergent(&initial_change));
        assert_eq!(state.successors(&initial_commit), hashset!());
    }

    #[test]
    fn add_commit_data_divergent_convergent() {
        let mut state = State::default();

        let initial_commit = CommitId::from_hex("aaa111");
        let initial_change = ChangeId::from_hex("aaa111");
        let rewritten_commit1 = CommitId::from_hex("aaa222");
        let rewritten_commit2 = CommitId::from_hex("aaa333");
        let convergent_commit = CommitId::from_hex("aaa444");

        state.add_commit_data(&initial_commit, &initial_change, &[], &[], false);
        state.add_commit_data(
            &rewritten_commit1,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        state.add_commit_data(
            &rewritten_commit2,
            &initial_change,
            &[],
            &[initial_commit.clone()],
            false,
        );
        state.add_commit_data(
            &convergent_commit,
            &initial_change,
            &[],
            &[rewritten_commit1.clone(), rewritten_commit2.clone()],
            false,
        );
        assert!(state.is_obsolete(&initial_commit));
        assert!(state.is_obsolete(&rewritten_commit1));
        assert!(state.is_obsolete(&rewritten_commit2));
        assert!(!state.is_obsolete(&convergent_commit));
        assert!(!state.is_divergent(&initial_change));
        assert_eq!(
            state.successors(&rewritten_commit1),
            hashset!(convergent_commit.clone())
        );
        assert_eq!(
            state.successors(&rewritten_commit2),
            hashset!(convergent_commit)
        );
    }
}
