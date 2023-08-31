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

#![allow(missing_docs)]

use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;

use itertools::Itertools;
use thiserror::Error;

use crate::dag_walk;
use crate::op_store::{OpStore, OpStoreError, OperationId};
use crate::operation::Operation;

#[derive(Debug, Error)]
pub enum OpHeadResolutionError<E> {
    #[error("Operation log has no heads")]
    NoHeads,
    #[error(transparent)]
    OpStore(#[from] OpStoreError),
    #[error("Op resolution error: {0}")]
    Err(#[source] E),
}

pub trait OpHeadsStoreLock<'a> {
    fn promote_new_op(&self, new_op: &Operation);
}

/// Manages the set of current heads of the operation log.
pub trait OpHeadsStore: Send + Sync + Debug {
    fn name(&self) -> &str;

    fn add_op_head(&self, id: &OperationId);

    fn remove_op_head(&self, id: &OperationId);

    fn get_op_heads(&self) -> Vec<OperationId>;

    fn lock<'a>(&'a self) -> Box<dyn OpHeadsStoreLock<'a> + 'a>;

    /// Removes operations in the input that are ancestors of other operations
    /// in the input. The ancestors are removed both from the list and from
    /// storage.
    fn handle_ancestor_ops(&self, op_heads: Vec<Operation>) -> Vec<Operation> {
        let op_head_ids_before: HashSet<_> = op_heads.iter().map(|op| op.id().clone()).collect();
        // Remove ancestors so we don't create merge operation with an operation and its
        // ancestor
        let op_heads = dag_walk::heads(
            op_heads,
            |op: &Operation| op.id().clone(),
            |op: &Operation| op.parents(),
        );
        let op_head_ids_after: HashSet<_> = op_heads.iter().map(|op| op.id().clone()).collect();
        for removed_op_head in op_head_ids_before.difference(&op_head_ids_after) {
            self.remove_op_head(removed_op_head);
        }
        op_heads.into_iter().collect()
    }
}

// Given an OpHeadsStore, fetch and resolve its op heads down to one under a
// lock.
//
// This routine is defined outside the trait because it must support generics.
pub fn resolve_op_heads<E>(
    op_heads_store: &dyn OpHeadsStore,
    op_store: &Arc<dyn OpStore>,
    resolver: impl FnOnce(Vec<Operation>) -> Result<Operation, E>,
) -> Result<Operation, OpHeadResolutionError<E>> {
    let mut op_heads = op_heads_store.get_op_heads();

    // TODO: De-duplicate this 'simple-resolution' code.
    if op_heads.is_empty() {
        return Err(OpHeadResolutionError::NoHeads);
    }

    if op_heads.len() == 1 {
        let operation_id = op_heads.pop().unwrap();
        let operation = op_store.read_operation(&operation_id)?;
        return Ok(Operation::new(op_store.clone(), operation_id, operation));
    }

    // There are multiple heads. We take a lock, then check if there are still
    // multiple heads (it's likely that another process was in the process of
    // deleting on of them). If there are still multiple heads, we attempt to
    // merge all the views into one. We then write that view and a corresponding
    // operation to the op-store.
    // Note that the locking isn't necessary for correctness; we take the lock
    // only to prevent other concurrent processes from doing the same work (and
    // producing another set of divergent heads).
    let lock = op_heads_store.lock();
    let op_head_ids = op_heads_store.get_op_heads();

    if op_head_ids.is_empty() {
        return Err(OpHeadResolutionError::NoHeads);
    }

    if op_head_ids.len() == 1 {
        let op_head_id = op_head_ids[0].clone();
        let op_head = op_store.read_operation(&op_head_id)?;
        return Ok(Operation::new(op_store.clone(), op_head_id, op_head));
    }

    let op_heads = op_head_ids
        .iter()
        .map(|op_id: &OperationId| -> Result<Operation, OpStoreError> {
            let data = op_store.read_operation(op_id)?;
            Ok(Operation::new(op_store.clone(), op_id.clone(), data))
        })
        .try_collect()?;
    let mut op_heads = op_heads_store.handle_ancestor_ops(op_heads);

    // Return without creating a merge operation
    if op_heads.len() == 1 {
        return Ok(op_heads.pop().unwrap());
    }

    op_heads.sort_by_key(|op| op.store_operation().metadata.end_time.timestamp.clone());
    match resolver(op_heads) {
        Ok(new_op) => {
            lock.promote_new_op(&new_op);
            Ok(new_op)
        }
        Err(e) => Err(OpHeadResolutionError::Err(e)),
    }
}
