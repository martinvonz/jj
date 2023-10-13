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
use crate::op_store::{BranchTarget, RefTarget, RefTargetOptionExt as _, RemoteRef, WorkspaceId};
use crate::refs::{iter_named_ref_pairs, merge_ref_targets, TrackingRefPair};

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

    /// Iterates pair of local and remote branches by branch name.
    pub fn branches(&self) -> impl Iterator<Item = (&str, BranchTarget<'_>)> {
        op_store::merge_join_branch_views(&self.data.local_branches, &self.data.remote_views)
    }

    pub fn tags(&self) -> &BTreeMap<String, RefTarget> {
        &self.data.tags
    }

    pub fn git_refs(&self) -> &BTreeMap<String, RefTarget> {
        &self.data.git_refs
    }

    pub fn git_head(&self) -> &RefTarget {
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

    pub fn get_ref(&self, name: &RefName) -> &RefTarget {
        match &name {
            RefName::LocalBranch(name) => self.get_local_branch(name),
            RefName::RemoteBranch { branch, remote } => {
                &self.get_remote_branch(branch, remote).target
            }
            RefName::Tag(name) => self.get_tag(name),
            RefName::GitRef(name) => self.get_git_ref(name),
        }
    }

    /// Sets reference of the specified kind to point to the given target. If
    /// the target is absent, the reference will be removed.
    pub fn set_ref_target(&mut self, name: &RefName, target: RefTarget) {
        match name {
            RefName::LocalBranch(name) => self.set_local_branch_target(name, target),
            RefName::RemoteBranch { branch, remote } => {
                self.set_remote_branch_target(branch, remote, target)
            }
            RefName::Tag(name) => self.set_tag_target(name, target),
            RefName::GitRef(name) => self.set_git_ref_target(name, target),
        }
    }

    /// Returns true if any local or remote branch of the given `name` exists.
    #[must_use]
    pub fn has_branch(&self, name: &str) -> bool {
        self.data.local_branches.contains_key(name)
            || self
                .data
                .remote_views
                .values()
                .any(|remote_view| remote_view.branches.contains_key(name))
    }

    // TODO: maybe rename to forget_branch() because this seems unusual operation?
    pub fn remove_branch(&mut self, name: &str) {
        self.data.local_branches.remove(name);
        for remote_view in self.data.remote_views.values_mut() {
            remote_view.branches.remove(name);
        }
    }

    /// Iterates local branch `(name, target)`s in lexicographical order.
    pub fn local_branches(&self) -> impl Iterator<Item = (&str, &RefTarget)> {
        self.data
            .local_branches
            .iter()
            .map(|(name, target)| (name.as_ref(), target))
    }

    pub fn get_local_branch(&self, name: &str) -> &RefTarget {
        self.data.local_branches.get(name).flatten()
    }

    /// Sets local branch to point to the given target. If the target is absent,
    /// and if no associated remote branches exist, the branch will be removed.
    pub fn set_local_branch_target(&mut self, name: &str, target: RefTarget) {
        if target.is_present() {
            self.data.local_branches.insert(name.to_owned(), target);
        } else {
            self.data.local_branches.remove(name);
        }
    }

    /// Iterates remote branch `((name, remote_name), remote_ref)`s in
    /// lexicographical order.
    pub fn all_remote_branches(&self) -> impl Iterator<Item = ((&str, &str), &RemoteRef)> {
        op_store::flatten_remote_branches(&self.data.remote_views)
    }

    /// Iterates branch `(name, remote_ref)`s of the specified remote in
    /// lexicographical order.
    pub fn remote_branches(&self, remote_name: &str) -> impl Iterator<Item = (&str, &RemoteRef)> {
        let maybe_remote_view = self.data.remote_views.get(remote_name);
        maybe_remote_view
            .map(|remote_view| {
                remote_view
                    .branches
                    .iter()
                    .map(|(name, remote_ref)| (name.as_ref(), remote_ref))
            })
            .into_iter()
            .flatten()
    }

    pub fn get_remote_branch(&self, name: &str, remote_name: &str) -> &RemoteRef {
        if let Some(remote_view) = self.data.remote_views.get(remote_name) {
            remote_view.branches.get(name).flatten()
        } else {
            RemoteRef::absent_ref()
        }
    }

    /// Sets remote-tracking branch to the given target and state. If the target
    /// is absent, the branch will be removed.
    pub fn set_remote_branch(&mut self, name: &str, remote_name: &str, remote_ref: RemoteRef) {
        if remote_ref.is_present() {
            let remote_view = self
                .data
                .remote_views
                .entry(remote_name.to_owned())
                .or_default();
            remote_view.branches.insert(name.to_owned(), remote_ref);
        } else if let Some(remote_view) = self.data.remote_views.get_mut(remote_name) {
            remote_view.branches.remove(name);
        }
    }

    /// Sets remote-tracking branch to point to the given target. If the target
    /// is absent, the branch will be removed.
    ///
    /// If the branch already exists, its tracking state won't be changed.
    fn set_remote_branch_target(&mut self, name: &str, remote_name: &str, target: RefTarget) {
        if target.is_present() {
            let remote_view = self
                .data
                .remote_views
                .entry(remote_name.to_owned())
                .or_default();
            if let Some(remote_ref) = remote_view.branches.get_mut(name) {
                remote_ref.target = target;
            } else {
                let remote_ref = RemoteRef { target };
                remote_view.branches.insert(name.to_owned(), remote_ref);
            }
        } else if let Some(remote_view) = self.data.remote_views.get_mut(remote_name) {
            remote_view.branches.remove(name);
        }
    }

    /// Iterates local/remote branch `(name, targets)`s of the specified remote
    /// in lexicographical order.
    pub fn local_remote_branches<'a>(
        &'a self,
        remote_name: &str,
    ) -> impl Iterator<Item = (&'a str, TrackingRefPair<'a>)> + 'a {
        // TODO: maybe untracked remote target can be translated to absent, and rename
        // the method accordingly.
        iter_named_ref_pairs(
            self.local_branches(),
            self.remote_branches(remote_name)
                .map(|(name, remote_ref)| (name, &remote_ref.target)),
        )
        .map(|(name, (local_target, remote_target))| {
            let targets = TrackingRefPair {
                local_target,
                remote_target,
            };
            (name, targets)
        })
    }

    pub fn remove_remote(&mut self, remote_name: &str) {
        self.data.remote_views.remove(remote_name);
    }

    pub fn rename_remote(&mut self, old: &str, new: &str) {
        if let Some(remote_view) = self.data.remote_views.remove(old) {
            self.data.remote_views.insert(new.to_owned(), remote_view);
        }
    }

    pub fn get_tag(&self, name: &str) -> &RefTarget {
        self.data.tags.get(name).flatten()
    }

    /// Sets tag to point to the given target. If the target is absent, the tag
    /// will be removed.
    pub fn set_tag_target(&mut self, name: &str, target: RefTarget) {
        if target.is_present() {
            self.data.tags.insert(name.to_owned(), target);
        } else {
            self.data.tags.remove(name);
        }
    }

    pub fn get_git_ref(&self, name: &str) -> &RefTarget {
        self.data.git_refs.get(name).flatten()
    }

    /// Sets the last imported Git ref to point to the given target. If the
    /// target is absent, the reference will be removed.
    pub fn set_git_ref_target(&mut self, name: &str, target: RefTarget) {
        if target.is_present() {
            self.data.git_refs.insert(name.to_owned(), target);
        } else {
            self.data.git_refs.remove(name);
        }
    }

    /// Sets `HEAD@git` to point to the given target. If the target is absent,
    /// the reference will be cleared.
    pub fn set_git_head_target(&mut self, target: RefTarget) {
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
        base_target: &RefTarget,
        other_target: &RefTarget,
    ) {
        if base_target != other_target {
            let self_target = self.get_ref(ref_name);
            let new_target = merge_ref_targets(index, self_target, base_target, other_target);
            if new_target != *self_target {
                self.set_ref_target(ref_name, new_target);
            }
        }
    }
}
