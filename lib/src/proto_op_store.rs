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

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::ErrorKind;
use std::path::PathBuf;

use itertools::Itertools;
use protobuf::{Message, MessageField};
use tempfile::NamedTempFile;

use crate::backend::{CommitId, MillisSinceEpoch, Timestamp};
use crate::content_hash::blake2b_hash;
use crate::file_util::persist_content_addressed_temp_file;
use crate::op_store::{
    BranchTarget, OpStoreError, OpStoreResult, Operation, OperationId, OperationMetadata,
    RefTarget, View, ViewId, WorkspaceId,
};

impl From<protobuf::Error> for OpStoreError {
    fn from(err: protobuf::Error) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

#[derive(Debug)]
pub struct ProtoOpStore {
    path: PathBuf,
}

impl ProtoOpStore {
    pub fn init(store_path: PathBuf) -> Self {
        fs::create_dir(store_path.join("views")).unwrap();
        fs::create_dir(store_path.join("operations")).unwrap();
        ProtoOpStore { path: store_path }
    }

    pub fn load(store_path: PathBuf) -> Self {
        ProtoOpStore { path: store_path }
    }

    fn view_path(&self, id: &ViewId) -> PathBuf {
        self.path.join("views").join(id.hex())
    }

    fn operation_path(&self, id: &OperationId) -> PathBuf {
        self.path.join("operations").join(id.hex())
    }

    pub fn read_view(&self, id: &ViewId) -> OpStoreResult<View> {
        let path = self.view_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;

        let proto: crate::protos::op_store::View = Message::parse_from_reader(&mut file)?;
        Ok(view_from_proto(&proto))
    }

    pub fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = view_to_proto(view);
        proto.write_to_writer(&mut temp_file.as_file())?;

        let id = ViewId::new(blake2b_hash(view).to_vec());

        persist_content_addressed_temp_file(temp_file, self.view_path(&id))?;
        Ok(id)
    }

    pub fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let path = self.operation_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;

        let proto: crate::protos::op_store::Operation = Message::parse_from_reader(&mut file)?;
        Ok(operation_from_proto(&proto))
    }

    pub fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = operation_to_proto(operation);
        proto.write_to_writer(&mut temp_file.as_file())?;

        let id = OperationId::new(blake2b_hash(operation).to_vec());

        persist_content_addressed_temp_file(temp_file, self.operation_path(&id))?;
        Ok(id)
    }
}

fn not_found_to_store_error(err: std::io::Error) -> OpStoreError {
    if err.kind() == ErrorKind::NotFound {
        OpStoreError::NotFound
    } else {
        OpStoreError::from(err)
    }
}

fn timestamp_to_proto(timestamp: &Timestamp) -> crate::protos::op_store::Timestamp {
    let mut proto = crate::protos::op_store::Timestamp::new();
    proto.millis_since_epoch = timestamp.timestamp.0;
    proto.tz_offset = timestamp.tz_offset;
    proto
}

fn timestamp_from_proto(proto: &crate::protos::op_store::Timestamp) -> Timestamp {
    Timestamp {
        timestamp: MillisSinceEpoch(proto.millis_since_epoch),
        tz_offset: proto.tz_offset,
    }
}

fn operation_metadata_to_proto(
    metadata: &OperationMetadata,
) -> crate::protos::op_store::OperationMetadata {
    let mut proto = crate::protos::op_store::OperationMetadata::new();
    proto.start_time = MessageField::some(timestamp_to_proto(&metadata.start_time));
    proto.end_time = MessageField::some(timestamp_to_proto(&metadata.end_time));
    proto.description = metadata.description.clone();
    proto.hostname = metadata.hostname.clone();
    proto.username = metadata.username.clone();
    proto.tags = metadata.tags.clone();
    proto
}

fn operation_metadata_from_proto(
    proto: &crate::protos::op_store::OperationMetadata,
) -> OperationMetadata {
    let start_time = timestamp_from_proto(&proto.start_time);
    let end_time = timestamp_from_proto(&proto.end_time);
    let description = proto.description.to_owned();
    let hostname = proto.hostname.to_owned();
    let username = proto.username.to_owned();
    let tags = proto.tags.clone();
    OperationMetadata {
        start_time,
        end_time,
        description,
        hostname,
        username,
        tags,
    }
}

fn operation_to_proto(operation: &Operation) -> crate::protos::op_store::Operation {
    let mut proto = crate::protos::op_store::Operation::new();
    proto.view_id = operation.view_id.as_bytes().to_vec();
    for parent in &operation.parents {
        proto.parents.push(parent.to_bytes());
    }
    proto.metadata = MessageField::some(operation_metadata_to_proto(&operation.metadata));
    proto
}

