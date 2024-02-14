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

use std::any::Any;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Debug, Error, Formatter};
use std::iter;
use std::time::SystemTime;

use itertools::Itertools as _;
use once_cell::sync::Lazy;
use thiserror::Error;

use crate::backend::{CommitId, MillisSinceEpoch, Timestamp};
use crate::content_hash::ContentHash;
use crate::merge::Merge;
use crate::object_id::{id_type, HexPrefix, ObjectId, PrefixResolution};

#[derive(ContentHash, PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct WorkspaceId(String);

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

#[derive(ContentHash, PartialEq, Eq, Hash, Clone, Debug)]
pub struct RefTarget {
    merge: Merge<Option<CommitId>>,
}

impl Default for RefTarget {
    fn default() -> Self {
        Self::absent()
    }
}

impl RefTarget {
    /// Creates non-conflicting target pointing to no commit.
    pub fn absent() -> Self {
        Self::from_merge(Merge::absent())
    }

    /// Returns non-conflicting target pointing to no commit.
    ///
    /// This will typically be used in place of `None` returned by map lookup.
    pub fn absent_ref() -> &'static Self {
        static TARGET: Lazy<RefTarget> = Lazy::new(RefTarget::absent);
        &TARGET
    }

    /// Creates non-conflicting target that optionally points to a commit.
    pub fn resolved(maybe_id: Option<CommitId>) -> Self {
        Self::from_merge(Merge::resolved(maybe_id))
    }

    /// Creates non-conflicting target pointing to a commit.
    pub fn normal(id: CommitId) -> Self {
        Self::from_merge(Merge::normal(id))
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

    /// Returns the underlying value if this target is non-conflicting.
    pub fn as_resolved(&self) -> Option<&Option<CommitId>> {
        self.merge.as_resolved()
    }

    /// Returns id if this target is non-conflicting and points to a commit.
    pub fn as_normal(&self) -> Option<&CommitId> {
        self.merge.as_normal()
    }

    /// Returns true if this target points to no commit.
    pub fn is_absent(&self) -> bool {
        self.merge.is_absent()
    }

    /// Returns true if this target points to any commit. Conflicting target is
    /// always "present" as it should have at least one commit id.
    pub fn is_present(&self) -> bool {
        self.merge.is_present()
    }

    /// Whether this target has conflicts.
    pub fn has_conflict(&self) -> bool {
        !self.merge.is_resolved()
    }

    pub fn removed_ids(&self) -> impl Iterator<Item = &CommitId> {
        self.merge.removes().flatten()
    }

    pub fn added_ids(&self) -> impl Iterator<Item = &CommitId> {
        self.merge.adds().flatten()
    }

    pub fn as_merge(&self) -> &Merge<Option<CommitId>> {
        &self.merge
    }
}

/// Remote branch or tag.
#[derive(ContentHash, Clone, Debug, Eq, Hash, PartialEq)]
pub struct RemoteRef {
    pub target: RefTarget,
    pub state: RemoteRefState,
}

impl RemoteRef {
    /// Creates remote ref pointing to no commit.
    pub fn absent() -> Self {
        RemoteRef {
            target: RefTarget::absent(),
            state: RemoteRefState::New,
        }
    }

    /// Returns remote ref pointing to no commit.
    ///
    /// This will typically be used in place of `None` returned by map lookup.
    pub fn absent_ref() -> &'static Self {
        static TARGET: Lazy<RemoteRef> = Lazy::new(RemoteRef::absent);
        &TARGET
    }

    /// Returns true if the target points to no commit.
    pub fn is_absent(&self) -> bool {
        self.target.is_absent()
    }

    /// Returns true if the target points to any commit.
    pub fn is_present(&self) -> bool {
        self.target.is_present()
    }

    /// Returns true if the ref is supposed to be merged in to the local ref.
    pub fn is_tracking(&self) -> bool {
        self.state == RemoteRefState::Tracking
    }

    /// Target that should have been merged in to the local ref.
    ///
    /// Use this as the base or known target when merging new remote ref in to
    /// local or pushing local ref to remote.
    pub fn tracking_target(&self) -> &RefTarget {
        if self.is_tracking() {
            &self.target
        } else {
            RefTarget::absent_ref()
        }
    }
}

/// Whether the ref is tracked or not.
#[derive(ContentHash, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RemoteRefState {
    /// Remote ref is not merged in to the local ref.
    New,
    /// Remote ref has been merged in to the local ref. Incoming ref will be
    /// merged, too.
    Tracking,
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

impl RefTargetOptionExt for Option<RemoteRef> {
    type Value = RemoteRef;

