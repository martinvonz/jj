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

use std::sync::Arc;

use crate::op_heads_store::OpHeadsStore;
use crate::op_store;
use crate::op_store::{OperationId, OperationMetadata};
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo};
use crate::store::Timestamp;

pub struct Transaction<'r> {
    repo: Option<Arc<MutableRepo<'r>>>,
    parents: Vec<OperationId>,
    description: String,
    start_time: Timestamp,
    closed: bool,
}

impl<'r> Transaction<'r> {
    pub fn new(mut_repo: Arc<MutableRepo<'r>>, description: &str) -> Transaction<'r> {
        let parents = vec![mut_repo.base_repo().op_id().clone()];
        Transaction {
            repo: Some(mut_repo),
            parents,
            description: description.to_owned(),
            start_time: Timestamp::now(),
            closed: false,
        }
    }

    pub fn base_repo(&self) -> &'r ReadonlyRepo {
        self.repo.as_ref().unwrap().base_repo()
    }

    pub fn set_parents(&mut self, parents: Vec<OperationId>) {
        self.parents = parents;
    }

    pub fn mut_repo(&mut self) -> &mut MutableRepo<'r> {
        Arc::get_mut(self.repo.as_mut().unwrap()).unwrap()
    }

    /// Writes the transaction to the operation store and publishes it.
    pub fn commit(self) -> Operation {
        self.write().publish()
    }

    /// Writes the transaction to the operation store, but does not publish it.
    /// That means that a repo can be loaded at the operation, but the
    /// operation will not be seen when loading the repo at head.
    pub fn write(mut self) -> UnpublishedOperation {
        let mut_repo = Arc::try_unwrap(self.repo.take().unwrap()).ok().unwrap();
        let base_repo = mut_repo.base_repo();
        let (mut_index, mut_view) = mut_repo.consume();
        let index = base_repo.index_store().write_index(mut_index).unwrap();

        let view_id = base_repo
            .op_store()
            .write_view(mut_view.store_view())
            .unwrap();
        let operation_metadata =
            OperationMetadata::new(self.description.clone(), self.start_time.clone());
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
        UnpublishedOperation::new(base_repo.op_heads_store().clone(), operation)
    }

    pub fn discard(mut self) {
        self.closed = true;
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            debug_assert!(self.closed, "Transaction was dropped without being closed.");
        }
    }
}

pub struct UnpublishedOperation {
    op_heads_store: Arc<OpHeadsStore>,
    operation: Option<Operation>,
    closed: bool,
}

impl UnpublishedOperation {
    fn new(op_heads_store: Arc<OpHeadsStore>, operation: Operation) -> Self {
        UnpublishedOperation {
            op_heads_store,
            operation: Some(operation),
            closed: false,
        }
    }

    pub fn operation(&self) -> &Operation {
        self.operation.as_ref().unwrap()
    }

    pub fn publish(mut self) -> Operation {
        let operation = self.operation.take().unwrap();
        self.op_heads_store.update_op_heads(&operation);
        self.closed = true;
        operation
    }

    pub fn leave_unpublished(mut self) -> Operation {
        self.closed = true;
        self.operation.take().unwrap()
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
