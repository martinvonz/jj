// Copyright 2020 The Jujutsu Authors
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

#![allow(missing_docs)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use itertools::Itertools;

use crate::backend::CommitId;
use crate::index::Index;
use crate::op_store;
use crate::op_store::{
    BranchTarget, RefTarget, RefTargetExt as _, RefTargetOptionExt as _, WorkspaceId,
};
use crate::refs::merge_ref_targets;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash, Debug)]
pub enum RefName {
    LocalBranch(String),
    RemoteBranch { branch: String, remote: String },
    Tag(String),
    GitRef(String),
}

impl fmt::Display for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefName::LocalBranch(name) => write!(f, "{name}"),
            RefName::RemoteBranch { branch, remote } => write!(f, "{branch}@{remote}"),
            RefName::Tag(name) => write!(f, "{name}"),
            RefName::GitRef(name) => write!(f, "{name}"),
        }
    }
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

    pub fn wc_commit_ids(&self) -> &HashMap<WorkspaceId, CommitId> {
        &self.data.wc_commit_ids
    }

    pub fn get_wc_commit_id(&self, workspace_id: &WorkspaceId) -> Option<&CommitId> {
        self.data.wc_commit_ids.get(workspace_id)
    }

    pub fn workspaces_for_wc_commit_id(&self, commit_id: &CommitId) -> Vec<WorkspaceId> {
        let mut workspaces_ids = vec![];
        for (workspace_id, wc_commit_id) in &self.data.wc_commit_ids {
            if wc_commit_id == commit_id {
                workspaces_ids.push(workspace_id.clone());
            }
        }
        workspaces_ids
    }

    pub fn is_wc_commit_id(&self, commit_id: &CommitId) -> bool {
        self.data.wc_commit_ids.values().contains(commit_id)
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

    pub fn tags(&self) -> &BTreeMap<String, Option<RefTarget>> {
        &self.data.tags
    }

    pub fn git_refs(&self) -> &BTreeMap<String, Option<RefTarget>> {
        &self.data.git_refs
    }

    pub fn git_head(&self) -> &Option<RefTarget> {
        &self.data.git_head
    }

    pub fn set_wc_commit(&mut self, workspace_id: WorkspaceId, commit_id: CommitId) {
        self.data.wc_commit_ids.insert(workspace_id, commit_id);
    }

    pub fn remove_wc_commit(&mut self, workspace_id: &WorkspaceId) {
        self.data.wc_commit_ids.remove(workspace_id);
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
            RefName::GitRef(name) => self.get_git_ref(name),
        }
    }

    /// Sets reference of the specified kind to point to the given target. If
    /// the target is absent, the reference will be removed.
    pub fn set_ref_target(&mut self, name: &RefName, target: Option<RefTarget>) {
        match name {
            RefName::LocalBranch(name) => self.set_local_branch_target(name, target),
            RefName::RemoteBranch { branch, remote } => {
                self.set_remote_branch_target(branch, remote, target)
            }
            RefName::Tag(name) => self.set_tag_target(name, target),
            RefName::GitRef(name) => self.set_git_ref_target(name, target),
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

    /// Sets local branch to point to the given target. If the target is absent,
    /// and if no associated remote branches exist, the branch will be removed.
    pub fn set_local_branch_target(&mut self, name: &str, target: Option<RefTarget>) {
        if target.is_present() {
            self.insert_local_branch(name.to_owned(), target);
        } else {
            self.remove_local_branch(name);
        }
    }

    fn insert_local_branch(&mut self, name: String, target: Option<RefTarget>) {
        assert!(target.is_present());
        self.data.branches.entry(name).or_default().local_target = target;
    }

    fn remove_local_branch(&mut self, name: &str) {
        if let Some(branch) = self.data.branches.get_mut(name) {
            branch.local_target = RefTarget::absent();
            if branch.remote_targets.is_empty() {
                self.remove_branch(name);
            }
        }
    }

    pub fn get_remote_branch(&self, name: &str, remote_name: &str) -> Option<RefTarget> {
        self.data.branches.get(name).and_then(|branch_target| {
            branch_target
                .remote_targets
                .get(remote_name)
                .flatten()
                .clone()
        })
    }

    /// Sets remote-tracking branch to point to the given target. If the target
    /// is absent, the branch will be removed.
    pub fn set_remote_branch_target(
        &mut self,
        name: &str,
        remote_name: &str,
        target: Option<RefTarget>,
    ) {
        if target.is_present() {
            self.insert_remote_branch(name.to_owned(), remote_name.to_owned(), target);
        } else {
            self.remove_remote_branch(name, remote_name);
        }
    }

    fn insert_remote_branch(
        &mut self,
        name: String,
        remote_name: String,
        target: Option<RefTarget>,
    ) {
        assert!(target.is_present());
        self.data
            .branches
            .entry(name)
            .or_default()
            .remote_targets
            .insert(remote_name, target);
    }

    fn remove_remote_branch(&mut self, name: &str, remote_name: &str) {
        if let Some(branch) = self.data.branches.get_mut(name) {
            branch.remote_targets.remove(remote_name);
            if branch.remote_targets.is_empty() && branch.local_target.is_absent() {
                self.remove_branch(name);
            }
        }
    }

    pub fn rename_remote(&mut self, old: &str, new: &str) {
        for branch in self.data.branches.values_mut() {
            let target = branch.remote_targets.remove(old).flatten();
            if target.is_present() {
                branch.remote_targets.insert(new.to_owned(), target);
            }
        }
    }

    pub fn get_tag(&self, name: &str) -> Option<RefTarget> {
        self.data.tags.get(name).flatten().clone()
    }

    /// Sets tag to point to the given target. If the target is absent, the tag
    /// will be removed.
    pub fn set_tag_target(&mut self, name: &str, target: Option<RefTarget>) {
        if target.is_present() {
            self.data.tags.insert(name.to_owned(), target);
        } else {
            self.data.tags.remove(name);
        }
    }

    pub fn get_git_ref(&self, name: &str) -> Option<RefTarget> {
        self.data.git_refs.get(name).flatten().clone()
    }

    /// Sets the last imported Git ref to point to the given target. If the
    /// target is absent, the reference will be removed.
    pub fn set_git_ref_target(&mut self, name: &str, target: Option<RefTarget>) {
        if target.is_present() {
            self.data.git_refs.insert(name.to_owned(), target);
        } else {
            self.data.git_refs.remove(name);
        }
    }

    /// Sets `HEAD@git` to point to the given target. If the target is absent,
    /// the reference will be cleared.
    pub fn set_git_head_target(&mut self, target: Option<RefTarget>) {
        self.data.git_head = target;
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

    pub fn merge_single_ref(
        &mut self,
        index: &dyn Index,
        ref_name: &RefName,
        base_target: Option<&RefTarget>,
        other_target: Option<&RefTarget>,
    ) {
        if base_target != other_target {
            let self_target = self.get_ref(ref_name);
            let new_target =
                merge_ref_targets(index, self_target.as_ref(), base_target, other_target);
            if new_target != self_target {
                self.set_ref_target(ref_name, new_target);
            }
        }
    }
}
