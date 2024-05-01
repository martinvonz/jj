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

use std::collections::hash_map::IntoIter;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::{AddAssign, Deref, DerefMut};

use itertools::Itertools;

use crate::backend::CommitId;
use crate::op_store::{BranchTarget, RefTarget, RefTargetOptionExt as _, RemoteRef, WorkspaceId};
use crate::refs::LocalAndRemoteRef;
use crate::str_util::StringPattern;
use crate::{op_store, refs};

/// A wrapper around [`op_store::View`] that defines additional methods.
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

    /// Iterates local branches `(name, target)` in lexicographical order where
    /// the target adds `commit_id`.
    pub fn local_branches_for_commit<'a: 'b, 'b>(
        &'a self,
        commit_id: &'b CommitId,
    ) -> impl Iterator<Item = (&'a str, &'a RefTarget)> + 'b {
        self.local_branches()
            .filter(|(_, target)| target.added_ids().contains(commit_id))
    }

    /// Iterates local branch `(name, target)`s matching the given pattern.
    /// Entries are sorted by `name`.
    pub fn local_branches_matching<'a: 'b, 'b>(
        &'a self,
        pattern: &'b StringPattern,
    ) -> impl Iterator<Item = (&'a str, &'a RefTarget)> + 'b {
        pattern
            .filter_btree_map(&self.data.local_branches)
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

    /// Iterates over `((name, remote_name), remote_ref)` for all remote
    /// branches in lexicographical order.
    pub fn all_remote_branches(&self) -> impl Iterator<Item = ((&str, &str), &RemoteRef)> {
        op_store::flatten_remote_branches(&self.data.remote_views)
    }

    /// Iterates over `(name, remote_ref)`s for all remote branches of the
    /// specified remote in lexicographical order.
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

    /// Iterates over `(name, remote_ref)`s for all remote branches of the
    /// specified remote that match the given pattern.
    ///
    /// Entries are sorted by `(name, remote_name)`.
    pub fn remote_branches_matching<'a: 'b, 'b>(
        &'a self,
        branch_pattern: &'b StringPattern,
        remote_pattern: &'b StringPattern,
    ) -> impl Iterator<Item = ((&'a str, &'a str), &'a RemoteRef)> + 'b {
        // Use kmerge instead of flat_map for consistency with all_remote_branches().
        remote_pattern
            .filter_btree_map(&self.data.remote_views)
            .map(|(remote_name, remote_view)| {
                branch_pattern.filter_btree_map(&remote_view.branches).map(
                    |(branch_name, remote_ref)| {
                        let full_name = (branch_name.as_ref(), remote_name.as_ref());
                        (full_name, remote_ref)
                    },
                )
            })
            .kmerge_by(|(full_name1, _), (full_name2, _)| full_name1 < full_name2)
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

    /// Iterates over `(name, {local_ref, remote_ref})`s for every branch
    /// present locally and/or on the specified remote, in lexicographical
    /// order.
    ///
    /// Note that this does *not* take into account whether the local branch
    /// tracks the remote branch or not. Missing values are represented as
    /// RefTarget::absent_ref() or RemoteRef::absent_ref().
    pub fn local_remote_branches<'a>(
        &'a self,
        remote_name: &str,
    ) -> impl Iterator<Item = (&'a str, LocalAndRemoteRef<'a>)> + 'a {
        refs::iter_named_local_remote_refs(self.local_branches(), self.remote_branches(remote_name))
            .map(|(name, (local_target, remote_ref))| {
                let targets = LocalAndRemoteRef {
                    local_target,
                    remote_ref,
                };
                (name, targets)
            })
    }

    /// Iterates over `(name, TrackingRefPair {local_ref, remote_ref})`s for
    /// every branch with a name that matches the given pattern, and that is
    /// present locally and/or on the specified remote.
    ///
    /// Entries are sorted by `name`.
    ///
    /// Note that this does *not* take into account whether the local branch
    /// tracks the remote branch or not. Missing values are represented as
    /// RefTarget::absent_ref() or RemoteRef::absent_ref().
    pub fn local_remote_branches_matching<'a: 'b, 'b>(
        &'a self,
        branch_pattern: &'b StringPattern,
        remote_name: &str,
    ) -> impl Iterator<Item = (&'a str, LocalAndRemoteRef<'a>)> + 'b {
        // Change remote_name to StringPattern if needed, but merge-join adapter won't
        // be usable.
        let maybe_remote_view = self.data.remote_views.get(remote_name);
        refs::iter_named_local_remote_refs(
            branch_pattern.filter_btree_map(&self.data.local_branches),
            maybe_remote_view
                .map(|remote_view| branch_pattern.filter_btree_map(&remote_view.branches))
                .into_iter()
                .flatten(),
        )
        .map(|(name, (local_target, remote_ref))| {
            let targets = LocalAndRemoteRef {
                local_target,
                remote_ref,
            };
            (name.as_ref(), targets)
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

    #[must_use]
    pub fn has_topic(&self, name: &str) -> bool {
        self.data.topics.contains_key(name)
    }

    pub fn remove_topic(&mut self, name: &str) {
        self.data.topics.remove(name);
    }

    /// Iterates local topics `(name, commit_ids)`s in lexicographical order.
    pub fn topics(&self) -> &BTreeMap<String, HashSet<CommitId>> {
        &self.data.topics
    }

    /// Iterates topics `(name, commit_ids)`s containing the given commit id in
    /// lexicographical order
    pub fn topics_containing_commit<'a: 'b, 'b>(
        &'a self,
        commit_id: &'b CommitId,
    ) -> impl Iterator<Item = &String> + 'b {
        self.topics().iter().filter_map(|(topic, commit_ids)| {
            if commit_ids.contains(commit_id) {
                Some(topic)
            } else {
                None
            }
        })
    }

    /// Iterates topic `(name, commit_ids)`s matching the given pattern.
    /// Entries are sorted by `name`.
    pub fn topics_matching<'a: 'b, 'b>(
        &'a self,
        pattern: &'b StringPattern,
    ) -> impl Iterator<Item = (&'a String, &'a HashSet<CommitId>)> + 'b {
        pattern.filter_btree_map(&self.data.topics)
    }

    pub fn get_topic_commits(&self, name: &str) -> Option<&HashSet<CommitId>> {
        self.data.topics.get(name)
    }

    pub fn update_topics<S, I, F>(&mut self, topics: I, update: F) -> ViewTopicsDiff
    where
        S: AsRef<str> + ToString,
        I: IntoIterator<Item = S>,
        F: Fn(&str, &HashSet<CommitId>) -> Option<HashSet<CommitId>>,
    {
        let mut stats = ViewTopicsDiff::default();

        for topic in topics {
            let Some(changes) = self.update_topic(topic.as_ref(), &update) else {
                continue;
            };

            stats.insert(topic.to_string(), changes);
        }
        stats
    }

    pub fn update_existing_topics<F>(&mut self, update: F) -> ViewTopicsDiff
    where
        F: Fn(&str, &HashSet<CommitId>) -> Option<HashSet<CommitId>>,
    {
        let mut stats = ViewTopicsDiff::default();

        for topic in self.data.topics.keys().cloned().collect_vec() {
            let Some(changes) = self.update_topic(&topic, &update) else {
                continue;
            };

            stats.insert(topic.to_string(), changes);
        }
        stats
    }

    fn update_topic<S, F>(&mut self, name: S, update: &F) -> Option<TopicDiff>
    where
        S: AsRef<str> + ToString,
        F: Fn(&str, &HashSet<CommitId>) -> Option<HashSet<CommitId>>,
    {
        let before = self.data.topics.get(name.as_ref());
        let after = update(name.as_ref(), before.unwrap_or(&HashSet::new()))?;
        let changes = TopicDiff::new(before.unwrap_or(&Default::default()), &after);
        if changes.is_empty() {
            return None;
        }

        self.set_topic_commits(name.to_string(), after);
        Some(changes)
    }

    /// Sets topic to contain to the given commit ids.
    /// If empty the topic will be removed.
    fn set_topic_commits(&mut self, name: String, commit_ids: HashSet<CommitId>) {
        if commit_ids.is_empty() {
            self.data.topics.remove(&name);
        } else {
            self.data.topics.insert(name.to_string(), commit_ids);
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

    /// Iterates all commit ids referenced by this view.
    ///
    /// This can include hidden commits referenced by remote branches, previous
    /// positions of conflicted branches, etc. The ancestors and predecessors of
    /// the returned commits should be considered reachable from the view. Use
    /// this to build commit index from scratch.
    ///
    /// The iteration order is unspecified, and may include duplicated entries.
    pub fn all_referenced_commit_ids(&self) -> impl Iterator<Item = &CommitId> {
        // Include both added/removed ids since ancestry information of old
        // references will be needed while merging views.
        fn ref_target_ids(target: &RefTarget) -> impl Iterator<Item = &CommitId> {
            target.as_merge().iter().flatten()
        }

        // Some of the fields (e.g. wc_commit_ids) would be redundant, but let's
        // not be smart here. Callers will build a larger set of commits anyway.
        let op_store::View {
            head_ids,
            local_branches,
            tags,
            topics,
            remote_views,
            git_refs,
            git_head,
            wc_commit_ids,
        } = &self.data;
        itertools::chain!(
            head_ids,
            local_branches.values().flat_map(ref_target_ids),
            topics.values().flatten(),
            tags.values().flat_map(ref_target_ids),
            remote_views.values().flat_map(|remote_view| {
                let op_store::RemoteView { branches } = remote_view;
                branches
                    .values()
                    .flat_map(|remote_ref| ref_target_ids(&remote_ref.target))
            }),
            git_refs.values().flat_map(ref_target_ids),
            ref_target_ids(git_head),
            wc_commit_ids.values()
        )
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

#[derive(Clone, Debug, Default)]
pub struct ViewTopicsDiff(pub HashMap<String, TopicDiff>);

impl ViewTopicsDiff {
    pub fn affected(&self) -> HashSet<&CommitId> {
        self.iter().fold(HashSet::new(), |mut acc, (_, diff)| {
            acc.extend(&diff.added);
            acc.extend(&diff.removed);
            acc
        })
    }
}

impl AddAssign for ViewTopicsDiff {
    fn add_assign(&mut self, rhs: Self) {
        for (topic, stats) in rhs.0 {
            *self.0.entry(topic).or_default() += stats;
        }
    }
}

impl Deref for ViewTopicsDiff {
    type Target = HashMap<String, TopicDiff>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ViewTopicsDiff {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl IntoIterator for ViewTopicsDiff {
    type Item = (String, TopicDiff);

    type IntoIter = IntoIter<String, TopicDiff>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(Clone, Debug, Default)]
pub struct TopicDiff {
    added: HashSet<CommitId>,
    removed: HashSet<CommitId>,
}

impl TopicDiff {
    pub fn len(&self) -> usize {
        self.added.len() + self.removed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    pub fn new(before: &HashSet<CommitId>, after: &HashSet<CommitId>) -> Self {
        Self {
            added: HashSet::from_iter(after.difference(before).cloned()),
            removed: HashSet::from_iter(before.difference(after).cloned()),
        }
    }

    pub fn added(&self) -> &HashSet<CommitId> {
        &self.added
    }

    pub fn removed(&self) -> &HashSet<CommitId> {
        &self.removed
    }
}

impl AddAssign for TopicDiff {
    fn add_assign(&mut self, rhs: Self) {
        for removed in &rhs.removed {
            self.added.remove(removed);
        }
        for added in &rhs.added {
            self.removed.remove(added);
        }
        self.added.extend(rhs.added);
        self.removed.extend(rhs.removed);
    }
}
