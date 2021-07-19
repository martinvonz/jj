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

use crate::index::IndexRef;
use crate::op_store;
use crate::op_store::RefTarget;
use crate::refs::merge_ref_targets;
use crate::store::CommitId;

pub struct View {
    data: op_store::View,
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

    pub fn git_refs(&self) -> &BTreeMap<String, RefTarget> {
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

    pub fn insert_git_ref(&mut self, name: String, target: RefTarget) {
        self.data.git_refs.insert(name, target);
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

    pub fn merge(&mut self, index: IndexRef, base: &View, other: &View) {
        if other.checkout() == base.checkout() || other.checkout() == self.checkout() {
            // Keep the self side
        } else if self.checkout() == base.checkout() {
            self.set_checkout(other.checkout().clone());
        } else {
            // TODO: Return an error here. Or should we just pick one of the
            // sides and emit a warning?
        }

        for removed_head in base.public_heads().difference(other.public_heads()) {
            self.remove_public_head(removed_head);
        }
        for added_head in other.public_heads().difference(base.public_heads()) {
            self.add_public_head(added_head);
        }

        for removed_head in base.heads().difference(other.heads()) {
            self.remove_head(removed_head);
        }
        for added_head in other.heads().difference(base.heads()) {
            self.add_head(added_head);
        }
        // TODO: Should it be considered a conflict if a commit-head is removed on one
        // side while a child or successor is created on another side? Maybe a
        // warning?

        // Merge git refs
        let base_git_ref_names: HashSet<_> = base.git_refs().keys().cloned().collect();
        let other_git_ref_names: HashSet<_> = other.git_refs().keys().cloned().collect();
        for maybe_modified_git_ref_name in other_git_ref_names.union(&base_git_ref_names) {
            let base_target = base.git_refs().get(maybe_modified_git_ref_name);
            let other_target = other.git_refs().get(maybe_modified_git_ref_name);
            if base_target == other_target {
                continue;
            }
            let self_target = self.git_refs().get(maybe_modified_git_ref_name);
            let merged_target = merge_ref_targets(index, self_target, base_target, other_target);
            match merged_target {
                None => {
                    self.remove_git_ref(maybe_modified_git_ref_name);
                }
                Some(target) => {
                    self.insert_git_ref(maybe_modified_git_ref_name.clone(), target);
                }
            }
        }
    }
}
