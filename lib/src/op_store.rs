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
use std::fmt::{Debug, Error, Formatter};

use once_cell::sync::Lazy;
use thiserror::Error;

use crate::backend::{id_type, CommitId, ObjectId, Timestamp};
use crate::conflicts::Merge;

content_hash! {
    #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
    pub struct WorkspaceId(String);
}

impl Debug for WorkspaceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("WorkspaceId").field(&self.0).finish()
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self("default".to_string())
    }
}

impl WorkspaceId {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

id_type!(pub ViewId);
id_type!(pub OperationId);

content_hash! {
    #[derive(PartialEq, Eq, Hash, Clone, Debug)]
    pub struct RefTarget {
        merge: Merge<Option<CommitId>>,
    }
}

impl Default for RefTarget {
    fn default() -> Self {
        Self::absent()
    }
}

impl RefTarget {
    /// Creates non-conflicting target pointing to no commit.
    pub fn absent() -> Self {
        Self::from_merge(Merge::resolved(None))
    }

    /// Returns non-conflicting target pointing to no commit.
    ///
    /// This will typically be used in place of `None` returned by map lookup.
    pub fn absent_ref() -> &'static Self {
        static TARGET: Lazy<RefTarget> = Lazy::new(RefTarget::absent);
        &TARGET
    }

    /// Creates non-conflicting target pointing to a commit.
    pub fn normal(id: CommitId) -> Self {
        Self::from_merge(Merge::resolved(Some(id)))
    }

    /// Creates target from removed/added ids.
    pub fn from_legacy_form(
        removed_ids: impl IntoIterator<Item = CommitId>,
        added_ids: impl IntoIterator<Item = CommitId>,
    ) -> Self {
        Self::from_merge(Merge::from_legacy_form(removed_ids, added_ids))
    }

    pub fn from_merge(merge: Merge<Option<CommitId>>) -> Self {
        RefTarget { merge }
    }

    /// Returns id if this target is non-conflicting and points to a commit.
    pub fn as_normal(&self) -> Option<&CommitId> {
        let maybe_id = self.merge.as_resolved()?;
        maybe_id.as_ref()
    }

    /// Returns true if this target points to no commit.
    pub fn is_absent(&self) -> bool {
        matches!(self.merge.as_resolved(), Some(None))
    }

    /// Returns true if this target points to any commit. Conflicting target is
    /// always "present" as it should have at least one commit id.
    pub fn is_present(&self) -> bool {
        !self.is_absent()
    }

    /// Whether this target has conflicts.
    pub fn has_conflict(&self) -> bool {
        !self.merge.is_resolved()
    }

    pub fn removed_ids(&self) -> impl Iterator<Item = &CommitId> {
        self.merge.removes().iter().flatten()
    }

    pub fn added_ids(&self) -> impl Iterator<Item = &CommitId> {
        self.merge.adds().iter().flatten()
    }

    pub fn as_conflict(&self) -> &Merge<Option<CommitId>> {
        &self.merge
    }
}

/// Helper to strip redundant `Option<T>` from `RefTarget` lookup result.
pub trait RefTargetOptionExt {
    type Value;

    fn flatten(self) -> Self::Value;
}

impl RefTargetOptionExt for Option<RefTarget> {
    type Value = RefTarget;

    fn flatten(self) -> Self::Value {
        self.unwrap_or_else(RefTarget::absent)
    }
}

impl<'a> RefTargetOptionExt for Option<&'a RefTarget> {
    type Value = &'a RefTarget;

    fn flatten(self) -> Self::Value {
        self.unwrap_or_else(|| RefTarget::absent_ref())
    }
}

content_hash! {
    #[derive(Default, PartialEq, Eq, Clone, Debug)]
    pub struct BranchTarget {
        /// The commit the branch points to locally. `None` if the branch has been
        /// deleted locally.
        pub local_target: RefTarget,
        // TODO: Do we need to support tombstones for remote branches? For example, if the branch
        // has been deleted locally and you pull from a remote, maybe it should make a difference
        // whether the branch is known to have existed on the remote. We may not want to resurrect
        // the branch if the branch's state on the remote was just not known.
        pub remote_targets: BTreeMap<String, RefTarget>,
    }
}

content_hash! {
    /// Represents the way the repo looks at a given time, just like how a Tree
    /// object represents how the file system looks at a given time.
    #[derive(PartialEq, Eq, Clone, Debug, Default)]
    pub struct View {
        /// All head commits
        pub head_ids: HashSet<CommitId>,
        /// Heads of the set of public commits.
        pub public_head_ids: HashSet<CommitId>,
        pub branches: BTreeMap<String, BranchTarget>,
        pub tags: BTreeMap<String, RefTarget>,
        pub git_refs: BTreeMap<String, RefTarget>,
        /// The commit the Git HEAD points to.
        // TODO: Support multiple Git worktrees?
        // TODO: Do we want to store the current branch name too?
        pub git_head: RefTarget,
        // The commit that *should be* checked out in the workspace. Note that the working copy
        // (.jj/working_copy/) has the source of truth about which commit *is* checked out (to be
        // precise: the commit to which we most recently completed an update to).
        pub wc_commit_ids: HashMap<WorkspaceId, CommitId>,
    }
}

content_hash! {
    /// Represents an operation (transaction) on the repo view, just like how a
    /// Commit object represents an operation on the tree.
    ///
    /// Operations and views are not meant to be exchanged between repos or users;
    /// they represent local state and history.
    ///
    /// The operation history will almost always be linear. It will only have
    /// forks when parallel operations occurred. The parent is determined when
    /// the transaction starts. When the transaction commits, a lock will be
    /// taken and it will be checked that the current head of the operation
    /// graph is unchanged. If the current head has changed, there has been
    /// concurrent operation.
    #[derive(PartialEq, Eq, Clone, Debug)]
    pub struct Operation {
        pub view_id: ViewId,
        pub parents: Vec<OperationId>,
        pub metadata: OperationMetadata,
    }
}

content_hash! {
    #[derive(PartialEq, Eq, Clone, Debug)]
    pub struct OperationMetadata {
        pub start_time: Timestamp,
        pub end_time: Timestamp,
        // Whatever is useful to the user, such as exact command line call
        pub description: String,
        pub hostname: String,
        pub username: String,
        pub tags: HashMap<String, String>,
    }
}

#[derive(Debug, Error)]
pub enum OpStoreError {
    #[error("Operation not found")]
    NotFound,
    #[error("Error when reading object {hash} of type {object_type}: {source}")]
    ReadObject {
        object_type: String,
        hash: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Could not write object of type {object_type}: {source}")]
    WriteObject {
        object_type: &'static str,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

pub type OpStoreResult<T> = Result<T, OpStoreError>;

pub trait OpStore: Send + Sync + Debug {
    fn name(&self) -> &str;

    fn read_view(&self, id: &ViewId) -> OpStoreResult<View>;

    fn write_view(&self, contents: &View) -> OpStoreResult<ViewId>;

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation>;

    fn write_operation(&self, contents: &Operation) -> OpStoreResult<OperationId>;
}
