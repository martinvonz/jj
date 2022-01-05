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

use crate::backend::CommitId;
use crate::index::IndexRef;
use crate::op_store;
use crate::op_store::{BranchTarget, RefTarget};
use crate::refs::merge_ref_targets;

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum RefName {
    LocalBranch(String),
    RemoteBranch { branch: String, remote: String },
    Tag(String),
    GitRef(String),
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct View {
    data: op_store::View,
}

impl View {
    pub fn new(op_store_view: op_store::View) -> Self {
        View {
            data: op_store_view,
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

    pub fn branches(&self) -> &BTreeMap<String, BranchTarget> {
        &self.data.branches
    }

    pub fn tags(&self) -> &BTreeMap<String, RefTarget> {
        &self.data.tags
    }

    pub fn git_refs(&self) -> &BTreeMap<String, RefTarget> {
        &self.data.git_refs
    }

    pub fn git_head(&self) -> Option<CommitId> {
        self.data.git_head.clone()
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

    pub fn get_ref(&self, name: &RefName) -> Option<RefTarget> {
        match &name {
            RefName::LocalBranch(name) => self.get_local_branch(name),
            RefName::RemoteBranch { branch, remote } => self.get_remote_branch(branch, remote),
            RefName::Tag(name) => self.get_tag(name),
            RefName::GitRef(name) => self.git_refs().get(name).cloned(),
        }
    }

    pub fn set_or_remove_ref(&mut self, name: RefName, target: Option<RefTarget>) {
        if let Some(target) = target {
            match name {
                RefName::LocalBranch(name) => {
                    self.set_local_branch(name, target);
                }
                RefName::RemoteBranch { branch, remote } => {
                    self.set_remote_branch(branch, remote, target);
                }
                RefName::Tag(name) => {
                    self.set_tag(name, target);
                }
                RefName::GitRef(name) => {
                    self.set_git_ref(name, target);
                }
            }
        } else {
            match name {
                RefName::LocalBranch(name) => {
                    self.remove_local_branch(&name);
                }
                RefName::RemoteBranch { branch, remote } => {
                    self.remove_remote_branch(&branch, &remote);
                }
                RefName::Tag(name) => {
                    self.remove_tag(&name);
                }
                RefName::GitRef(name) => {
                    self.remove_git_ref(&name);
                }
            }
        }
    }

    pub fn get_branch(&self, name: &str) -> Option<&BranchTarget> {
        self.data.branches.get(name)
    }

    pub fn set_branch(&mut self, name: String, target: BranchTarget) {
        self.data.branches.insert(name, target);
    }

    pub fn remove_branch(&mut self, name: &str) {
        self.data.branches.remove(name);
    }

    pub fn get_local_branch(&self, name: &str) -> Option<RefTarget> {
        self.data
            .branches
            .get(name)
            .and_then(|branch_target| branch_target.local_target.clone())
    }

    pub fn set_local_branch(&mut self, name: String, target: RefTarget) {
        self.data.branches.entry(name).or_default().local_target = Some(target);
    }

    pub fn remove_local_branch(&mut self, name: &str) {
        if let Some(branch) = self.data.branches.get_mut(name) {
            branch.local_target = None;
            if branch.remote_targets.is_empty() {
                self.remove_branch(name);
            }
        }
    }

    pub fn get_remote_branch(&self, name: &str, remote_name: &str) -> Option<RefTarget> {
        self.data
            .branches
            .get(name)
            .and_then(|branch_target| branch_target.remote_targets.get(remote_name).cloned())
    }

    pub fn set_remote_branch(&mut self, name: String, remote_name: String, target: RefTarget) {
        self.data
            .branches
            .entry(name)
            .or_default()
            .remote_targets
            .insert(remote_name, target);
    }

    pub fn remove_remote_branch(&mut self, name: &str, remote_name: &str) {
        if let Some(branch) = self.data.branches.get_mut(name) {
            branch.remote_targets.remove(remote_name);
            if branch.remote_targets.is_empty() && branch.local_target.is_none() {
                self.remove_branch(name);
            }
        }
    }

    pub fn get_tag(&self, name: &str) -> Option<RefTarget> {
        self.data.tags.get(name).cloned()
    }

    pub fn set_tag(&mut self, name: String, target: RefTarget) {
        self.data.tags.insert(name, target);
    }

    pub fn remove_tag(&mut self, name: &str) {
        self.data.tags.remove(name);
    }

    pub fn set_git_ref(&mut self, name: String, target: RefTarget) {
        self.data.git_refs.insert(name, target);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        self.data.git_refs.remove(name);
    }

    pub fn set_git_head(&mut self, head_id: CommitId) {
        self.data.git_head = Some(head_id);
    }

    pub fn clear_git_head(&mut self) {
        self.data.git_head = None;
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

        let mut maybe_changed_ref_names = HashSet::new();

        let base_branches: HashSet<_> = base.branches().keys().cloned().collect();
        let other_branches: HashSet<_> = other.branches().keys().cloned().collect();
        for branch_name in base_branches.union(&other_branches) {
            let base_branch = base.branches().get(branch_name);
            let other_branch = other.branches().get(branch_name);
            if other_branch == base_branch {
                // Unchanged on other side
                continue;
            }

            maybe_changed_ref_names.insert(RefName::LocalBranch(branch_name.clone()));
            if let Some(branch) = base_branch {
                for remote in branch.remote_targets.keys() {
                    maybe_changed_ref_names.insert(RefName::RemoteBranch {
                        branch: branch_name.clone(),
                        remote: remote.clone(),
                    });
                }
            }
            if let Some(branch) = other_branch {
                for remote in branch.remote_targets.keys() {
                    maybe_changed_ref_names.insert(RefName::RemoteBranch {
                        branch: branch_name.clone(),
                        remote: remote.clone(),
                    });
                }
            }
        }

        for tag_name in base.tags().keys() {
            maybe_changed_ref_names.insert(RefName::Tag(tag_name.clone()));
        }
        for tag_name in other.tags().keys() {
            maybe_changed_ref_names.insert(RefName::Tag(tag_name.clone()));
        }

        for git_ref_name in base.git_refs().keys() {
            maybe_changed_ref_names.insert(RefName::GitRef(git_ref_name.clone()));
        }
        for git_ref_name in other.git_refs().keys() {
            maybe_changed_ref_names.insert(RefName::GitRef(git_ref_name.clone()));
        }

        for ref_name in maybe_changed_ref_names {
            let base_target = base.get_ref(&ref_name);
            let other_target = other.get_ref(&ref_name);
            self.merge_single_ref(
                index,
                &ref_name,
                base_target.as_ref(),
                other_target.as_ref(),
            );
        }
    }

    pub fn merge_single_ref(
        &mut self,
        index: IndexRef,
        ref_name: &RefName,
        base_target: Option<&RefTarget>,
        other_target: Option<&RefTarget>,
    ) {
        if base_target != other_target {
            let self_target = self.get_ref(ref_name);
            let new_target =
                merge_ref_targets(index, self_target.as_ref(), base_target, other_target);
            if new_target != self_target {
                self.set_or_remove_ref(ref_name.clone(), new_target);
            }
        }
    }
}
