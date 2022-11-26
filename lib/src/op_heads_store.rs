// Copyright 2021 The Jujutsu Authors
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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use itertools::Itertools;
use thiserror::Error;

use crate::lock::FileLock;
use crate::op_store::{OpStore, OperationId, OperationMetadata};
use crate::operation::Operation;
use crate::{dag_walk, op_store};

/// Manages the very set of current heads of the operation log. The store is
/// simply a directory where each operation id is a file with that name (and no
/// content).
pub struct OpHeadsStore {
    dir: PathBuf,
}

pub enum OpHeads {
    /// There's a single latest operation. This is the normal case.
    Single(Operation),
    /// There are multiple latest operations, which means there has been
    /// concurrent operations. These need to be resolved.
    Unresolved {
        locked_op_heads: LockedOpHeads,
        op_heads: Vec<Operation>,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpHeadResolutionError {
    #[error("Operation log has no heads")]
    NoHeads,
}

pub struct LockedOpHeads {
    store: Arc<OpHeadsStore>,
    _lock: FileLock,
}

impl LockedOpHeads {
    pub fn finish(self, new_op: &Operation) {
        self.store.add_op_head(new_op.id());
        for old_id in new_op.parent_ids() {
            self.store.remove_op_head(old_id);
        }
    }
}

impl OpHeadsStore {
    pub fn init(
        dir: PathBuf,
        op_store: &Arc<dyn OpStore>,
        root_view: &op_store::View,
        operation_metadata: OperationMetadata,
    ) -> (Self, Operation) {
        let root_view_id = op_store.write_view(root_view).unwrap();
        let init_operation = op_store::Operation {
            view_id: root_view_id,
            parents: vec![],
            metadata: operation_metadata,
        };
        let init_operation_id = op_store.write_operation(&init_operation).unwrap();
        let init_operation = Operation::new(op_store.clone(), init_operation_id, init_operation);

        let op_heads_store = OpHeadsStore { dir };
        op_heads_store.add_op_head(init_operation.id());
        (op_heads_store, init_operation)
    }

    pub fn load(dir: PathBuf) -> OpHeadsStore {
        OpHeadsStore { dir }
    }

    fn add_op_head(&self, id: &OperationId) {
        std::fs::write(self.dir.join(id.hex()), "").unwrap();
    }

    fn remove_op_head(&self, id: &OperationId) {
        // It's fine if the old head was not found. It probably means
        // that we're on a distributed file system where the locking
        // doesn't work. We'll probably end up with two current
        // heads. We'll detect that next time we load the view.
        std::fs::remove_file(self.dir.join(id.hex())).ok();
    }

    pub fn get_op_heads(&self) -> Vec<OperationId> {
        let mut op_heads = vec![];
        for op_head_entry in std::fs::read_dir(&self.dir).unwrap() {
            let op_head_file_name = op_head_entry.unwrap().file_name();
            let op_head_file_name = op_head_file_name.to_str().unwrap();
            if let Ok(op_head) = hex::decode(op_head_file_name) {
                op_heads.push(OperationId::new(op_head));
            }
        }
        op_heads
    }

    pub fn lock(self: &Arc<Self>) -> LockedOpHeads {
        let lock = FileLock::lock(self.dir.join("lock"));
        LockedOpHeads {
            store: self.clone(),
            _lock: lock,
        }
    }

    pub fn get_heads(
        self: &Arc<Self>,
        op_store: &Arc<dyn OpStore>,
    ) -> Result<OpHeads, OpHeadResolutionError> {
        let mut op_heads = self.get_op_heads();

        if op_heads.is_empty() {
            return Err(OpHeadResolutionError::NoHeads);
        }

        if op_heads.len() == 1 {
            let operation_id = op_heads.pop().unwrap();
            let operation = op_store.read_operation(&operation_id).unwrap();
            return Ok(OpHeads::Single(Operation::new(
                op_store.clone(),
                operation_id,
                operation,
            )));
        }

        // There are multiple heads. We take a lock, then check if there are still
        // multiple heads (it's likely that another process was in the process of
        // deleting on of them). If there are still multiple heads, we attempt to
        // merge all the views into one. We then write that view and a corresponding
        // operation to the op-store.
        // Note that the locking isn't necessary for correctness; we take the lock
        // only to prevent other concurrent processes from doing the same work (and
        // producing another set of divergent heads).
        let locked_op_heads = self.lock();
        let op_head_ids = self.get_op_heads();

        if op_head_ids.is_empty() {
            return Err(OpHeadResolutionError::NoHeads);
        }

        if op_head_ids.len() == 1 {
            let op_head_id = op_head_ids[0].clone();
            let op_head = op_store.read_operation(&op_head_id).unwrap();
            // Return early so we don't write a merge operation with a single parent
            return Ok(OpHeads::Single(Operation::new(
                op_store.clone(),
                op_head_id,
                op_head,
            )));
        }

        let op_heads = op_head_ids
            .iter()
            .map(|op_id: &OperationId| {
                let data = op_store.read_operation(op_id).unwrap();
                Operation::new(op_store.clone(), op_id.clone(), data)
            })
            .collect_vec();
        let mut op_heads = self.handle_ancestor_ops(op_heads);

        // Return without creating a merge operation
        if op_heads.len() == 1 {
            return Ok(OpHeads::Single(op_heads.pop().unwrap()));
        }

        op_heads.sort_by_key(|op| op.store_operation().metadata.end_time.timestamp.clone());
        Ok(OpHeads::Unresolved {
            locked_op_heads,
            op_heads,
        })
    }

    /// Removes operations in the input that are ancestors of other operations
    /// in the input. The ancestors are removed both from the list and from
    /// disk.
    fn handle_ancestor_ops(&self, op_heads: Vec<Operation>) -> Vec<Operation> {
        let op_head_ids_before: HashSet<_> = op_heads.iter().map(|op| op.id().clone()).collect();
        let neighbors_fn = |op: &Operation| op.parents();
        // Remove ancestors so we don't create merge operation with an operation and its
        // ancestor
        let op_heads = dag_walk::heads(op_heads, &neighbors_fn, &|op: &Operation| op.id().clone());
        let op_head_ids_after: HashSet<_> = op_heads.iter().map(|op| op.id().clone()).collect();
        for removed_op_head in op_head_ids_before.difference(&op_head_ids_after) {
            self.remove_op_head(removed_op_head);
        }
        op_heads.into_iter().collect()
    }
}
