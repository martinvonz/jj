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

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use prost::Message;
use tempfile::{NamedTempFile, PersistError};
use thiserror::Error;

use crate::backend::{CommitId, MillisSinceEpoch, ObjectId, Timestamp};
use crate::content_hash::blake2b_hash;
use crate::file_util::persist_content_addressed_temp_file;
use crate::merge::Merge;
use crate::op_store::{
    OpStore, OpStoreError, OpStoreResult, Operation, OperationId, OperationMetadata, RefTarget,
    RemoteRef, RemoteView, View, ViewId, WorkspaceId,
};
use crate::{git, op_store};

impl From<PersistError> for OpStoreError {
    fn from(err: PersistError) -> Self {
        OpStoreError::Other(err.into())
    }
}

#[derive(Debug, Error)]
#[error("Failed to read {kind} with ID {id}: {err}")]
struct DecodeError {
    kind: &'static str,
    id: String,
    #[source]
    err: prost::DecodeError,
}

impl From<DecodeError> for OpStoreError {
    fn from(err: DecodeError) -> Self {
        OpStoreError::Other(err.into())
    }
}

#[derive(Debug)]
pub struct SimpleOpStore {
    path: PathBuf,
}

impl SimpleOpStore {
    pub fn name() -> &'static str {
        "simple_op_store"
    }

    /// Creates an empty OpStore, panics if it already exists
    pub fn init(store_path: &Path) -> Self {
        fs::create_dir(store_path.join("views")).unwrap();
        fs::create_dir(store_path.join("operations")).unwrap();
        SimpleOpStore {
            path: store_path.to_owned(),
        }
    }

    /// Load an existing OpStore
    pub fn load(store_path: &Path) -> Self {
        SimpleOpStore {
            path: store_path.to_path_buf(),
        }
    }

    fn view_path(&self, id: &ViewId) -> PathBuf {
        self.path.join("views").join(id.hex())
    }

    fn operation_path(&self, id: &OperationId) -> PathBuf {
        self.path.join("operations").join(id.hex())
    }
}

impl OpStore for SimpleOpStore {
    fn name(&self) -> &str {
        Self::name()
    }

    fn read_view(&self, id: &ViewId) -> OpStoreResult<View> {
        let path = self.view_path(id);
        let buf = fs::read(path).map_err(|err| not_found_to_store_error(err, id))?;

        let proto = crate::protos::op_store::View::decode(&*buf).map_err(|err| DecodeError {
            kind: "view",
            id: id.hex(),
            err,
        })?;
        Ok(view_from_proto(proto))
    }

    fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        let temp_file =
            NamedTempFile::new_in(&self.path).map_err(|err| io_to_write_error(err, "view"))?;

        let proto = view_to_proto(view);
        temp_file
            .as_file()
            .write_all(&proto.encode_to_vec())
            .map_err(|err| io_to_write_error(err, "view"))?;

        let id = ViewId::new(blake2b_hash(view).to_vec());

        persist_content_addressed_temp_file(temp_file, self.view_path(&id))?;
        Ok(id)
    }

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let path = self.operation_path(id);
        let buf = fs::read(path).map_err(|err| not_found_to_store_error(err, id))?;

        let proto =
            crate::protos::op_store::Operation::decode(&*buf).map_err(|err| DecodeError {
                kind: "operation",
                id: id.hex(),
                err,
            })?;
        Ok(operation_from_proto(proto))
    }

    fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        let temp_file =
            NamedTempFile::new_in(&self.path).map_err(|err| io_to_write_error(err, "operation"))?;

        let proto = operation_to_proto(operation);
        temp_file
            .as_file()
            .write_all(&proto.encode_to_vec())
            .map_err(|err| io_to_write_error(err, "operation"))?;

        let id = OperationId::new(blake2b_hash(operation).to_vec());

        persist_content_addressed_temp_file(temp_file, self.operation_path(&id))?;
        Ok(id)
    }
}

