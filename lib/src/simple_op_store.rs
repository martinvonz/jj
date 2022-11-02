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

use blake2::{Blake2b512, Digest};
use byteorder::{LittleEndian, WriteBytesExt};
use itertools::Itertools;
use tempfile::{NamedTempFile, PersistError};
use thrift::protocol::{TCompactInputProtocol, TCompactOutputProtocol, TSerializable};

use crate::backend::{CommitId, MillisSinceEpoch, Timestamp};
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
        .tempdir()
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
        Ok(view_from_thrift(&thrift_view))
    }

    fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        let id = hash_view(view);
        let temp_file = NamedTempFile::new_in(&self.path)?;
        let thrift_view = view_to_thrift(view);
        write_thrift(&thrift_view, &mut temp_file.as_file())?;
        persist_content_addressed_temp_file(temp_file, self.view_path(&id))?;
        Ok(id)
    }

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let path = self.operation_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;
        let thrift_operation = read_thrift(&mut file)?;
        Ok(operation_from_thrift(&thrift_operation))
    }

    fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        let id = hash_operation(operation);
        let temp_file = NamedTempFile::new_in(&self.path)?;
        let thrift_operation = operation_to_thrift(operation);
        write_thrift(&thrift_operation, &mut temp_file.as_file())?;
        persist_content_addressed_temp_file(temp_file, self.operation_path(&id))?;
        Ok(id)
    }
}

fn hash_ref_target(ref_target: &RefTarget) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    match ref_target {
        RefTarget::Normal(id) => {
            hasher.update(b"0");
            hasher.update(id.as_bytes());
        }
        RefTarget::Conflict { removes, adds } => {
            hasher.update(b"1");
            for id in removes {
                hasher.update(b"0");
                hasher.update(id.as_bytes());
            }
            for id in adds {
                hasher.update(b"1");
                hasher.update(id.as_bytes());
            }
        }
    }
    hasher.finalize().to_vec()
}

fn hash_branch_target(branch_target: &BranchTarget) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    match &branch_target.local_target {
        None => {
            hasher.update(b"0");
        }
        Some(ref_target) => {
            hasher.update(b"1");
            hasher.update(&hash_ref_target(ref_target));
        }
    }
    for (name, ref_target) in branch_target
        .remote_targets
        .iter()
        .sorted_by_key(|(name, _)| name.clone())
    {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&hash_ref_target(ref_target));
    }
    hasher.finalize().to_vec()
}

fn hash_heads(heads: &HashSet<CommitId>) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    for head in heads.iter().sorted() {
        hasher.update(head.as_bytes());
    }
    hasher.finalize().to_vec()
}

fn hash_branches(branches: &BTreeMap<String, BranchTarget>) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    for (name, target) in branches.iter() {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&hash_branch_target(target));
    }
    hasher.finalize().to_vec()
}

fn hash_refs(refs: &BTreeMap<String, RefTarget>) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    for (name, target) in refs.iter() {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&hash_ref_target(target));
    }
    hasher.finalize().to_vec()
}

fn hash_wc_commits(wc_commit_ids: &HashMap<WorkspaceId, CommitId>) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    for (workspace_id, commit_id) in wc_commit_ids.iter().sorted_by_key(|(name, _)| name.clone()) {
        hasher.update(workspace_id.as_str().as_bytes());
        hasher.update(&[0]);
        hasher.update(commit_id.as_bytes());
    }
    hasher.finalize().to_vec()
}

fn hash_git_head(git_head: &Option<CommitId>) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    match git_head {
        None => {
            hasher.update(b"0");
        }
        Some(commit_id) => {
            hasher.update(b"1");
            hasher.update(commit_id.as_bytes());
        }
    }
    hasher.finalize().to_vec()
}

