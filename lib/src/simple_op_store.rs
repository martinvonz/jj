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

use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use blake2::{Blake2b, Digest};
use protobuf::{Message, ProtobufError};
use tempfile::{NamedTempFile, PersistError};

use crate::file_util::persist_content_addressed_temp_file;
use crate::op_store::{
    OpStore, OpStoreError, OpStoreResult, Operation, OperationId, OperationMetadata, View, ViewId,
};
use crate::store::{CommitId, MillisSinceEpoch, Timestamp};

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

impl From<ProtobufError> for OpStoreError {
    fn from(err: ProtobufError) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

#[derive(Debug)]
pub struct SimpleOpStore {
    path: PathBuf,
}

impl SimpleOpStore {
    pub fn init(store_path: PathBuf) -> Self {
        fs::create_dir(store_path.join("views")).unwrap();
        fs::create_dir(store_path.join("operations")).unwrap();
        Self::load(store_path)
    }

    pub fn load(store_path: PathBuf) -> Self {
        SimpleOpStore { path: store_path }
    }

    fn view_path(&self, id: &ViewId) -> PathBuf {
        self.path.join("views").join(id.hex())
    }

    fn operation_path(&self, id: &OperationId) -> PathBuf {
        self.path.join("operations").join(id.hex())
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
        let path = self.view_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;

        let proto: crate::protos::op_store::View = Message::parse_from_reader(&mut file)?;
        Ok(view_from_proto(&proto))
    }

    fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = view_to_proto(view);
        let mut proto_bytes: Vec<u8> = Vec::new();
        proto.write_to_writer(&mut proto_bytes)?;

        temp_file.as_file().write_all(&proto_bytes)?;

        let id = ViewId(Blake2b::digest(&proto_bytes).to_vec());

        persist_content_addressed_temp_file(temp_file, self.view_path(&id))?;
        Ok(id)
    }

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let path = self.operation_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;

        let proto: crate::protos::op_store::Operation = Message::parse_from_reader(&mut file)?;
        Ok(operation_from_proto(&proto))
    }

    fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = operation_to_proto(operation);
        let mut proto_bytes: Vec<u8> = Vec::new();
        proto.write_to_writer(&mut proto_bytes)?;

        temp_file.as_file().write_all(&proto_bytes)?;

        let id = OperationId(Blake2b::digest(&proto_bytes).to_vec());

        persist_content_addressed_temp_file(temp_file, self.operation_path(&id))?;
        Ok(id)
    }
}

fn timestamp_to_proto(timestamp: &Timestamp) -> crate::protos::op_store::Timestamp {
    let mut proto = crate::protos::op_store::Timestamp::new();
    proto.set_millis_since_epoch(timestamp.timestamp.0);
    proto.set_tz_offset(timestamp.tz_offset);
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
    proto.set_start_time(timestamp_to_proto(&metadata.start_time));
    proto.set_end_time(timestamp_to_proto(&metadata.end_time));
    proto.set_description(metadata.description.clone());
    proto.set_hostname(metadata.hostname.clone());
    proto.set_username(metadata.username.clone());
    proto.set_tags(metadata.tags.clone());
    proto
}

fn operation_metadata_from_proto(
    proto: &crate::protos::op_store::OperationMetadata,
) -> OperationMetadata {
    let start_time = timestamp_from_proto(proto.get_start_time());
    let end_time = timestamp_from_proto(proto.get_end_time());
    let description = proto.get_description().to_owned();
    let hostname = proto.get_hostname().to_owned();
    let username = proto.get_username().to_owned();
    let tags = proto.get_tags().clone();
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
    proto.set_view_id(operation.view_id.0.clone());
    for parent in &operation.parents {
        proto.parents.push(parent.0.clone());
    }
    proto.set_metadata(operation_metadata_to_proto(&operation.metadata));
    proto
}

fn operation_from_proto(proto: &crate::protos::op_store::Operation) -> Operation {
    let operation_id_from_proto = |parent: &Vec<u8>| OperationId(parent.clone());
    let parents = proto.parents.iter().map(operation_id_from_proto).collect();
    let view_id = ViewId(proto.view_id.to_vec());
    let metadata = operation_metadata_from_proto(proto.get_metadata());
    Operation {
        view_id,
        parents,
        metadata,
    }
}

fn view_to_proto(view: &View) -> crate::protos::op_store::View {
    let mut proto = crate::protos::op_store::View::new();
    proto.checkout = view.checkout.0.clone();
    for head_id in &view.head_ids {
        proto.head_ids.push(head_id.0.clone());
    }
    for head_id in &view.public_head_ids {
        proto.public_head_ids.push(head_id.0.clone());
    }
    for (git_ref_name, commit_id) in &view.git_refs {
        let mut git_ref_proto = crate::protos::op_store::GitRef::new();
        git_ref_proto.set_name(git_ref_name.clone());
        git_ref_proto.set_commit_id(commit_id.0.clone());
        proto.git_refs.push(git_ref_proto);
    }
    proto
}

fn view_from_proto(proto: &crate::protos::op_store::View) -> View {
    let mut view = View::new(CommitId(proto.checkout.clone()));
    for head_id_bytes in proto.head_ids.iter() {
        view.head_ids.insert(CommitId(head_id_bytes.to_vec()));
    }
    for head_id_bytes in proto.public_head_ids.iter() {
        view.public_head_ids
            .insert(CommitId(head_id_bytes.to_vec()));
    }
    for git_ref in proto.git_refs.iter() {
        view.git_refs
            .insert(git_ref.name.clone(), CommitId(git_ref.commit_id.to_vec()));
    }
    view
}
