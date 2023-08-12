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

use std::any::Any;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;

use itertools::Itertools;
use thiserror::Error;

use crate::dag_walk;
use crate::op_store::{OpStore, OpStoreError, OperationId};
use crate::operation::Operation;

#[derive(Debug, Error)]
pub enum OpHeadResolutionError {
    #[error("Operation log has no heads")]
    NoHeads,
}

pub trait OpHeadsStoreLock {}

/// Manages the set of current heads of the operation log.
pub trait OpHeadsStore: Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;

    fn name(&self) -> &str;

    /// Remove the old op heads and add the new one.
    ///
    /// The old op heads must not contain the new one.
    fn update_op_heads(&self, old_ids: &[OperationId], new_id: &OperationId);

    fn get_op_heads(&self) -> Vec<OperationId>;

    /// Optionally takes a lock on the op heads store. The purpose of the lock
    /// is to prevent concurrent processes from resolving the same divergent
    /// operations. It is not needed for correctness; implementations are free
    /// to return a type that doesn't hold.
    fn lock(&self) -> Box<dyn OpHeadsStoreLock + '_>;
}

// Given an OpHeadsStore, fetch and resolve its op heads down to one under a
// lock.
//
// This routine is defined outside the trait because it must support generics.
pub fn resolve_op_heads<E>(
    op_heads_store: &dyn OpHeadsStore,
    op_store: &Arc<dyn OpStore>,
    resolver: impl FnOnce(Vec<Operation>) -> Result<Operation, E>,
) -> Result<Operation, E>
where
    E: From<OpHeadResolutionError> + From<OpStoreError>,
{
    let mut op_heads = op_heads_store.get_op_heads();

    // TODO: De-duplicate this 'simple-resolution' code.
    if op_heads.is_empty() {
        return Err(OpHeadResolutionError::NoHeads.into());
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
    let _lock = op_heads_store.lock();
    let op_head_ids = op_heads_store.get_op_heads();

    if op_head_ids.is_empty() {
        return Err(OpHeadResolutionError::NoHeads.into());
    }

    if op_head_ids.len() == 1 {
        let op_head_id = op_head_ids[0].clone();
        let op_head = op_store.read_operation(&op_head_id)?;
        return Ok(Operation::new(op_store.clone(), op_head_id, op_head));
    }

    let op_heads: Vec<_> = op_head_ids
        .iter()
        .map(|op_id: &OperationId| -> Result<Operation, OpStoreError> {
            let data = op_store.read_operation(op_id)?;
            Ok(Operation::new(op_store.clone(), op_id.clone(), data))
        })
        .try_collect()?;
    // Remove ancestors so we don't create merge operation with an operation and its
    // ancestor
    let op_head_ids_before: HashSet<_> = op_heads.iter().map(|op| op.id().clone()).collect();
    let filtered_op_heads = dag_walk::heads_ok(
        op_heads.into_iter().map(Ok),
        |op: &Operation| op.id().clone(),
        |op: &Operation| op.parents().collect_vec(),
    )?;
    let op_head_ids_after: HashSet<_> =
        filtered_op_heads.iter().map(|op| op.id().clone()).collect();
    let ancestor_op_heads = op_head_ids_before
        .difference(&op_head_ids_after)
        .cloned()
        .collect_vec();
    let mut op_heads = filtered_op_heads.into_iter().collect_vec();

    // Return without creating a merge operation
    if let [op_head] = &*op_heads {
        op_heads_store.update_op_heads(&ancestor_op_heads, op_head.id());
        return Ok(op_head.clone());
    }

    op_heads.sort_by_key(|op| op.metadata().end_time.timestamp);
    let new_op = resolver(op_heads)?;
    let mut old_op_heads = ancestor_op_heads;
    old_op_heads.extend_from_slice(new_op.parent_ids());
    op_heads_store.update_op_heads(&old_op_heads, new_op.id());
    Ok(new_op)
}
