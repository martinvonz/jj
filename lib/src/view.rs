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

use std::collections::{BTreeMap, HashSet};

use crate::op_store;
use crate::store::CommitId;

pub struct View {
    data: op_store::View,
}

// TODO: Make a member of View?
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

impl View {
    pub fn new(op_store_view: op_store::View) -> Self {
        View {
            data: op_store_view,
        }
    }

    pub fn start_modification(&self) -> View {
        // TODO: Avoid the cloning of the sets here.
        View {
            data: self.data.clone(),
        }
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
