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

use std::cmp::min;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::commit::Commit;
use crate::op_store;

use crate::store::CommitId;
use crate::store_wrapper::StoreWrapper;

pub enum ViewRef<'a> {
    Readonly(&'a ReadonlyView),
    Mutable(&'a MutableView),
}

impl<'a> ViewRef<'a> {
    pub fn checkout(&self) -> &'a CommitId {
        match self {
            ViewRef::Readonly(view) => view.checkout(),
            ViewRef::Mutable(view) => view.checkout(),
        }
    }

    pub fn heads(&self) -> &'a HashSet<CommitId> {
        match self {
            ViewRef::Readonly(view) => view.heads(),
            ViewRef::Mutable(view) => view.heads(),
        }
    }

    pub fn public_heads(&self) -> &'a HashSet<CommitId> {
        match self {
            ViewRef::Readonly(view) => view.public_heads(),
            ViewRef::Mutable(view) => view.public_heads(),
        }
    }

    pub fn git_refs(&self) -> &'a BTreeMap<String, CommitId> {
        match self {
            ViewRef::Readonly(view) => view.git_refs(),
            ViewRef::Mutable(view) => view.git_refs(),
        }
    }
}

pub struct ReadonlyView {
    store: Arc<StoreWrapper>,
    data: op_store::View,
}

pub struct MutableView {
    store: Arc<StoreWrapper>,
    data: op_store::View,
}

fn enforce_invariants(store: &StoreWrapper, view: &mut op_store::View) {
    // TODO: This is surely terribly slow on large repos, at least in its current
    // form. We should make it faster (using the index) and avoid calling it in
    // most cases (avoid adding a head that's already reachable in the view).
    view.public_head_ids = heads_of_set(store, view.public_head_ids.iter().cloned());
    view.head_ids.extend(view.public_head_ids.iter().cloned());
    view.head_ids.extend(view.git_refs.values().cloned());
    view.head_ids = heads_of_set(store, view.head_ids.iter().cloned());
}

fn heads_of_set(
    store: &StoreWrapper,
    commit_ids: impl Iterator<Item = CommitId>,
) -> HashSet<CommitId> {
    let mut visited = HashSet::new();
    let mut work = vec![];
    let mut oldest = u64::MAX;
    let mut heads: HashSet<CommitId> = commit_ids.collect();
    for commit_id in &heads {
        let commit = store.get_commit(commit_id).unwrap();
        oldest = min(oldest, commit.committer().timestamp.timestamp.0);
        work.push(commit);
    }
    // Assume clock skew less than a month:
    // TODO: use generation numbers here
    let threshold = oldest.saturating_sub(1000 * 3600 * 24 * 30);
    while !work.is_empty() {
        let commit = work.pop().unwrap();
        if visited.contains(commit.id()) {
            continue;
        }
        visited.insert(commit.id().clone());

        for parent in commit.parents() {
            if parent.committer().timestamp.timestamp.0 < threshold {
                continue;
            }
            heads.remove(parent.id());
            work.push(parent);
        }
    }
    heads
}