fn hash_view(view: &View) -> ViewId {
    let View {
        head_ids,
        public_head_ids,
        branches,
        tags,
        git_refs,
        git_head,
        wc_commit_ids,
    } = view;
    let mut hasher = Blake2b512::new();
    hasher.update(&hash_heads(head_ids));
    hasher.update(&hash_heads(public_head_ids));
    hasher.update(&hash_branches(branches));
    hasher.update(&hash_refs(tags));
    hasher.update(&hash_refs(git_refs));
    hasher.update(&hash_git_head(git_head));
    hasher.update(&hash_wc_commits(wc_commit_ids));
    ViewId::new(hasher.finalize().to_vec())
}

fn hash_tags(tags: &HashMap<String, String>) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    for (name, value) in tags.iter().sorted_by_key(|(name, _)| name.clone()) {
        hasher.update(name.as_str().as_bytes());
        hasher.update(&[0]);
        hasher.update(value.as_str().as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_vec()
}

fn timestamp_to_bytes(timestamp: &Timestamp) -> Vec<u8> {
    let mut bytes = vec![];
    bytes
        .write_i64::<LittleEndian>(timestamp.timestamp.0)
        .unwrap();
    bytes
        .write_i32::<LittleEndian>(timestamp.tz_offset)
        .unwrap();
    bytes
}

fn hash_operation_metadata(metadata: &OperationMetadata) -> Vec<u8> {
    let OperationMetadata {
        start_time,
        end_time,
        description,
        hostname,
        username,
        tags,
    } = metadata;
    let mut hasher = Blake2b512::new();
    hasher.update(&timestamp_to_bytes(start_time));
    hasher.update(&timestamp_to_bytes(end_time));
    hasher.update(description.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(hostname.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(username.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(&hash_tags(tags));
    hasher.finalize().to_vec()
}

fn hash_operation(operation: &Operation) -> OperationId {
    let Operation {
        view_id,
        parents,
        metadata,
    } = operation;
    let mut view_hasher = Blake2b512::new();
    view_hasher.update(view_id.as_bytes());
    for parent in parents {
        view_hasher.update(parent.as_bytes());
    }
    view_hasher.update(&hash_operation_metadata(metadata));
    OperationId::new(view_hasher.finalize().to_vec())
}

fn read_thrift<T: TSerializable>(input: &mut impl Read) -> OpStoreResult<T> {
    let mut protocol = TCompactInputProtocol::new(input);
    Ok(TSerializable::read_from_in_protocol(&mut protocol).unwrap())
}

fn write_thrift<T: TSerializable>(thrift_object: &T, output: &mut impl Write) -> OpStoreResult<()> {
    let mut protocol = TCompactOutputProtocol::new(output);
    thrift_object.write_to_out_protocol(&mut protocol)?;
    Ok(())
}

fn timestamp_to_thrift(timestamp: &Timestamp) -> simple_op_store_model::Timestamp {
    simple_op_store_model::Timestamp::new(timestamp.timestamp.0, timestamp.tz_offset)
}

fn timestamp_from_thrift(proto: &simple_op_store_model::Timestamp) -> Timestamp {
    Timestamp {
        timestamp: MillisSinceEpoch(proto.millis_since_epoch),
        tz_offset: proto.tz_offset,
    }
}

fn operation_metadata_to_thrift(
    metadata: &OperationMetadata,
) -> simple_op_store_model::OperationMetadata {
    let start_time = timestamp_to_thrift(&metadata.start_time);
    let end_time = timestamp_to_thrift(&metadata.end_time);
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

fn operation_metadata_from_thrift(
    thrift: &simple_op_store_model::OperationMetadata,
) -> OperationMetadata {
    let start_time = timestamp_from_thrift(&thrift.start_time);
    let end_time = timestamp_from_thrift(&thrift.end_time);
    let description = thrift.description.to_owned();
    let hostname = thrift.hostname.to_owned();
    let username = thrift.username.to_owned();
    let tags = thrift
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

impl From<&Operation> for simple_op_store_model::Operation {
    fn from(operation: &Operation) -> Self {
        operation_to_thrift(operation)
    }
}

impl From<&View> for simple_op_store_model::View {
    fn from(view: &View) -> Self {
        view_to_thrift(view)
    }
}

fn operation_to_thrift(operation: &Operation) -> simple_op_store_model::Operation {
    let view_id = operation.view_id.as_bytes().to_vec();
    let mut parents = vec![];
    for parent in &operation.parents {
        parents.push(parent.to_bytes());
    }
    let metadata = Box::new(operation_metadata_to_thrift(&operation.metadata));
    simple_op_store_model::Operation::new(view_id, parents, metadata)
}

fn operation_from_thrift(thrift: &simple_op_store_model::Operation) -> Operation {
    let operation_id_from_thrift = |parent: &Vec<u8>| OperationId::new(parent.clone());
    let parents = thrift
        .parents
        .iter()
        .map(operation_id_from_thrift)
        .collect();
    let view_id = ViewId::new(thrift.view_id.clone());
    let metadata = operation_metadata_from_thrift(&thrift.metadata);
    Operation {
        view_id,
        parents,
        metadata,
    }
}

fn view_to_thrift(view: &View) -> simple_op_store_model::View {
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
        let local_target = target.local_target.as_ref().map(ref_target_to_thrift);
        let mut remote_branches = vec![];
        for (remote_name, target) in &target.remote_targets {
            remote_branches.push(simple_op_store_model::RemoteBranch::new(
                remote_name.clone(),
                ref_target_to_thrift(target),
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
            ref_target_to_thrift(target),
        ));
    }

    let mut git_refs = vec![];
    for (git_ref_name, target) in &view.git_refs {
        git_refs.push(simple_op_store_model::GitRef::new(
            git_ref_name.clone(),
            ref_target_to_thrift(target),
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

fn view_from_thrift(thrift: &simple_op_store_model::View) -> View {
    let mut view = View::default();
    for (workspace_id, commit_id) in thrift.wc_commit_ids.iter() {
        view.wc_commit_ids.insert(
            WorkspaceId::new(workspace_id.clone()),
            CommitId::new(commit_id.clone()),
        );
    }
    for head_id_bytes in thrift.head_ids.iter() {
        view.head_ids.insert(CommitId::from_bytes(head_id_bytes));
    }
    for head_id_bytes in thrift.public_head_ids.iter() {
        view.public_head_ids
            .insert(CommitId::from_bytes(head_id_bytes));
    }

    for thrift_branch in thrift.branches.iter() {
        let local_target = thrift_branch
            .local_target
            .as_ref()
            .map(ref_target_from_thrift);

        let mut remote_targets = BTreeMap::new();
        for remote_branch in thrift_branch.remote_branches.iter() {
            remote_targets.insert(
                remote_branch.remote_name.clone(),
                ref_target_from_thrift(&remote_branch.target),
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

    for thrift_tag in thrift.tags.iter() {
        view.tags.insert(
            thrift_tag.name.clone(),
            ref_target_from_thrift(&thrift_tag.target),
        );
    }

    for git_ref in thrift.git_refs.iter() {
        view.git_refs.insert(
            git_ref.name.clone(),
            ref_target_from_thrift(&git_ref.target),
        );
    }

    view.git_head = thrift
        .git_head
        .as_ref()
        .map(|head| CommitId::new(head.clone()));

    view
}

fn ref_target_to_thrift(value: &RefTarget) -> simple_op_store_model::RefTarget {
    match value {
        RefTarget::Normal(id) => simple_op_store_model::RefTarget::CommitId(id.to_bytes()),
        RefTarget::Conflict { removes, adds } => {
            let adds = adds.iter().map(|id| id.to_bytes()).collect_vec();
            let removes = removes.iter().map(|id| id.to_bytes()).collect_vec();
            let ref_conflict_thrift = simple_op_store_model::RefConflict::new(removes, adds);
            simple_op_store_model::RefTarget::Conflict(ref_conflict_thrift)
        }
    }
}

fn ref_target_from_thrift(thrift: &simple_op_store_model::RefTarget) -> RefTarget {
    match thrift {
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

#[cfg(test)]
mod tests {
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
    fn test_hash_ref_target() {
        let id1 = CommitId::from_hex("aaa111");
        let id2 = CommitId::from_hex("aaa222");

        // Different non-conflicts give different hash
        assert_ne!(
            hash_ref_target(&RefTarget::Normal(id1.clone())),
            hash_ref_target(&RefTarget::Normal(id2.clone()))
        );
        // Different conflicts give different hash
        assert_ne!(
            hash_ref_target(&RefTarget::Conflict {
                removes: vec![id1.clone()],
                adds: vec![id2.clone()],
            }),
            hash_ref_target(&RefTarget::Conflict {
                removes: vec![id2.clone()],
                adds: vec![id1.clone()],
            })
        );
        // Conflict "-A" and conflict "+A" are not confused
        assert_ne!(
            hash_ref_target(&RefTarget::Conflict {
                removes: vec![id1.clone()],
                adds: vec![],
            }),
            hash_ref_target(&RefTarget::Conflict {
                removes: vec![],
                adds: vec![id1.clone()],
            })
        );
        // Non-conflict "A" and conflict "+A" are not confused
        assert_ne!(
            hash_ref_target(&RefTarget::Normal(id1.clone())),
            hash_ref_target(&RefTarget::Conflict {
                removes: vec![],
                adds: vec![id1.clone()],
            })
        );
        // Non-conflict "A" and conflict "+A" are not confused
        assert_ne!(
            hash_ref_target(&RefTarget::Normal(id1.clone())),
            hash_ref_target(&RefTarget::Conflict {
                removes: vec![],
                adds: vec![id1.clone()],
            })
        );
    }

    #[test]
    fn test_hash_branch_target() {
        let id1 = CommitId::from_hex("fff999");
        let id2 = CommitId::from_hex("fff888");

        // Missing local target and present local target
        assert_ne!(
            hash_branch_target(&BranchTarget {
                local_target: None,
                remote_targets: BTreeMap::new()
            }),
            hash_branch_target(&BranchTarget {
                local_target: Some(RefTarget::Normal(id1.clone())),
                remote_targets: BTreeMap::new()
            })
        );
        // Different local targets
        assert_ne!(
            hash_branch_target(&BranchTarget {
                local_target: Some(RefTarget::Normal(id1.clone())),
                remote_targets: BTreeMap::new()
            }),
            hash_branch_target(&BranchTarget {
                local_target: Some(RefTarget::Normal(id2.clone())),
                remote_targets: BTreeMap::new()
            })
        );
        // Different remote branch target
        assert_ne!(
            hash_branch_target(&BranchTarget {
                local_target: None,
                remote_targets: btreemap! { "origin".to_string() => RefTarget::Normal(id1.clone()) }
            }),
            hash_branch_target(&BranchTarget {
                local_target: None,
                remote_targets: btreemap! { "origin".to_string() => RefTarget::Normal(id2.clone()) }
            })
        );
        // Different remote name
        assert_ne!(
            hash_branch_target(&BranchTarget {
                local_target: None,
                remote_targets: btreemap! { "origin".to_string() => RefTarget::Normal(id1.clone()) }
            }),
            hash_branch_target(&BranchTarget {
                local_target: None,
                remote_targets: btreemap! { "source".to_string() => RefTarget::Normal(id2.clone()) }
            })
        );
    }

    #[test]
    fn test_hash_view() {
        let base_view = create_view();
        let id1 = CommitId::from_hex("aaa111");
        let id2 = CommitId::from_hex("aaa222");

        // Different head_ids
        assert_ne!(
            hash_view(&View {
                head_ids: hashset! {id1.clone()},
                ..base_view.clone()
            }),
            hash_view(&View {
                head_ids: hashset! {id1.clone(), id2.clone()},
                ..base_view.clone()
            })
        );
        // Different public_head_ids
        assert_ne!(
            hash_view(&View {
                public_head_ids: hashset! {id1.clone()},
                ..base_view.clone()
            }),
            hash_view(&View {
                public_head_ids: hashset! {id1.clone(), id2.clone()},
                ..base_view.clone()
            })
        );
        // head_ids and public_head_ids are not confused
        assert_ne!(
            hash_view(&View {
                head_ids: hashset! {id1.clone()},
                public_head_ids: hashset! {},
                ..base_view.clone()
            }),
            hash_view(&View {
                head_ids: hashset! {},
                public_head_ids: hashset! {id1.clone()},
                ..base_view.clone()
            })
        );
        // Different branch names
        assert_ne!(
            hash_view(&View {
                branches: btreemap! { "main".to_string() => BranchTarget {
                    local_target: Some(RefTarget::Normal(id1.clone())),
                    remote_targets: BTreeMap::new(),
                } },
                ..base_view.clone()
            }),
            hash_view(&View {
                branches: btreemap! { "other".to_string() => BranchTarget {
                    local_target: Some(RefTarget::Normal(id1.clone())),
                    remote_targets: BTreeMap::new(),
                } },
                ..base_view.clone()
            })
        );
        // Different branch targets
        assert_ne!(
            hash_view(&View {
                branches: btreemap! { "main".to_string() => BranchTarget {
                    local_target: Some(RefTarget::Normal(id1.clone())),
                    remote_targets: BTreeMap::new(),
                } },
                ..base_view.clone()
            }),
            hash_view(&View {
                branches: btreemap! { "main".to_string() => BranchTarget {
                    local_target: Some(RefTarget::Normal(id2.clone())),
                    remote_targets: BTreeMap::new(),
                } },
                ..base_view.clone()
            })
        );
        // Different tag names
        assert_ne!(
            hash_view(&View {
                tags: btreemap! { "tag1".to_string() => RefTarget::Normal(id1.clone()) },
                ..base_view.clone()
            }),
            hash_view(&View {
                tags: btreemap! { "tag2".to_string() => RefTarget::Normal(id1.clone()) },
                ..base_view.clone()
            })
        );
        // Different tag targets
        assert_ne!(
            hash_view(&View {
                tags: btreemap! { "tag1".to_string() => RefTarget::Normal(id1.clone()) },
                ..base_view.clone()
            }),
            hash_view(&View {
                tags: btreemap! { "tag1".to_string() => RefTarget::Normal(id2.clone()) },
                ..base_view.clone()
            })
        );
        // Different git ref names
        assert_ne!(
            hash_view(&View {
                git_refs: btreemap! { "refs/foo".to_string() => RefTarget::Normal(id1.clone()) },
                ..base_view.clone()
            }),
            hash_view(&View {
                git_refs: btreemap! { "refs/bar".to_string() => RefTarget::Normal(id1.clone()) },
                ..base_view.clone()
            })
        );
        // Different git ref targets
        assert_ne!(
            hash_view(&View {
                git_refs: btreemap! { "refs/foo".to_string() => RefTarget::Normal(id1.clone()) },
                ..base_view.clone()
            }),
            hash_view(&View {
                git_refs: btreemap! { "refs/foo".to_string() => RefTarget::Normal(id2.clone()) },
                ..base_view.clone()
            })
        );
        // Absent vs present git_head
        assert_ne!(
            hash_view(&View {
                git_head: None,
                ..base_view.clone()
            }),
            hash_view(&View {
                git_head: Some(id1.clone()),
                ..base_view.clone()
            })
        );
        // Different git_head
        assert_ne!(
            hash_view(&View {
                git_head: Some(id1.clone()),
                ..base_view.clone()
            }),
            hash_view(&View {
                git_head: Some(id2.clone()),
                ..base_view.clone()
            })
        );
        // Different workspace names
        assert_ne!(
            hash_view(&View {
                wc_commit_ids: hashmap! { WorkspaceId::new("main".to_string()) => id1.clone() },
                ..base_view.clone()
            }),
            hash_view(&View {
                wc_commit_ids: hashmap! { WorkspaceId::new("test".to_string()) => id1.clone() },
                ..base_view.clone()
            })
        );
        // Different workspace commits
        assert_ne!(
            hash_view(&View {
                wc_commit_ids: hashmap! { WorkspaceId::new("main".to_string()) => id1.clone() },
                ..base_view.clone()
            }),
            hash_view(&View {
                wc_commit_ids: hashmap! { WorkspaceId::new("main".to_string()) => id2.clone() },
                ..base_view.clone()
            })
        );
    }

    #[test]
    fn test_hash_operation_metadata() {
        let base_metadata = create_operation().metadata;
        // Different start timestamp
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                start_time: Timestamp {
                    timestamp: MillisSinceEpoch(0),
                    tz_offset: 0,
                },
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                start_time: Timestamp {
                    timestamp: MillisSinceEpoch(1),
                    tz_offset: 0,
                },
                ..base_metadata.clone()
            }),
        );
        // Different start timezone
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                start_time: Timestamp {
                    timestamp: MillisSinceEpoch(0),
                    tz_offset: 0,
                },
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                start_time: Timestamp {
                    timestamp: MillisSinceEpoch(0),
                    tz_offset: 1,
                },
                ..base_metadata.clone()
            }),
        );
        // Different end timestamp
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                end_time: Timestamp {
                    timestamp: MillisSinceEpoch(0),
                    tz_offset: 0,
                },
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                end_time: Timestamp {
                    timestamp: MillisSinceEpoch(1),
                    tz_offset: 0,
                },
                ..base_metadata.clone()
            }),
        );
        // Different description
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                description: "home".to_string(),
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                description: "work".to_string(),
                ..base_metadata.clone()
            }),
        );
        // Different username
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                username: "alice".to_string(),
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                username: "bob".to_string(),
                ..base_metadata.clone()
            }),
        );
        // Different hostname
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                hostname: "home".to_string(),
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                hostname: "work".to_string(),
                ..base_metadata.clone()
            }),
        );
        // Different tag name
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                tags: hashmap! { "key1".to_string() => "value".to_string() },
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                tags: hashmap! { "key2".to_string() => "value".to_string() },
                ..base_metadata.clone()
            }),
        );
        // Different tag value
        assert_ne!(
            hash_operation_metadata(&OperationMetadata {
                tags: hashmap! { "key".to_string() => "value1".to_string() },
                ..base_metadata.clone()
            }),
            hash_operation_metadata(&OperationMetadata {
                tags: hashmap! { "key".to_string() => "value2".to_string() },
                ..base_metadata.clone()
            }),
        );
    }

    #[test]
    fn test_hash_operation() {
        let base_operation = create_operation();
        // Different view ID
        assert_ne!(
            hash_operation(&Operation {
                view_id: ViewId::from_hex("aaa111"),
                ..base_operation.clone()
            }),
            hash_operation(&Operation {
                view_id: ViewId::from_hex("aaa222"),
                ..base_operation.clone()
            })
        );
        // Different parents
        assert_ne!(
            hash_operation(&Operation {
                parents: vec![OperationId::from_hex("aaa111")],
                ..base_operation.clone()
            }),
            hash_operation(&Operation {
                parents: vec![OperationId::from_hex("aaa222")],
                ..base_operation.clone()
            })
        );
        // Different metadata
        assert_ne!(
            hash_operation(&Operation {
                metadata: OperationMetadata {
                    username: "alice".to_string(),
                    ..base_operation.metadata.clone()
                },
                ..base_operation.clone()
            }),
            hash_operation(&Operation {
                metadata: OperationMetadata {
                    username: "bob".to_string(),
                    ..base_operation.metadata.clone()
                },
                ..base_operation.clone()
            })
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
