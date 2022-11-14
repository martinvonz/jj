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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;

use blake2::Blake2b512;
use itertools::Itertools;
use tempfile::{NamedTempFile, PersistError};
use thrift::protocol::{TCompactInputProtocol, TCompactOutputProtocol, TSerializable};

use crate::backend::{CommitId, MillisSinceEpoch, Timestamp};
use crate::content_hash::ContentHash;
use crate::file_util::persist_content_addressed_temp_file;
use crate::op_store::{
    BranchTarget, OpStore, OpStoreError, OpStoreResult, Operation, OperationId, OperationMetadata,
    RefTarget, View, ViewId, WorkspaceId,
};
#[cfg(feature = "legacy_protobuf")]
use crate::proto_op_store::ProtoOpStore;
use crate::simple_op_store_model;

impl From<std::io::Error> for OpStoreError {
    fn from(err: std::io::Error) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

impl From<PersistError> for OpStoreError {
    fn from(err: PersistError) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

impl From<thrift::Error> for OpStoreError {
    fn from(err: thrift::Error) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

// TODO: In version 0.7.0 or so, inline ThriftOpStore into this type and drop
// support for upgrading from the proto format
#[derive(Debug)]
pub struct SimpleOpStore {
    delegate: ThriftOpStore,
}

#[cfg(feature = "legacy_protobuf")]
fn upgrade_to_thrift(store_path: PathBuf) -> std::io::Result<()> {
    println!("Upgrading operation log to Thrift format...");
    let proto_store = ProtoOpStore::load(store_path.clone());
    let tmp_store_dir = tempfile::Builder::new()
        .prefix("jj-op-store-upgrade-")
        .tempdir_in(store_path.parent().unwrap())
        .unwrap();
    let tmp_store_path = tmp_store_dir.path().to_path_buf();

    // Find the current operation head(s) of the operation log. Because the hash is
    // based on the serialized format, it will be different after conversion, so
    // we need to rewrite these later.
    let op_heads_store_path = store_path.parent().unwrap().join("op_heads");
    let mut old_op_heads = HashSet::new();
    for entry in fs::read_dir(&op_heads_store_path)? {
        let basename = entry?.file_name();
        let op_id_str = basename.to_str().unwrap();
        if let Ok(op_id_bytes) = hex::decode(op_id_str) {
            old_op_heads.insert(OperationId::new(op_id_bytes));
        }
    }

    // Do a DFS to rewrite the operations
    let thrift_store = ThriftOpStore::init(tmp_store_path.clone());
    let mut converted: HashMap<OperationId, OperationId> = HashMap::new();
    // The DFS stack
    let mut to_convert = old_op_heads
        .iter()
        .map(|op_id| (op_id.clone(), proto_store.read_operation(op_id).unwrap()))
        .collect_vec();
    while !to_convert.is_empty() {
        let (_, op) = to_convert.last().unwrap();
        let mut new_parent_ids: Vec<OperationId> = vec![];
        let mut new_to_convert = vec![];
        // Check which parents are already converted and which ones we need to rewrite
        // first
        for parent_id in &op.parents {
            if let Some(new_parent_id) = converted.get(parent_id) {
                new_parent_ids.push(new_parent_id.clone());
            } else {
                let parent_op = proto_store.read_operation(parent_id).unwrap();
                new_to_convert.push((parent_id.clone(), parent_op));
            }
        }
        if new_to_convert.is_empty() {
            // If all parents have already been converted, remove this operation from the
            // stack and convert it
            let (op_id, mut op) = to_convert.pop().unwrap();
            op.parents = new_parent_ids;
            let view = proto_store.read_view(&op.view_id).unwrap();
            let thrift_view_id = thrift_store.write_view(&view).unwrap();
            op.view_id = thrift_view_id;
            let thrift_op_id = thrift_store.write_operation(&op).unwrap();
            converted.insert(op_id, thrift_op_id);
        } else {
            to_convert.extend(new_to_convert);
        }
    }

    fs::write(tmp_store_path.join("thrift_store"), "")?;
    let backup_store_path = store_path.parent().unwrap().join("op_store_old");
    fs::rename(&store_path, &backup_store_path)?;
    fs::rename(&tmp_store_path, &store_path)?;

    // Update the pointers to the head(s) of the operation log
    for old_op_head in old_op_heads {
        let new_op_head = converted.get(&old_op_head).unwrap().clone();
        fs::write(op_heads_store_path.join(new_op_head.hex()), "")?;
        fs::remove_file(op_heads_store_path.join(old_op_head.hex()))?;
    }

    // Update the pointers from operations to index files
    let index_operations_path = store_path
        .parent()
        .unwrap()
        .join("index")
        .join("operations");
    for entry in fs::read_dir(&index_operations_path)? {
        let basename = entry?.file_name();
        let op_id_str = basename.to_str().unwrap();
        if let Ok(op_id_bytes) = hex::decode(op_id_str) {
            let old_op_id = OperationId::new(op_id_bytes);
            // This should always succeed, but just skip it if it doesn't. We'll index
            // the commits on demand if we don't have an pointer to an index file.
            if let Some(new_op_id) = converted.get(&old_op_id) {
                fs::rename(
                    index_operations_path.join(basename),
                    index_operations_path.join(new_op_id.hex()),
                )?;
            }
        }
    }

    // Update the pointer to the last operation exported to Git
    let git_export_path = store_path.parent().unwrap().join("git_export_operation_id");
    if let Ok(op_id_string) = fs::read_to_string(&git_export_path) {
        if let Ok(op_id_bytes) = hex::decode(&op_id_string) {
            let old_op_id = OperationId::new(op_id_bytes);
            let new_op_id = converted.get(&old_op_id).unwrap();
            fs::write(&git_export_path, new_op_id.hex())?;
        }
    }

    println!("Upgrade complete");
    Ok(())
}

impl SimpleOpStore {
    pub fn init(store_path: PathBuf) -> Self {
        #[cfg(feature = "legacy_protobuf")]
        fs::write(store_path.join("thrift_store"), "").unwrap();
        let delegate = ThriftOpStore::init(store_path);
        SimpleOpStore { delegate }
    }