    fn flatten(self) -> Self::Value {
        self.unwrap_or_else(RemoteRef::absent)
    }
}

impl<'a> RefTargetOptionExt for Option<&'a RemoteRef> {
    type Value = &'a RemoteRef;

    fn flatten(self) -> Self::Value {
        self.unwrap_or_else(|| RemoteRef::absent_ref())
    }
}

/// Local and remote branches of the same branch name.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct BranchTarget<'a> {
    /// The commit the branch points to locally.
    pub local_target: &'a RefTarget,
    /// `(remote_name, remote_ref)` pairs in lexicographical order.
    pub remote_refs: Vec<(&'a str, &'a RemoteRef)>,
}

/// Represents the way the repo looks at a given time, just like how a Tree
/// object represents how the file system looks at a given time.
#[derive(ContentHash, PartialEq, Eq, Clone, Debug, Default)]
pub struct View {
    /// All head commits
    pub head_ids: HashSet<CommitId>,
    pub local_branches: BTreeMap<String, RefTarget>,
    pub tags: BTreeMap<String, RefTarget>,
    pub remote_views: BTreeMap<String, RemoteView>,
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

/// Represents the state of the remote repo.
#[derive(ContentHash, Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteView {
    // TODO: Do we need to support tombstones for remote branches? For example, if the branch
    // has been deleted locally and you pull from a remote, maybe it should make a difference
    // whether the branch is known to have existed on the remote. We may not want to resurrect
    // the branch if the branch's state on the remote was just not known.
    pub branches: BTreeMap<String, RemoteRef>,
    // TODO: pub tags: BTreeMap<String, RemoteRef>,
}

/// Iterates pair of local and remote branches by branch name.
pub(crate) fn merge_join_branch_views<'a>(
    local_branches: &'a BTreeMap<String, RefTarget>,
    remote_views: &'a BTreeMap<String, RemoteView>,
) -> impl Iterator<Item = (&'a str, BranchTarget<'a>)> {
    let mut local_branches_iter = local_branches
        .iter()
        .map(|(branch_name, target)| (branch_name.as_str(), target))
        .peekable();
    let mut remote_branches_iter = flatten_remote_branches(remote_views).peekable();

    iter::from_fn(move || {
        // Pick earlier branch name
        let (branch_name, local_target) =
            if let Some(&((remote_branch_name, _), _)) = remote_branches_iter.peek() {
                local_branches_iter
                    .next_if(|&(local_branch_name, _)| local_branch_name <= remote_branch_name)
                    .unwrap_or((remote_branch_name, RefTarget::absent_ref()))
            } else {
                local_branches_iter.next()?
            };
        let remote_refs = remote_branches_iter
            .peeking_take_while(|&((remote_branch_name, _), _)| remote_branch_name == branch_name)
            .map(|((_, remote_name), remote_ref)| (remote_name, remote_ref))
            .collect();
        let branch_target = BranchTarget {
            local_target,
            remote_refs,
        };
        Some((branch_name, branch_target))
    })
}

/// Iterates branch `((name, remote_name), remote_ref)`s in lexicographical
/// order.
pub(crate) fn flatten_remote_branches(
    remote_views: &BTreeMap<String, RemoteView>,
) -> impl Iterator<Item = ((&str, &str), &RemoteRef)> {
    remote_views
        .iter()
        .map(|(remote_name, remote_view)| {
            remote_view
                .branches
                .iter()
                .map(move |(branch_name, remote_ref)| {
                    let full_name = (branch_name.as_str(), remote_name.as_str());
                    (full_name, remote_ref)
                })
        })
        .kmerge_by(|(full_name1, _), (full_name2, _)| full_name1 < full_name2)
}

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
#[derive(ContentHash, PartialEq, Eq, Clone, Debug)]
pub struct Operation {
    pub view_id: ViewId,
    pub parents: Vec<OperationId>,
    pub metadata: OperationMetadata,
}

impl Operation {
    pub fn make_root(empty_view_id: ViewId) -> Operation {
        let timestamp = Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        };
        let metadata = OperationMetadata {
            start_time: timestamp.clone(),
            end_time: timestamp,
            description: "".to_string(),
            hostname: "".to_string(),
            username: "".to_string(),
            is_snapshot: false,
            tags: HashMap::new(),
        };
        Operation {
            view_id: empty_view_id,
            parents: vec![],
            metadata,
        }
    }
}

#[derive(ContentHash, PartialEq, Eq, Clone, Debug)]
pub struct OperationMetadata {
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    // Whatever is useful to the user, such as exact command line call
    pub description: String,
    pub hostname: String,
    pub username: String,
    /// Whether this operation represents a pure snapshotting of the working
    /// copy.
    pub is_snapshot: bool,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Error)]
