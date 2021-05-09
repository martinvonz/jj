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
use std::sync::{Arc, Mutex};

use crate::evolution::ReadonlyEvolution;
use crate::index::ReadonlyIndex;
use crate::op_store;
use crate::op_store::{OperationId, OperationMetadata};
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo, RepoLoader};
use crate::store::Timestamp;
use crate::view::ReadonlyView;
use crate::working_copy::WorkingCopy;

pub struct Transaction {
    repo: Option<Arc<MutableRepo>>,
    parents: Vec<OperationId>,
    description: String,
    start_time: Timestamp,
    tags: HashMap<String, String>,
    closed: bool,
}

impl Transaction {
    pub fn new(mut_repo: Arc<MutableRepo>, description: &str) -> Transaction {
        let parents = vec![mut_repo.base_repo().op_id().clone()];
        Transaction {
            repo: Some(mut_repo),
            parents,
            description: description.to_owned(),
            start_time: Timestamp::now(),
            tags: Default::default(),
            closed: false,
        }
    }

    pub fn base_repo(&self) -> &ReadonlyRepo {
        self.repo.as_ref().unwrap().base_repo()
    }

    pub fn set_parents(&mut self, parents: Vec<OperationId>) {
        self.parents = parents;
    }

    pub fn set_tag(&mut self, key: String, value: String) {
        self.tags.insert(key, value);
    }

    pub fn mut_repo(&mut self) -> &mut MutableRepo {
        Arc::get_mut(self.repo.as_mut().unwrap()).unwrap()
    }

    /// Writes the transaction to the operation store and publishes it.
    pub fn commit(self) -> Arc<ReadonlyRepo> {
        self.write().publish()
    }

    /// Writes the transaction to the operation store, but does not publish it.
    /// That means that a repo can be loaded at the operation, but the
    /// operation will not be seen when loading the repo at head.
    pub fn write(mut self) -> UnpublishedOperation {
        let mut_repo = Arc::try_unwrap(self.repo.take().unwrap()).ok().unwrap();
        let base_repo = mut_repo.base_repo().clone();
        let (mut_index, mut_view, maybe_mut_evolution) = mut_repo.consume();
        let maybe_evolution =
            maybe_mut_evolution.map(|mut_evolution| Arc::new(mut_evolution.freeze()));
        let index = base_repo.index_store().write_index(mut_index).unwrap();

        let view = mut_view.freeze();
        let view_id = base_repo.op_store().write_view(view.store_view()).unwrap();
        let mut operation_metadata =
            OperationMetadata::new(self.description.clone(), self.start_time.clone());
        operation_metadata.tags = self.tags.clone();
        let store_operation = op_store::Operation {
            view_id,
            parents: self.parents.clone(),
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
        self.closed = true;
        UnpublishedOperation::new(
            base_repo.loader(),
            operation,
            view,
            base_repo.working_copy().clone(),
            index,
            maybe_evolution,
        )
    }

    pub fn discard(mut self) {
        self.closed = true;
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            debug_assert!(self.closed, "Transaction was dropped without being closed.");
        }
    }
}

struct NewRepoData {
    operation: Operation,
    view: ReadonlyView,
    working_copy: Arc<Mutex<WorkingCopy>>,
    index: Arc<ReadonlyIndex>,
    evolution: Option<Arc<ReadonlyEvolution>>,
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
        view: ReadonlyView,
        working_copy: Arc<Mutex<WorkingCopy>>,
        index: Arc<ReadonlyIndex>,
        evolution: Option<Arc<ReadonlyEvolution>>,
    ) -> Self {
        let data = Some(NewRepoData {
            operation,
            view,
            working_copy,
            index,
            evolution,
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
            .update_op_heads(&data.operation);
        let repo = self.repo_loader.create_from(
            data.operation,
            data.view,
            data.working_copy,
            data.index,
            data.evolution,
        );
        self.closed = true;
        repo
    }

    pub fn leave_unpublished(mut self) -> Arc<ReadonlyRepo> {
        let data = self.data.take().unwrap();
        let repo = self.repo_loader.create_from(
            data.operation,
            data.view,
            data.working_copy,
            data.index,
            data.evolution,
        );
        self.closed = true;
        repo
    }
}

impl Drop for UnpublishedOperation {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            debug_assert!(
                self.closed,
                "UnpublishedOperation was dropped without being closed."
            );
        }
    }
}