fn not_found_to_store_error(err: std::io::Error, id: &impl ObjectId) -> OpStoreError {
    if err.kind() == ErrorKind::NotFound {
        OpStoreError::NotFound
    } else {
        io_to_read_error(err, id)
    }
}

fn io_to_read_error(err: std::io::Error, id: &impl ObjectId) -> OpStoreError {
    if err.kind() == ErrorKind::NotFound {
        OpStoreError::NotFound
    } else {
        OpStoreError::ReadObject {
            object_type: id.object_type(),
            hash: id.hex(),
            source: Box::new(err),
        }
    }
}

fn io_to_write_error(err: std::io::Error, object_type: &'static str) -> OpStoreError {
    OpStoreError::WriteObject {
        object_type,
        source: Box::new(err),
    }
}

fn timestamp_to_proto(timestamp: &Timestamp) -> crate::protos::op_store::Timestamp {
    crate::protos::op_store::Timestamp {
        millis_since_epoch: timestamp.timestamp.0,
        tz_offset: timestamp.tz_offset,
    }
}

fn timestamp_from_proto(proto: crate::protos::op_store::Timestamp) -> Timestamp {
    Timestamp {
        timestamp: MillisSinceEpoch(proto.millis_since_epoch),
        tz_offset: proto.tz_offset,
    }
}

fn operation_metadata_to_proto(
    metadata: &OperationMetadata,
) -> crate::protos::op_store::OperationMetadata {
    crate::protos::op_store::OperationMetadata {
        start_time: Some(timestamp_to_proto(&metadata.start_time)),
        end_time: Some(timestamp_to_proto(&metadata.end_time)),
        description: metadata.description.clone(),
        hostname: metadata.hostname.clone(),
        username: metadata.username.clone(),
        tags: metadata.tags.clone(),
    }
}

fn operation_metadata_from_proto(
    proto: crate::protos::op_store::OperationMetadata,
) -> OperationMetadata {
    let start_time = timestamp_from_proto(proto.start_time.unwrap_or_default());
    let end_time = timestamp_from_proto(proto.end_time.unwrap_or_default());
    OperationMetadata {
        start_time,
        end_time,
        description: proto.description,
        hostname: proto.hostname,
        username: proto.username,
        tags: proto.tags,
    }
}

fn operation_to_proto(operation: &Operation) -> crate::protos::op_store::Operation {
    let mut proto = crate::protos::op_store::Operation {
        view_id: operation.view_id.as_bytes().to_vec(),
        metadata: Some(operation_metadata_to_proto(&operation.metadata)),
        ..Default::default()
    };
    for parent in &operation.parents {
        proto.parents.push(parent.to_bytes());
    }
    proto
}

fn operation_from_proto(proto: crate::protos::op_store::Operation) -> Operation {
    let parents = proto.parents.into_iter().map(OperationId::new).collect();
    let view_id = ViewId::new(proto.view_id);
    let metadata = operation_metadata_from_proto(proto.metadata.unwrap_or_default());
    Operation {
        view_id,
        parents,
        metadata,
    }
}

fn view_to_proto(view: &View) -> crate::protos::op_store::View {
    let mut proto = crate::protos::op_store::View {
        // New/loaded view should have been migrated to the latest format
        has_git_refs_migrated_to_remote: true,
        ..Default::default()
    };
    for (workspace_id, commit_id) in &view.wc_commit_ids {
        proto
            .wc_commit_ids
            .insert(workspace_id.as_str().to_string(), commit_id.to_bytes());
    }
    for head_id in &view.head_ids {
        proto.head_ids.push(head_id.to_bytes());
    }
    for head_id in &view.public_head_ids {
        proto.public_head_ids.push(head_id.to_bytes());
    }

    proto.branches = branch_views_to_proto_legacy(&view.local_branches, &view.remote_views);

    for (name, target) in &view.tags {
        proto.tags.push(crate::protos::op_store::Tag {
            name: name.clone(),
            target: ref_target_to_proto(target),
        });
    }

    for (git_ref_name, target) in &view.git_refs {
        proto.git_refs.push(crate::protos::op_store::GitRef {
            name: git_ref_name.clone(),
            target: ref_target_to_proto(target),
            ..Default::default()
        });
    }

    proto.git_head = ref_target_to_proto(&view.git_head);

    proto
}