pub fn merge_views(
    store: &StoreWrapper,
    left: &op_store::View,
    base: &op_store::View,
    right: &op_store::View,
) -> op_store::View {
    let mut result = left.clone();
    if right.checkout == base.checkout || right.checkout == left.checkout {
        // Keep the left side
    } else if left.checkout == base.checkout {
        result.checkout = right.checkout.clone();
    } else {
        // TODO: Return an error here. Or should we just pick one of the sides
        // and emit a warning?
    }

    for removed_head in base.public_head_ids.difference(&right.public_head_ids) {
        result.public_head_ids.remove(removed_head);
    }
    for added_head in right.public_head_ids.difference(&base.public_head_ids) {
        result.public_head_ids.insert(added_head.clone());
    }

    for removed_head in base.head_ids.difference(&right.head_ids) {
        result.head_ids.remove(removed_head);
    }
    for added_head in right.head_ids.difference(&base.head_ids) {
        result.head_ids.insert(added_head.clone());
    }
    enforce_invariants(store, &mut result);
    // TODO: Should it be considered a conflict if a commit-head is removed on one
    // side while a child or successor is created on another side? Maybe a
    // warning?

    // Merge git refs
    let base_git_ref_names: HashSet<_> = base.git_refs.keys().clone().collect();
    let right_git_ref_names: HashSet<_> = right.git_refs.keys().clone().collect();
    for maybe_modified_git_ref_name in right_git_ref_names.intersection(&base_git_ref_names) {
        let base_commit_id = base.git_refs.get(*maybe_modified_git_ref_name).unwrap();
        let right_commit_id = right.git_refs.get(*maybe_modified_git_ref_name).unwrap();
        if base_commit_id == right_commit_id {
            continue;
        }
        // TODO: Handle modify/modify conflict (i.e. if left and base are different
        // here)
        result.git_refs.insert(
            (*maybe_modified_git_ref_name).clone(),
            right_commit_id.clone(),
        );
    }
    for added_git_ref_name in right_git_ref_names.difference(&base_git_ref_names) {
        // TODO: Handle add/add conflict (i.e. if left also has the ref here)
        result.git_refs.insert(
            (*added_git_ref_name).clone(),
            right.git_refs.get(*added_git_ref_name).unwrap().clone(),
        );
    }
    for removed_git_ref_name in base_git_ref_names.difference(&right_git_ref_names) {
        // TODO: Handle modify/remove conflict (i.e. if left and base are different
        // here)
        result.git_refs.remove(*removed_git_ref_name);
    }

    result
}

impl ReadonlyView {
    pub fn new(store: Arc<StoreWrapper>, op_store_view: op_store::View) -> Self {
        ReadonlyView {
            store,
            data: op_store_view,
        }
    }

    pub fn start_modification(&self) -> MutableView {
        // TODO: Avoid the cloning of the sets here.
        MutableView {
            store: self.store.clone(),
            data: self.data.clone(),
        }
    }

    pub fn as_view_ref(&self) -> ViewRef {
        ViewRef::Readonly(self)
    }

    pub fn checkout(&self) -> &CommitId {
        &self.data.checkout
    }

    pub fn heads(&self) -> &HashSet<CommitId> {
        &self.data.head_ids
    }

    pub fn public_heads(&self) -> &HashSet<CommitId> {
        &self.data.public_head_ids
    }

    pub fn git_refs(&self) -> &BTreeMap<String, CommitId> {
        &self.data.git_refs
    }
}

impl MutableView {
    pub fn as_view_ref(&self) -> ViewRef {
        ViewRef::Mutable(self)
    }

    pub fn checkout(&self) -> &CommitId {
        &self.data.checkout
    }

    pub fn heads(&self) -> &HashSet<CommitId> {
        &self.data.head_ids
    }

    pub fn public_heads(&self) -> &HashSet<CommitId> {
        &self.data.public_head_ids
    }

    pub fn git_refs(&self) -> &BTreeMap<String, CommitId> {
        &self.data.git_refs
    }

    pub fn set_checkout(&mut self, id: CommitId) {
        self.data.checkout = id;
    }

    pub fn add_head(&mut self, head: &Commit) {
        self.data.head_ids.insert(head.id().clone());
        for parent in head.parents() {
            self.data.head_ids.remove(parent.id());
        }
        enforce_invariants(&self.store, &mut self.data);
    }

    pub fn remove_head(&mut self, head: &Commit) {
        self.data.head_ids.remove(head.id());
        enforce_invariants(&self.store, &mut self.data);
    }

    pub fn add_public_head(&mut self, head: &Commit) {
        self.data.public_head_ids.insert(head.id().clone());
        enforce_invariants(&self.store, &mut self.data);
    }

    pub fn remove_public_head(&mut self, head: &Commit) {
        self.data.public_head_ids.remove(head.id());
    }

    pub fn insert_git_ref(&mut self, name: String, commit_id: CommitId) {
        self.data.git_refs.insert(name, commit_id);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        self.data.git_refs.remove(name);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        self.data = data;
        enforce_invariants(&self.store, &mut self.data);
    }

    pub fn store_view(&self) -> &op_store::View {
        &self.data
    }
}
