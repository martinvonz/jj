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
    data: op_store::View,
}

pub struct MutableView {
    data: op_store::View,
}

pub(crate) fn heads_of_set(
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

// TODO: Make a member of MutableView?
pub(crate) fn merge_views(
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
    pub fn new(op_store_view: op_store::View) -> Self {
        ReadonlyView {
            data: op_store_view,
        }
    }

    pub fn start_modification(&self) -> MutableView {
        // TODO: Avoid the cloning of the sets here.
        MutableView {
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

    pub fn store_view(&self) -> &op_store::View {
        &self.data
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

    pub fn add_head(&mut self, head_id: &CommitId) {
        self.data.head_ids.insert(head_id.clone());
    }

    pub fn remove_head(&mut self, head_id: &CommitId) {
        self.data.head_ids.remove(head_id);
    }

    pub fn add_public_head(&mut self, head_id: &CommitId) {
        self.data.public_head_ids.insert(head_id.clone());
    }

    pub fn remove_public_head(&mut self, head_id: &CommitId) {
        self.data.public_head_ids.remove(head_id);
    }

    pub fn insert_git_ref(&mut self, name: String, commit_id: CommitId) {
        self.data.git_refs.insert(name, commit_id);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        self.data.git_refs.remove(name);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        self.data = data;
    }

    pub fn store_view(&self) -> &op_store::View {
        &self.data
    }

    pub fn store_view_mut(&mut self) -> &mut op_store::View {
        &mut self.data
    }
}