fn view_from_proto(proto: crate::protos::op_store::View) -> View {
    let mut view = View::default();
    // For compatibility with old repos before we had support for multiple working
    // copies
    #[allow(deprecated)]
    if !proto.wc_commit_id.is_empty() {
        view.wc_commit_ids
            .insert(WorkspaceId::default(), CommitId::new(proto.wc_commit_id));
    }
    for (workspace_id, commit_id) in proto.wc_commit_ids {
        view.wc_commit_ids
            .insert(WorkspaceId::new(workspace_id), CommitId::new(commit_id));
    }
    for head_id_bytes in proto.head_ids {
        view.head_ids.insert(CommitId::new(head_id_bytes));
    }
    for head_id_bytes in proto.public_head_ids {
        view.public_head_ids.insert(CommitId::new(head_id_bytes));
    }

    let (local_branches, remote_views) = branch_views_from_proto_legacy(proto.branches);
    view.local_branches = local_branches;
    view.remote_views = remote_views;

    for tag_proto in proto.tags {
        view.tags
            .insert(tag_proto.name, ref_target_from_proto(tag_proto.target));
    }

    for git_ref in proto.git_refs {
        let target = if git_ref.target.is_some() {
            ref_target_from_proto(git_ref.target)
        } else {
            // Legacy format
            RefTarget::normal(CommitId::new(git_ref.commit_id))
        };
        view.git_refs.insert(git_ref.name, target);
    }

    #[allow(deprecated)]
    if proto.git_head.is_some() {
        view.git_head = ref_target_from_proto(proto.git_head);
    } else if !proto.git_head_legacy.is_empty() {
        view.git_head = RefTarget::normal(CommitId::new(proto.git_head_legacy));
    }

    if !proto.has_git_refs_migrated_to_remote {
        migrate_git_refs_to_remote(&mut view);
    }

    view
}

fn branch_views_to_proto_legacy(
    local_branches: &BTreeMap<String, RefTarget>,
    remote_views: &BTreeMap<String, RemoteView>,
) -> Vec<crate::protos::op_store::Branch> {
    op_store::merge_join_branch_views(local_branches, remote_views)
        .map(|(name, branch_target)| {
            let local_target = ref_target_to_proto(branch_target.local_target);
            let remote_branches = branch_target
                .remote_refs
                .iter()
                .map(
                    |&(remote_name, remote_ref)| crate::protos::op_store::RemoteBranch {
                        remote_name: remote_name.to_owned(),
                        target: ref_target_to_proto(&remote_ref.target),
                    },
                )
                .collect();
            crate::protos::op_store::Branch {
                name: name.to_owned(),
                local_target,
                remote_branches,
            }
        })
        .collect()
}

fn branch_views_from_proto_legacy(
    branches_legacy: Vec<crate::protos::op_store::Branch>,
) -> (BTreeMap<String, RefTarget>, BTreeMap<String, RemoteView>) {
    let mut local_branches: BTreeMap<String, RefTarget> = BTreeMap::new();
    let mut remote_views: BTreeMap<String, RemoteView> = BTreeMap::new();
    for branch_proto in branches_legacy {
        for remote_branch in branch_proto.remote_branches {
            let remote_view = remote_views.entry(remote_branch.remote_name).or_default();
            let remote_ref = RemoteRef {
                target: ref_target_from_proto(remote_branch.target),
            };
            remote_view
                .branches
                .insert(branch_proto.name.clone(), remote_ref);
        }
        let local_target = ref_target_from_proto(branch_proto.local_target);
        if local_target.is_present() {
            local_branches.insert(branch_proto.name, local_target);
        }
    }
    (local_branches, remote_views)
}