    #[cfg(feature = "legacy_protobuf")]
    pub fn load(store_path: PathBuf) -> Self {
        if !store_path.join("thrift_store").exists() {
            upgrade_to_thrift(store_path.clone())
                .expect("Failed to upgrade operation log to Thrift format");
        }
        let delegate = ThriftOpStore::load(store_path);
        SimpleOpStore { delegate }
    }

    #[cfg(not(feature = "legacy_protobuf"))]
    pub fn load(store_path: PathBuf) -> Self {
        let delegate = ThriftOpStore::load(store_path);
        SimpleOpStore { delegate }
    }
}

fn not_found_to_store_error(err: std::io::Error) -> OpStoreError {
    if err.kind() == ErrorKind::NotFound {
        OpStoreError::NotFound
    } else {
        OpStoreError::from(err)
    }
}

impl OpStore for SimpleOpStore {
    fn read_view(&self, id: &ViewId) -> OpStoreResult<View> {
        self.delegate.read_view(id)
    }

    fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        self.delegate.write_view(view)
    }

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        self.delegate.read_operation(id)
    }

    fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        self.delegate.write_operation(operation)
    }
}

#[derive(Debug)]
struct ThriftOpStore {
    path: PathBuf,
}

impl ThriftOpStore {
    fn init(store_path: PathBuf) -> Self {
        fs::create_dir(store_path.join("views")).unwrap();
        fs::create_dir(store_path.join("operations")).unwrap();
        Self::load(store_path)
    }

    fn load(store_path: PathBuf) -> Self {
        ThriftOpStore { path: store_path }
    }

    fn view_path(&self, id: &ViewId) -> PathBuf {
        self.path.join("views").join(id.hex())
    }

    fn operation_path(&self, id: &OperationId) -> PathBuf {
        self.path.join("operations").join(id.hex())
    }
}

impl OpStore for ThriftOpStore {
    fn read_view(&self, id: &ViewId) -> OpStoreResult<View> {
        let path = self.view_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;
        let thrift_view = read_thrift(&mut file)?;
        Ok(View::from(&thrift_view))
    }

    fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        let id = ViewId::new(hash(view).to_vec());
        let temp_file = NamedTempFile::new_in(&self.path)?;
        let thrift_view = simple_op_store_model::View::from(view);
        write_thrift(&thrift_view, &mut temp_file.as_file())?;
        persist_content_addressed_temp_file(temp_file, self.view_path(&id))?;
        Ok(id)
    }

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let path = self.operation_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;
        let thrift_operation = read_thrift(&mut file)?;
        Ok(Operation::from(&thrift_operation))
    }

    fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        let id = OperationId::new(hash(operation).to_vec());
        let temp_file = NamedTempFile::new_in(&self.path)?;
        let thrift_operation = simple_op_store_model::Operation::from(operation);
        write_thrift(&thrift_operation, &mut temp_file.as_file())?;
        persist_content_addressed_temp_file(temp_file, self.operation_path(&id))?;
        Ok(id)
    }
}

fn hash(x: &impl ContentHash) -> digest::Output<Blake2b512> {
    use digest::Digest;
    let mut hasher = Blake2b512::default();
    x.hash(&mut hasher);
    hasher.finalize()
}

pub fn read_thrift<T: TSerializable>(input: &mut impl Read) -> OpStoreResult<T> {
    let mut protocol = TCompactInputProtocol::new(input);
    Ok(TSerializable::read_from_in_protocol(&mut protocol).unwrap())
}

pub fn write_thrift<T: TSerializable>(
    thrift_object: &T,
    output: &mut impl Write,
) -> OpStoreResult<()> {
    let mut protocol = TCompactOutputProtocol::new(output);
    thrift_object.write_to_out_protocol(&mut protocol)?;
    Ok(())
}

impl From<&Timestamp> for simple_op_store_model::Timestamp {
    fn from(timestamp: &Timestamp) -> Self {
        simple_op_store_model::Timestamp::new(timestamp.timestamp.0, timestamp.tz_offset)
    }
}

impl From<&simple_op_store_model::Timestamp> for Timestamp {
    fn from(timestamp: &simple_op_store_model::Timestamp) -> Self {
        Timestamp {
            timestamp: MillisSinceEpoch(timestamp.millis_since_epoch),
            tz_offset: timestamp.tz_offset,
        }
    }
}

impl From<&OperationMetadata> for simple_op_store_model::OperationMetadata {
    fn from(metadata: &OperationMetadata) -> Self {
        let start_time = simple_op_store_model::Timestamp::from(&metadata.start_time);
        let end_time = simple_op_store_model::Timestamp::from(&metadata.end_time);
        let description = metadata.description.clone();
        let hostname = metadata.hostname.clone();
        let username = metadata.username.clone();
        let tags: BTreeMap<String, String> = metadata
            .tags
            .iter()
            .map(|(x, y)| (x.clone(), y.clone()))
            .collect();
        simple_op_store_model::OperationMetadata::new(
            start_time,
            end_time,
            description,
            hostname,
            username,
            tags,
        )
    }
}

impl From<&simple_op_store_model::OperationMetadata> for OperationMetadata {
    fn from(metadata: &simple_op_store_model::OperationMetadata) -> Self {
        let start_time = Timestamp::from(&metadata.start_time);
        let end_time = Timestamp::from(&metadata.end_time);
        let description = metadata.description.to_owned();
        let hostname = metadata.hostname.to_owned();
        let username = metadata.username.to_owned();
        let tags = metadata
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        OperationMetadata {
            start_time,
            end_time,
            description,
            hostname,
            username,
            tags,
        }
    }
}

impl From<&Operation> for simple_op_store_model::Operation {
    fn from(operation: &Operation) -> Self {
        let view_id = operation.view_id.as_bytes().to_vec();
        let mut parents = vec![];
        for parent in &operation.parents {
            parents.push(parent.to_bytes());
        }
        let metadata = Box::new(simple_op_store_model::OperationMetadata::from(
            &operation.metadata,
        ));
        simple_op_store_model::Operation::new(view_id, parents, metadata)
    }
}