fn operation_from_proto(proto: &crate::protos::op_store::Operation) -> Operation {
    let operation_id_from_proto = |parent: &Vec<u8>| OperationId::new(parent.clone());
    let parents = proto.parents.iter().map(operation_id_from_proto).collect();
    let view_id = ViewId::new(proto.view_id.clone());
    let metadata = operation_metadata_from_proto(&proto.metadata);
    Operation {
        view_id,
        parents,
        metadata,
    }
}

fn view_to_proto(view: &View) -> crate::protos::op_store::View {
    let mut proto = crate::protos::op_store::View::new();
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

    for (name, target) in &view.branches {
        let mut branch_proto = crate::protos::op_store::Branch::new();
        branch_proto.name = name.clone();
        if let Some(local_target) = &target.local_target {
            branch_proto.local_target = MessageField::some(ref_target_to_proto(local_target));
        }
        for (remote_name, target) in &target.remote_targets {
            let mut remote_branch_proto = crate::protos::op_store::RemoteBranch::new();
            remote_branch_proto.remote_name = remote_name.clone();
            remote_branch_proto.target = MessageField::some(ref_target_to_proto(target));
            branch_proto.remote_branches.push(remote_branch_proto);
        }
        proto.branches.push(branch_proto);
    }

    for (name, target) in &view.tags {
        let mut tag_proto = crate::protos::op_store::Tag::new();
        tag_proto.name = name.clone();
        tag_proto.target = MessageField::some(ref_target_to_proto(target));
        proto.tags.push(tag_proto);
    }

    for (git_ref_name, target) in &view.git_refs {
        let mut git_ref_proto = crate::protos::op_store::GitRef::new();
        git_ref_proto.name = git_ref_name.clone();
        git_ref_proto.target = MessageField::some(ref_target_to_proto(target));
        proto.git_refs.push(git_ref_proto);
    }

    if let Some(git_head) = &view.git_head {
        proto.git_head = git_head.to_bytes();
    }

    proto
}

fn view_from_proto(proto: &crate::protos::op_store::View) -> View {
    let mut view = View::default();
    // For compatibility with old repos before we had support for multiple working
    // copies
    if !proto.wc_commit_id.is_empty() {
        view.wc_commit_ids.insert(
            WorkspaceId::default(),
            CommitId::new(proto.wc_commit_id.clone()),
        );
    }
    for (workspace_id, commit_id) in &proto.wc_commit_ids {
        view.wc_commit_ids.insert(
            WorkspaceId::new(workspace_id.clone()),
            CommitId::new(commit_id.clone()),
        );
    }
    for head_id_bytes in &proto.head_ids {
        view.head_ids.insert(CommitId::from_bytes(head_id_bytes));
    }
    for head_id_bytes in &proto.public_head_ids {
        view.public_head_ids
            .insert(CommitId::from_bytes(head_id_bytes));
    }

    for branch_proto in &proto.branches {
        let local_target = branch_proto
            .local_target
            .as_ref()
            .map(ref_target_from_proto);

        let mut remote_targets = BTreeMap::new();
        for remote_branch in &branch_proto.remote_branches {
            remote_targets.insert(
                remote_branch.remote_name.clone(),
                ref_target_from_proto(&remote_branch.target),
            );
        }

        view.branches.insert(
            branch_proto.name.clone(),
            BranchTarget {
                local_target,
                remote_targets,
            },
        );
    }

    for tag_proto in &proto.tags {
        view.tags.insert(
            tag_proto.name.clone(),
            ref_target_from_proto(&tag_proto.target),
        );
    }

    for git_ref in &proto.git_refs {
        if let Some(target) = git_ref.target.as_ref() {
            view.git_refs
                .insert(git_ref.name.clone(), ref_target_from_proto(target));
        } else {
            // Legacy format
            view.git_refs.insert(
                git_ref.name.clone(),
                RefTarget::Normal(CommitId::new(git_ref.commit_id.clone())),
            );
        }
    }

    if !proto.git_head.is_empty() {
        view.git_head = Some(CommitId::new(proto.git_head.clone()));
    }

    view
}

fn ref_target_to_proto(value: &RefTarget) -> crate::protos::op_store::RefTarget {
    let mut proto = crate::protos::op_store::RefTarget::new();
    match value {
        RefTarget::Normal(id) => {
            proto.set_commit_id(id.to_bytes());
        }
        RefTarget::Conflict { removes, adds } => {
            let mut ref_conflict_proto = crate::protos::op_store::RefConflict::new();
            for id in removes {
                ref_conflict_proto.removes.push(id.to_bytes());
            }
            for id in adds {
                ref_conflict_proto.adds.push(id.to_bytes());
            }
            proto.set_conflict(ref_conflict_proto);
        }
    }
    proto
}

fn ref_target_from_proto(proto: &crate::protos::op_store::RefTarget) -> RefTarget {
    match proto.value.as_ref().unwrap() {
        crate::protos::op_store::ref_target::Value::CommitId(id) => {
            RefTarget::Normal(CommitId::from_bytes(id))
        }
        crate::protos::op_store::ref_target::Value::Conflict(conflict) => {
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
