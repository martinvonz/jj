// Copyright 2022 The Jujutsu Authors
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
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;

use itertools::Itertools;
use thrift::protocol::{TCompactInputProtocol, TSerializable};

use crate::backend::{CommitId, MillisSinceEpoch, Timestamp};
use crate::op_store::{
    BranchTarget, OpStoreError, OpStoreResult, Operation, OperationId, OperationMetadata,
    RefTarget, View, ViewId, WorkspaceId,
};
use crate::simple_op_store_model;

impl From<thrift::Error> for OpStoreError {
    fn from(err: thrift::Error) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

fn not_found_to_store_error(err: std::io::Error) -> OpStoreError {
    if err.kind() == ErrorKind::NotFound {
        OpStoreError::NotFound
    } else {
        OpStoreError::from(err)
    }
}

#[derive(Debug)]
pub struct ThriftOpStore {
    path: PathBuf,
}

impl ThriftOpStore {
    pub fn load(store_path: PathBuf) -> Self {
        ThriftOpStore { path: store_path }
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
        let thrift_view = read_thrift(&mut file)?;
        Ok(View::from(&thrift_view))
    }

    pub fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let path = self.operation_path(id);
        let mut file = File::open(path).map_err(not_found_to_store_error)?;
        let thrift_operation = read_thrift(&mut file)?;
        Ok(Operation::from(&thrift_operation))
    }
}

pub fn read_thrift<T: TSerializable>(input: &mut impl Read) -> OpStoreResult<T> {
    let mut protocol = TCompactInputProtocol::new(input);
    Ok(TSerializable::read_from_in_protocol(&mut protocol).unwrap())
}

impl From<&simple_op_store_model::Timestamp> for Timestamp {
    fn from(timestamp: &simple_op_store_model::Timestamp) -> Self {
        Timestamp {
            timestamp: MillisSinceEpoch(timestamp.millis_since_epoch),
            tz_offset: timestamp.tz_offset,
        }
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