impl From<&View> for simple_op_store_model::View {
    fn from(view: &View) -> Self {
        let mut wc_commit_ids = BTreeMap::new();
        for (workspace_id, commit_id) in &view.wc_commit_ids {
            wc_commit_ids.insert(workspace_id.as_str().to_string(), commit_id.to_bytes());
        }

        let mut head_ids = vec![];
        for head_id in &view.head_ids {
            head_ids.push(head_id.to_bytes());
        }

        let mut public_head_ids = vec![];
        for head_id in &view.public_head_ids {
            public_head_ids.push(head_id.to_bytes());
        }

        let mut branches = vec![];
        for (name, target) in &view.branches {
            let local_target = target
                .local_target
                .as_ref()
                .map(simple_op_store_model::RefTarget::from);
            let mut remote_branches = vec![];
            for (remote_name, target) in &target.remote_targets {
                remote_branches.push(simple_op_store_model::RemoteBranch::new(
                    remote_name.clone(),
                    simple_op_store_model::RefTarget::from(target),
                ));
            }
            branches.push(simple_op_store_model::Branch::new(
                name.clone(),
                local_target,
                remote_branches,
            ));
        }

        let mut tags = vec![];
        for (name, target) in &view.tags {
            tags.push(simple_op_store_model::Tag::new(
                name.clone(),
                simple_op_store_model::RefTarget::from(target),
            ));
        }

        let mut git_refs = vec![];
        for (git_ref_name, target) in &view.git_refs {
            git_refs.push(simple_op_store_model::GitRef::new(
                git_ref_name.clone(),
                simple_op_store_model::RefTarget::from(target),
            ));
        }

        let git_head = view.git_head.as_ref().map(|git_head| git_head.to_bytes());

        simple_op_store_model::View::new(
            head_ids,
            public_head_ids,
            wc_commit_ids,
            branches,
            tags,
            git_refs,
            git_head,
        )
    }
}

impl From<&simple_op_store_model::Operation> for Operation {
    fn from(operation: &simple_op_store_model::Operation) -> Self {
        let operation_id_from_thrift = |parent: &Vec<u8>| OperationId::new(parent.clone());
        let parents = operation
            .parents
            .iter()
            .map(operation_id_from_thrift)
            .collect();
        let view_id = ViewId::new(operation.view_id.clone());
        let metadata = OperationMetadata::from(operation.metadata.as_ref());
        Operation {
            view_id,
            parents,
            metadata,
        }
    }
}

impl From<&simple_op_store_model::View> for View {
    fn from(thrift_view: &simple_op_store_model::View) -> Self {
        let mut view = View::default();
        for (workspace_id, commit_id) in &thrift_view.wc_commit_ids {
            view.wc_commit_ids.insert(
                WorkspaceId::new(workspace_id.clone()),
                CommitId::new(commit_id.clone()),
            );
        }
        for head_id_bytes in &thrift_view.head_ids {
            view.head_ids.insert(CommitId::from_bytes(head_id_bytes));
        }
        for head_id_bytes in &thrift_view.public_head_ids {
            view.public_head_ids
                .insert(CommitId::from_bytes(head_id_bytes));
        }

        for thrift_branch in &thrift_view.branches {
            let local_target = thrift_branch.local_target.as_ref().map(RefTarget::from);

            let mut remote_targets = BTreeMap::new();
            for remote_branch in &thrift_branch.remote_branches {
                remote_targets.insert(
                    remote_branch.remote_name.clone(),
                    RefTarget::from(&remote_branch.target),
                );
            }

            view.branches.insert(
                thrift_branch.name.clone(),
                BranchTarget {
                    local_target,
                    remote_targets,
                },
            );
        }

        for thrift_tag in &thrift_view.tags {
            view.tags
                .insert(thrift_tag.name.clone(), RefTarget::from(&thrift_tag.target));
        }

        for git_ref in &thrift_view.git_refs {
            view.git_refs
                .insert(git_ref.name.clone(), RefTarget::from(&git_ref.target));
        }

        view.git_head = thrift_view
            .git_head
            .as_ref()
            .map(|head| CommitId::new(head.clone()));

        view
    }
}

impl From<&RefTarget> for simple_op_store_model::RefTarget {
    fn from(ref_target: &RefTarget) -> Self {
        match ref_target {
            RefTarget::Normal(id) => simple_op_store_model::RefTarget::CommitId(id.to_bytes()),
            RefTarget::Conflict { removes, adds } => {
                let adds = adds.iter().map(|id| id.to_bytes()).collect_vec();
                let removes = removes.iter().map(|id| id.to_bytes()).collect_vec();
                let ref_conflict_thrift = simple_op_store_model::RefConflict::new(removes, adds);
                simple_op_store_model::RefTarget::Conflict(ref_conflict_thrift)
            }
        }
    }
}

