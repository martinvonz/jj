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

use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::Timestamp;
use crate::dag_walk::closest_common_node;
use crate::index::ReadonlyIndex;
use crate::op_store;
use crate::op_store::OperationMetadata;
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo, RepoLoader};
use crate::view::View;

pub struct Transaction {
    repo: Option<MutableRepo>,
    parent_ops: Vec<Operation>,
    description: String,
    start_time: Timestamp,
    tags: HashMap<String, String>,
}

impl Transaction {
    pub fn new(mut_repo: MutableRepo, description: &str) -> Transaction {
        let parent_ops = vec![mut_repo.base_repo().operation().clone()];
        Transaction {
            repo: Some(mut_repo),
            parent_ops,
            description: description.to_owned(),
            start_time: Timestamp::now(),
            tags: Default::default(),
        }
    }

    pub fn base_repo(&self) -> &ReadonlyRepo {
        self.repo.as_ref().unwrap().base_repo()
    }

    pub fn set_tag(&mut self, key: String, value: String) {
        self.tags.insert(key, value);
    }

    pub fn mut_repo(&mut self) -> &mut MutableRepo {
        self.repo.as_mut().unwrap()
    }

    pub fn merge_operation(&mut self, other_op: Operation) {
        let ancestor_op = closest_common_node(
            self.parent_ops.clone(),
            vec![other_op.clone()],
            &|op: &Operation| op.parents(),
            &|op: &Operation| op.id().clone(),
        )
        .unwrap();
        let repo_loader = self.base_repo().loader();
        let base_repo = repo_loader.load_at(&ancestor_op);
        let other_repo = repo_loader.load_at(&other_op);
        self.parent_ops.push(other_op);
        let merged_repo = self.mut_repo();
        merged_repo.merge(&base_repo, &other_repo);
    }

    /// Writes the transaction to the operation store and publishes it.
    pub fn commit(self) -> Arc<ReadonlyRepo> {
        self.write().publish()
    }

    /// Writes the transaction to the operation store, but does not publish it.
    /// That means that a repo can be loaded at the operation, but the
    /// operation will not be seen when loading the repo at head.
    pub fn write(mut self) -> UnpublishedOperation {
        let mut_repo = self.repo.take().unwrap();
        // TODO: Should we instead just do the rebasing here if necessary?
        assert!(
            !mut_repo.has_rewrites(),
            "BUG: Descendants have not been rebased after the last rewrites."
        );
        let base_repo = mut_repo.base_repo().clone();
        let (mut_index, view) = mut_repo.consume();
        let index = base_repo.index_store().write_index(mut_index).unwrap();

        let view_id = base_repo.op_store().write_view(view.store_view()).unwrap();
        let mut operation_metadata =
            OperationMetadata::new(self.description.clone(), self.start_time.clone());
        operation_metadata.tags = self.tags.clone();
        let parents = self.parent_ops.iter().map(|op| op.id().clone()).collect();
        let store_operation = op_store::Operation {
            view_id,
            parents,
            metadata: operation_metadata,
        };
        let new_op_id = base_repo
            .op_store()
            .write_operation(&store_operation)
            .unwrap();
        let operation = Operation::new(base_repo.op_store().clone(), new_op_id, store_operation);

        base_repo
            .index_store()
            .associate_file_with_operation(&index, operation.id())
            .unwrap();
        UnpublishedOperation::new(base_repo.loader(), operation, view, index)
    }
}

struct NewRepoData {
    operation: Operation,
    view: View,
    index: Arc<ReadonlyIndex>,
}

pub struct UnpublishedOperation {
    repo_loader: RepoLoader,
    data: Option<NewRepoData>,
    closed: bool,
}

impl UnpublishedOperation {
    fn new(
        repo_loader: RepoLoader,
        operation: Operation,
        view: View,
        index: Arc<ReadonlyIndex>,
    ) -> Self {
        let data = Some(NewRepoData {
            operation,
            view,
            index,
        });
        UnpublishedOperation {
            repo_loader,
            data,
            closed: false,
        }
    }

    pub fn operation(&self) -> &Operation {
        &self.data.as_ref().unwrap().operation
    }

    pub fn publish(mut self) -> Arc<ReadonlyRepo> {
        let data = self.data.take().unwrap();
        self.repo_loader
            .op_heads_store()
            .lock()
            .finish(&data.operation);
        let repo = self
            .repo_loader
            .create_from(data.operation, data.view, data.index);
        self.closed = true;
        repo
    }

    pub fn leave_unpublished(mut self) -> Arc<ReadonlyRepo> {
        let data = self.data.take().unwrap();
        let repo = self
            .repo_loader
            .create_from(data.operation, data.view, data.index);
        self.closed = true;
        repo
    }
}

impl Drop for UnpublishedOperation {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                self.closed,
                "UnpublishedOperation was dropped without being closed."
            );
        }
    }
}