pub enum OpStoreError {
    #[error("Object {hash} of type {object_type} not found")]
    ObjectNotFound {
        object_type: String,
        hash: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Error when reading object {hash} of type {object_type}")]
    ReadObject {
        object_type: String,
        hash: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Could not write object of type {object_type}")]
    WriteObject {
        object_type: &'static str,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

pub type OpStoreResult<T> = Result<T, OpStoreError>;

pub trait OpStore: Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;

    fn name(&self) -> &str;

    fn root_operation_id(&self) -> &OperationId;

    fn read_view(&self, id: &ViewId) -> OpStoreResult<View>;

    fn write_view(&self, contents: &View) -> OpStoreResult<ViewId>;

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation>;

    fn write_operation(&self, contents: &Operation) -> OpStoreResult<OperationId>;

    /// Resolves an unambiguous operation ID prefix.
    fn resolve_operation_id_prefix(
        &self,
        prefix: &HexPrefix,
    ) -> OpStoreResult<PrefixResolution<OperationId>>;

    /// Prunes unreachable operations and views.
    ///
    /// All operations and views reachable from the `head_ids` won't be
    /// removed. In addition to that, objects created after `keep_newer` will be
    /// preserved. This mitigates a risk of deleting new heads created
    /// concurrently by another process.
    // TODO: return stats?
    fn gc(&self, head_ids: &[OperationId], keep_newer: SystemTime) -> OpStoreResult<()>;
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use super::*;

    #[test]
    fn test_merge_join_branch_views() {
        let remote_ref = |target: &RefTarget| RemoteRef {
            target: target.clone(),
            state: RemoteRefState::Tracking, // doesn't matter
        };
        let local_branch1_target = RefTarget::normal(CommitId::from_hex("111111"));
        let local_branch2_target = RefTarget::normal(CommitId::from_hex("222222"));
        let git_branch1_remote_ref = remote_ref(&RefTarget::normal(CommitId::from_hex("333333")));
        let git_branch2_remote_ref = remote_ref(&RefTarget::normal(CommitId::from_hex("444444")));
        let remote1_branch1_remote_ref =
            remote_ref(&RefTarget::normal(CommitId::from_hex("555555")));
        let remote2_branch2_remote_ref =
            remote_ref(&RefTarget::normal(CommitId::from_hex("666666")));

        let local_branches = btreemap! {
            "branch1".to_owned() => local_branch1_target.clone(),
            "branch2".to_owned() => local_branch2_target.clone(),
        };
        let remote_views = btreemap! {
            "git".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch1".to_owned() => git_branch1_remote_ref.clone(),
                    "branch2".to_owned() => git_branch2_remote_ref.clone(),
                },
            },
            "remote1".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch1".to_owned() => remote1_branch1_remote_ref.clone(),
                },
            },
            "remote2".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch2".to_owned() => remote2_branch2_remote_ref.clone(),
                },
            },
        };
        assert_eq!(
            merge_join_branch_views(&local_branches, &remote_views).collect_vec(),
            vec![
                (
                    "branch1",
                    BranchTarget {
                        local_target: &local_branch1_target,
                        remote_refs: vec![
                            ("git", &git_branch1_remote_ref),
                            ("remote1", &remote1_branch1_remote_ref),
                        ],
                    },
                ),
                (
                    "branch2",
                    BranchTarget {
                        local_target: &local_branch2_target.clone(),
                        remote_refs: vec![
                            ("git", &git_branch2_remote_ref),
                            ("remote2", &remote2_branch2_remote_ref),
                        ],
                    },
                ),
            ],
        );

        // Local only
        let local_branches = btreemap! {
            "branch1".to_owned() => local_branch1_target.clone(),
        };
        let remote_views = btreemap! {};
        assert_eq!(
            merge_join_branch_views(&local_branches, &remote_views).collect_vec(),
            vec![(
                "branch1",
                BranchTarget {
                    local_target: &local_branch1_target,
                    remote_refs: vec![],
                },
            )],
        );

        // Remote only
        let local_branches = btreemap! {};
        let remote_views = btreemap! {
            "remote1".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch1".to_owned() => remote1_branch1_remote_ref.clone(),
                },
            },
        };
        assert_eq!(
            merge_join_branch_views(&local_branches, &remote_views).collect_vec(),
            vec![(
                "branch1",
                BranchTarget {
                    local_target: RefTarget::absent_ref(),
                    remote_refs: vec![("remote1", &remote1_branch1_remote_ref)],
                },
            )],
        );
    }
}