impl From<&simple_op_store_model::RefTarget> for RefTarget {
    fn from(thrift_ref_target: &simple_op_store_model::RefTarget) -> Self {
        match thrift_ref_target {
            simple_op_store_model::RefTarget::CommitId(commit_id) => {
                RefTarget::Normal(CommitId::from_bytes(commit_id))
            }
            simple_op_store_model::RefTarget::Conflict(conflict) => {
                let removes = conflict
                    .removes
                    .iter()
                    .map(|id_bytes| CommitId::from_bytes(id_bytes))
                    .collect_vec();
                let adds = conflict
                    .adds
                    .iter()
                    .map(|id_bytes| CommitId::from_bytes(id_bytes))
                    .collect_vec();
                RefTarget::Conflict { removes, adds }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use maplit::{btreemap, hashmap, hashset};

    use super::*;

    fn create_view() -> View {
        let head_id1 = CommitId::from_hex("aaa111");
        let head_id2 = CommitId::from_hex("aaa222");
        let public_head_id1 = CommitId::from_hex("bbb444");
        let public_head_id2 = CommitId::from_hex("bbb555");
        let branch_main_local_target = RefTarget::Normal(CommitId::from_hex("ccc111"));
        let branch_main_origin_target = RefTarget::Normal(CommitId::from_hex("ccc222"));
        let branch_deleted_origin_target = RefTarget::Normal(CommitId::from_hex("ccc333"));
        let tag_v1_target = RefTarget::Normal(CommitId::from_hex("ddd111"));
        let git_refs_main_target = RefTarget::Normal(CommitId::from_hex("fff111"));
        let git_refs_feature_target = RefTarget::Conflict {
            removes: vec![CommitId::from_hex("fff111")],
            adds: vec![CommitId::from_hex("fff222"), CommitId::from_hex("fff333")],
        };
        let default_wc_commit_id = CommitId::from_hex("abc111");
        let test_wc_commit_id = CommitId::from_hex("abc222");
        View {
            head_ids: hashset! {head_id1, head_id2},
            public_head_ids: hashset! {public_head_id1, public_head_id2},
            branches: btreemap! {
                "main".to_string() => BranchTarget {
                    local_target: Some(branch_main_local_target),
                    remote_targets: btreemap! {
                        "origin".to_string() => branch_main_origin_target,
                    }
                },
                "deleted".to_string() => BranchTarget {
                    local_target: None,
                    remote_targets: btreemap! {
                        "origin".to_string() => branch_deleted_origin_target,
                    }
                },
            },
            tags: btreemap! {
                "v1.0".to_string() => tag_v1_target,
            },
            git_refs: btreemap! {
                "refs/heads/main".to_string() => git_refs_main_target,
                "refs/heads/feature".to_string() => git_refs_feature_target
            },
            git_head: Some(CommitId::from_hex("fff111")),
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
            ViewId::new(hash(&create_view()).to_vec()).hex(),
            @"2a026b6a091219a3d8ca43d822984cf9be0c53438225d76a5ba5e6d3724fab15104579fb08fa949977c4357b1806d240bef28d958cbcd7d786962ac88c15df31"
        );
    }

    #[test]
    fn test_hash_operation() {
        // Test exact output so we detect regressions in compatibility
        assert_snapshot!(
            OperationId::new(hash(&create_operation()).to_vec()).hex(),
            @"3ec986c29ff8eb808ea8f6325d6307cea75ef02987536c8e4645406aba51afc8e229957a6e855170d77a66098c58912309323f5e0b32760caa2b59dc84d45fcf"
        );
    }

    #[test]
    fn test_read_write_view() {
        let temp_dir = testutils::new_temp_dir();
        let store = SimpleOpStore::init(temp_dir.path().to_owned());
        let view = create_view();
        let view_id = store.write_view(&view).unwrap();
        let read_view = store.read_view(&view_id).unwrap();
        assert_eq!(read_view, view);
    }

    #[test]
    fn test_read_write_operation() {
        let temp_dir = testutils::new_temp_dir();
        let store = SimpleOpStore::init(temp_dir.path().to_owned());
        let operation = create_operation();
        let op_id = store.write_operation(&operation).unwrap();
        let read_operation = store.read_operation(&op_id).unwrap();
        assert_eq!(read_operation, operation);
    }
}