fn migrate_git_refs_to_remote(view: &mut View) {
    if view.git_refs.is_empty() {
        // Not a repo backed by Git?
        return;
    }

    tracing::info!("migrating Git-tracking branches");
    let mut git_view = RemoteView::default();
    for (full_name, target) in &view.git_refs {
        if let Some(name) = full_name.strip_prefix("refs/heads/") {
            assert!(!name.is_empty());
            let remote_ref = RemoteRef {
                target: target.clone(),
            };
            git_view.branches.insert(name.to_owned(), remote_ref);
        }
    }
    view.remote_views
        .insert(git::REMOTE_NAME_FOR_LOCAL_GIT_REPO.to_owned(), git_view);

    // jj < 0.9 might have imported refs from remote named "git"
    let reserved_git_ref_prefix = format!("refs/remotes/{}/", git::REMOTE_NAME_FOR_LOCAL_GIT_REPO);
    view.git_refs
        .retain(|name, _| !name.starts_with(&reserved_git_ref_prefix));
}

fn ref_target_to_proto(value: &RefTarget) -> Option<crate::protos::op_store::RefTarget> {
    let term_to_proto = |term: &Option<CommitId>| crate::protos::op_store::ref_conflict::Term {
        value: term.as_ref().map(|id| id.to_bytes()),
    };
    let merge = value.as_merge();
    let conflict_proto = crate::protos::op_store::RefConflict {
        removes: merge.removes().iter().map(term_to_proto).collect(),
        adds: merge.adds().iter().map(term_to_proto).collect(),
    };
    let proto = crate::protos::op_store::RefTarget {
        value: Some(crate::protos::op_store::ref_target::Value::Conflict(
            conflict_proto,
        )),
    };
    Some(proto)
}

#[allow(deprecated)]
#[cfg(test)]
fn ref_target_to_proto_legacy(value: &RefTarget) -> Option<crate::protos::op_store::RefTarget> {
    if let Some(id) = value.as_normal() {
        let proto = crate::protos::op_store::RefTarget {
            value: Some(crate::protos::op_store::ref_target::Value::CommitId(
                id.to_bytes(),
            )),
        };
        Some(proto)
    } else if value.has_conflict() {
        let ref_conflict_proto = crate::protos::op_store::RefConflictLegacy {
            removes: value.removed_ids().map(|id| id.to_bytes()).collect(),
            adds: value.added_ids().map(|id| id.to_bytes()).collect(),
        };
        let proto = crate::protos::op_store::RefTarget {
            value: Some(crate::protos::op_store::ref_target::Value::ConflictLegacy(
                ref_conflict_proto,
            )),
        };
        Some(proto)
    } else {
        assert!(value.is_absent());
        None
    }
}

fn ref_target_from_proto(maybe_proto: Option<crate::protos::op_store::RefTarget>) -> RefTarget {
    // TODO: Delete legacy format handling when we decide to drop support for views
    // saved by jj <= 0.8.
    let Some(proto) = maybe_proto else {
        // Legacy absent id
        return RefTarget::absent();
    };
    match proto.value.unwrap() {
        #[allow(deprecated)]
        crate::protos::op_store::ref_target::Value::CommitId(id) => {
            // Legacy non-conflicting id
            RefTarget::normal(CommitId::new(id))
        }
        #[allow(deprecated)]
        crate::protos::op_store::ref_target::Value::ConflictLegacy(conflict) => {
            // Legacy conflicting ids
            let removes = conflict.removes.into_iter().map(CommitId::new);
            let adds = conflict.adds.into_iter().map(CommitId::new);
            RefTarget::from_legacy_form(removes, adds)
        }
        crate::protos::op_store::ref_target::Value::Conflict(conflict) => {
            let term_from_proto =
                |term: crate::protos::op_store::ref_conflict::Term| term.value.map(CommitId::new);
            let removes = conflict.removes.into_iter().map(term_from_proto).collect();
            let adds = conflict.adds.into_iter().map(term_from_proto).collect();
            RefTarget::from_merge(Merge::new(removes, adds))
        }
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use itertools::Itertools as _;
    use maplit::{btreemap, hashmap, hashset};

    use super::*;
    use crate::backend::{CommitId, MillisSinceEpoch, ObjectId, Timestamp};
    use crate::content_hash::blake2b_hash;
    use crate::op_store::{OperationMetadata, RefTarget, WorkspaceId};

    fn create_view() -> View {
        let remote_ref = |target: &RefTarget| RemoteRef {
            target: target.clone(),
        };
        let head_id1 = CommitId::from_hex("aaa111");
        let head_id2 = CommitId::from_hex("aaa222");
        let public_head_id1 = CommitId::from_hex("bbb444");
        let public_head_id2 = CommitId::from_hex("bbb555");
        let branch_main_local_target = RefTarget::normal(CommitId::from_hex("ccc111"));
        let branch_main_origin_target = RefTarget::normal(CommitId::from_hex("ccc222"));
        let branch_deleted_origin_target = RefTarget::normal(CommitId::from_hex("ccc333"));
        let tag_v1_target = RefTarget::normal(CommitId::from_hex("ddd111"));
        let git_refs_main_target = RefTarget::normal(CommitId::from_hex("fff111"));
        let git_refs_feature_target = RefTarget::from_legacy_form(
            [CommitId::from_hex("fff111")],
            [CommitId::from_hex("fff222"), CommitId::from_hex("fff333")],
        );
        let default_wc_commit_id = CommitId::from_hex("abc111");
        let test_wc_commit_id = CommitId::from_hex("abc222");
        View {
            head_ids: hashset! {head_id1, head_id2},
            public_head_ids: hashset! {public_head_id1, public_head_id2},
            local_branches: btreemap! {
                "main".to_string() => branch_main_local_target,
            },
            tags: btreemap! {
                "v1.0".to_string() => tag_v1_target,
            },
            remote_views: btreemap! {
                "origin".to_string() => RemoteView {
                    branches: btreemap! {
                        "main".to_string() => remote_ref(&branch_main_origin_target),
                        "deleted".to_string() => remote_ref(&branch_deleted_origin_target),
                    },
                },
            },
            git_refs: btreemap! {
                "refs/heads/main".to_string() => git_refs_main_target,
                "refs/heads/feature".to_string() => git_refs_feature_target,
            },
            git_head: RefTarget::normal(CommitId::from_hex("fff111")),
            wc_commit_ids: hashmap! {
                WorkspaceId::default() => default_wc_commit_id,
                WorkspaceId::new("test".to_string()) => test_wc_commit_id,
            },
        }
    }

    fn create_operation() -> Operation {
        Operation {
            view_id: ViewId::from_hex("aaa111"),
            parents: vec![
                OperationId::from_hex("bbb111"),
                OperationId::from_hex("bbb222"),
            ],
            metadata: OperationMetadata {
                start_time: Timestamp {
                    timestamp: MillisSinceEpoch(123456789),
                    tz_offset: 3600,
                },
                end_time: Timestamp {
                    timestamp: MillisSinceEpoch(123456800),
                    tz_offset: 3600,
                },
                description: "check out foo".to_string(),
                hostname: "some.host.example.com".to_string(),
                username: "someone".to_string(),
                tags: hashmap! {
                    "key1".to_string() => "value1".to_string(),
                    "key2".to_string() => "value2".to_string(),
                },
            },
        }
    }

    #[test]
    fn test_hash_view() {
        // Test exact output so we detect regressions in compatibility
        assert_snapshot!(
            ViewId::new(blake2b_hash(&create_view()).to_vec()).hex(),
            @"6ef5f01bb0bd239670c79966753c6af9ce18694bba1d5dbd1aa82de7f5c421dfc3bf91c1608eec480ae8e244093485bcff3e95db7acdc3958c7a8ead7a453b54"
        );
    }

    #[test]
    fn test_hash_operation() {
        // Test exact output so we detect regressions in compatibility
        assert_snapshot!(
            OperationId::new(blake2b_hash(&create_operation()).to_vec()).hex(),
            @"3ec986c29ff8eb808ea8f6325d6307cea75ef02987536c8e4645406aba51afc8e229957a6e855170d77a66098c58912309323f5e0b32760caa2b59dc84d45fcf"
        );
    }

    #[test]
    fn test_read_write_view() {
        let temp_dir = testutils::new_temp_dir();
        let store = SimpleOpStore::init(temp_dir.path());
        let view = create_view();
        let view_id = store.write_view(&view).unwrap();
        let read_view = store.read_view(&view_id).unwrap();
        assert_eq!(read_view, view);
    }

    #[test]
    fn test_read_write_operation() {
        let temp_dir = testutils::new_temp_dir();
        let store = SimpleOpStore::init(temp_dir.path());
        let operation = create_operation();
        let op_id = store.write_operation(&operation).unwrap();
        let read_operation = store.read_operation(&op_id).unwrap();
        assert_eq!(read_operation, operation);
    }

    #[test]
    fn test_branch_views_legacy_roundtrip() {
        let remote_ref = |target: &RefTarget| RemoteRef {
            target: target.clone(),
        };
        let local_branch1_target = RefTarget::normal(CommitId::from_hex("111111"));
        let local_branch3_target = RefTarget::normal(CommitId::from_hex("222222"));
        let git_branch1_target = RefTarget::normal(CommitId::from_hex("333333"));
        let remote1_branch1_target = RefTarget::normal(CommitId::from_hex("444444"));
        let remote2_branch2_target = RefTarget::normal(CommitId::from_hex("555555"));
        let remote2_branch4_target = RefTarget::normal(CommitId::from_hex("666666"));
        let local_branches = btreemap! {
            "branch1".to_owned() => local_branch1_target.clone(),
            "branch3".to_owned() => local_branch3_target.clone(),
        };
        let remote_views = btreemap! {
            "git".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch1".to_owned() => remote_ref(&git_branch1_target),
                },
            },
            "remote1".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch1".to_owned() => remote_ref(&remote1_branch1_target),
                },
            },
            "remote2".to_owned() => RemoteView {
                branches: btreemap! {
                    "branch2".to_owned() => remote_ref(&remote2_branch2_target),
                    "branch4".to_owned() => remote_ref(&remote2_branch4_target),
                },
            },
        };

        let branches_legacy = branch_views_to_proto_legacy(&local_branches, &remote_views);
        assert_eq!(
            branches_legacy
                .iter()
                .map(|proto| &proto.name)
                .sorted()
                .collect_vec(),
            vec!["branch1", "branch2", "branch3", "branch4"],
        );

        let (local_branches_reconstructed, remote_views_reconstructed) =
            branch_views_from_proto_legacy(branches_legacy);
        assert_eq!(local_branches_reconstructed, local_branches);
        assert_eq!(remote_views_reconstructed, remote_views);
    }

    #[test]
    fn test_migrate_git_refs_remote_named_git() {
        let normal_ref_target = |id_hex: &str| RefTarget::normal(CommitId::from_hex(id_hex));
        let normal_remote_ref = |id_hex: &str| RemoteRef {
            target: normal_ref_target(id_hex),
        };
        let branch_to_proto =
            |name: &str, local_ref_target, remote_branches| crate::protos::op_store::Branch {
                name: name.to_owned(),
                local_target: ref_target_to_proto(local_ref_target),
                remote_branches,
            };
        let remote_branch_to_proto =
            |remote_name: &str, ref_target| crate::protos::op_store::RemoteBranch {
                remote_name: remote_name.to_owned(),
                target: ref_target_to_proto(ref_target),
            };
        let git_ref_to_proto = |name: &str, ref_target| crate::protos::op_store::GitRef {
            name: name.to_owned(),
            target: ref_target_to_proto(ref_target),
            ..Default::default()
        };

        let proto = crate::protos::op_store::View {
            branches: vec![branch_to_proto(
                "main",
                &normal_ref_target("111111"),
                vec![
                    remote_branch_to_proto("git", &normal_ref_target("222222")),
                    remote_branch_to_proto("gita", &normal_ref_target("333333")),
                ],
            )],
            git_refs: vec![
                git_ref_to_proto("refs/heads/main", &normal_ref_target("444444")),
                git_ref_to_proto("refs/remotes/git/main", &normal_ref_target("555555")),
                git_ref_to_proto("refs/remotes/gita/main", &normal_ref_target("666666")),
            ],
            has_git_refs_migrated_to_remote: false,
            ..Default::default()
        };

        let view = view_from_proto(proto);
        assert_eq!(
            view.local_branches,
            btreemap! {
                "main".to_owned() => normal_ref_target("111111"),
            },
        );
        assert_eq!(
            view.remote_views,
            btreemap! {
                "git".to_owned() => RemoteView {
                    branches: btreemap! {
                        "main".to_owned() => normal_remote_ref("444444"), // refs/heads/main
                    },
                },
                "gita".to_owned() => RemoteView {
                    branches: btreemap! {
                        "main".to_owned() => normal_remote_ref("333333"),
                    },
                },
            },
        );
        assert_eq!(
            view.git_refs,
            btreemap! {
                "refs/heads/main".to_owned() => normal_ref_target("444444"),
                "refs/remotes/gita/main".to_owned() => normal_ref_target("666666"),
            },
        );

        // Once migrated, "git" remote branches shouldn't be populated again.
        let mut proto = view_to_proto(&view);
        assert!(proto.has_git_refs_migrated_to_remote);
        proto.branches.clear();
        let view = view_from_proto(proto);
        assert!(!view.remote_views.contains_key("git"));
    }

    #[test]
    fn test_ref_target_change_delete_order_roundtrip() {
        let target = RefTarget::from_merge(Merge::new(
            vec![Some(CommitId::from_hex("111111"))],
            vec![Some(CommitId::from_hex("222222")), None],
        ));
        let maybe_proto = ref_target_to_proto(&target);
        assert_eq!(ref_target_from_proto(maybe_proto), target);

        // If it were legacy format, order of None entry would be lost.
        let target = RefTarget::from_merge(Merge::new(
            vec![Some(CommitId::from_hex("111111"))],
            vec![None, Some(CommitId::from_hex("222222"))],
        ));
        let maybe_proto = ref_target_to_proto(&target);
        assert_eq!(ref_target_from_proto(maybe_proto), target);
    }

    #[test]
    fn test_ref_target_legacy_roundtrip() {
        let target = RefTarget::absent();
        let maybe_proto = ref_target_to_proto_legacy(&target);
        assert_eq!(ref_target_from_proto(maybe_proto), target);

        let target = RefTarget::normal(CommitId::from_hex("111111"));
        let maybe_proto = ref_target_to_proto_legacy(&target);
        assert_eq!(ref_target_from_proto(maybe_proto), target);

        // N-way conflict
        let target = RefTarget::from_legacy_form(
            [CommitId::from_hex("111111"), CommitId::from_hex("222222")],
            [
                CommitId::from_hex("333333"),
                CommitId::from_hex("444444"),
                CommitId::from_hex("555555"),
            ],
        );
        let maybe_proto = ref_target_to_proto_legacy(&target);
        assert_eq!(ref_target_from_proto(maybe_proto), target);

        // Change-delete conflict
        let target = RefTarget::from_legacy_form(
            [CommitId::from_hex("111111")],
            [CommitId::from_hex("222222")],
        );
        let maybe_proto = ref_target_to_proto_legacy(&target);
        assert_eq!(ref_target_from_proto(maybe_proto), target);
    }
}
