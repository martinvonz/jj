// Copyright 2020-2023 The Jujutsu Authors
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

//! Utility for operation id resolution and traversal.

use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use itertools::Itertools as _;
use thiserror::Error;

use crate::backend::ObjectId as _;
use crate::op_heads_store::{OpHeadResolutionError, OpHeadsStore};
use crate::op_store::{OpStore, OpStoreError, OpStoreResult, OperationId};
use crate::operation::Operation;
use crate::repo::{ReadonlyRepo, Repo as _, RepoLoader};
use crate::{dag_walk, op_heads_store};

/// Error that may occur during evaluation of operation set expression.
#[derive(Debug, Error)]
pub enum OpsetEvaluationError {
    /// Failed to resolve operation set expression.
    #[error(transparent)]
    OpsetResolution(#[from] OpsetResolutionError),
    /// Failed to resolve the current operation heads.
    #[error(transparent)]
    OpHeadResolution(#[from] OpHeadResolutionError),
    /// Failed to access operation object.
    #[error(transparent)]
    OpStore(#[from] OpStoreError),
}

/// Error that may occur during parsing and resolution of operation set
/// expression.
#[derive(Debug, Error)]
pub enum OpsetResolutionError {
    // TODO: Maybe empty/multiple operations should be allowed, and rejected by
    // caller as needed.
    /// Expression resolved to multiple operations.
    #[error(r#"The "{0}" expression resolved to more than one operation"#)]
    MultipleOperations(String),
    /// Expression resolved to no operations.
    #[error(r#"The "{0}" expression resolved to no operations"#)]
    EmptyOperations(String),
    /// Invalid symbol as an operation ID.
    #[error(r#"Operation ID "{0}" is not a valid hexadecimal prefix"#)]
    InvalidIdPrefix(String),
    /// Operation ID not found.
    #[error(r#"No operation ID matching "{0}""#)]
    NoSuchOperation(String),
    /// Operation ID prefix matches multiple operations.
    #[error(r#"Operation ID prefix "{0}" is ambiguous"#)]
    AmbiguousIdPrefix(String),
}

/// Resolves operation set expression without loading a repo.
pub fn resolve_op_for_load(
    repo_loader: &RepoLoader,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    let op_store = repo_loader.op_store();
    let op_heads_store = repo_loader.op_heads_store().as_ref();
    let get_current_op = || {
        op_heads_store::resolve_op_heads(op_heads_store, op_store, |_| {
            Err(OpsetResolutionError::MultipleOperations("@".to_owned()).into())
        })
    };
    resolve_single_op(op_store, op_heads_store, get_current_op, op_str)
}

/// Resolves operation set expression against the loaded repo.
///
/// The "@" symbol will be resolved to the operation the repo was loaded at.
pub fn resolve_op_with_repo(
    repo: &ReadonlyRepo,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    let op_store = repo.op_store();
    let op_heads_store = repo.op_heads_store().as_ref();
    let get_current_op = || Ok(repo.operation().clone());
    resolve_single_op(op_store, op_heads_store, get_current_op, op_str)
}

/// Resolves operation set expression with the given "@" symbol resolution
/// callback.
fn resolve_single_op(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &dyn OpHeadsStore,
    get_current_op: impl FnOnce() -> Result<Operation, OpsetEvaluationError>,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    let op_symbol = op_str.trim_end_matches('-');
    let op_postfix = &op_str[op_symbol.len()..];
    let mut operation = match op_symbol {
        "@" => get_current_op(),
        s => resolve_single_op_from_store(op_store, op_heads_store, s),
    }?;
    for _ in op_postfix.chars() {
        let mut parent_ops = operation.parents();
        let Some(op) = parent_ops.next().transpose()? else {
            return Err(OpsetResolutionError::EmptyOperations(op_str.to_owned()).into());
        };
        if parent_ops.next().is_some() {
            return Err(OpsetResolutionError::MultipleOperations(op_str.to_owned()).into());
        }
        drop(parent_ops);
        operation = op;
    }
    Ok(operation)
}

fn resolve_single_op_from_store(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &dyn OpHeadsStore,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    if op_str.is_empty() || !op_str.as_bytes().iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(OpsetResolutionError::InvalidIdPrefix(op_str.to_owned()).into());
    }
    if let Ok(binary_op_id) = hex::decode(op_str) {
        let op_id = OperationId::new(binary_op_id);
        match op_store.read_operation(&op_id) {
            Ok(operation) => {
                return Ok(Operation::new(op_store.clone(), op_id, operation));
            }
            Err(OpStoreError::ObjectNotFound { .. }) => {
                // Fall through
            }
            Err(err) => {
                return Err(OpsetEvaluationError::OpStore(err));
            }
        }
    }
    let mut matches = vec![];
    for op in find_all_operations(op_store, op_heads_store)? {
        if op.id().hex().starts_with(op_str) {
            matches.push(op);
        }
    }
    if matches.is_empty() {
        Err(OpsetResolutionError::NoSuchOperation(op_str.to_owned()).into())
    } else if matches.len() == 1 {
        Ok(matches.pop().unwrap())
    } else {
        Err(OpsetResolutionError::AmbiguousIdPrefix(op_str.to_owned()).into())
    }
}

fn find_all_operations(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &dyn OpHeadsStore,
) -> Result<Vec<Operation>, OpStoreError> {
    let mut visited = HashSet::new();
    let mut work: VecDeque<_> = op_heads_store.get_op_heads().into_iter().collect();
    let mut operations = vec![];
    while let Some(op_id) = work.pop_front() {
        if visited.insert(op_id.clone()) {
            let store_operation = op_store.read_operation(&op_id)?;
            work.extend(store_operation.parents.iter().cloned());
            let operation = Operation::new(op_store.clone(), op_id, store_operation);
            operations.push(operation);
        }
    }
    Ok(operations)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct OperationByEndTime(Operation);

impl Ord for OperationByEndTime {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_end_time = &self.0.store_operation().metadata.end_time;
        let other_end_time = &other.0.store_operation().metadata.end_time;
        self_end_time
            .cmp(other_end_time)
            .then_with(|| self.0.cmp(&other.0)) // to comply with Eq
    }
}

impl PartialOrd for OperationByEndTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Walks `head_ops` and their ancestors in reverse topological order.
pub fn walk_ancestors(head_ops: &[Operation]) -> impl Iterator<Item = OpStoreResult<Operation>> {
    // Lazily load operations based on timestamp-based heuristic. This works so long
    // as the operation history is mostly linear.
    dag_walk::topo_order_reverse_lazy_ok(
        head_ops
            .iter()
            .map(|op| Ok(OperationByEndTime(op.clone())))
            .collect_vec(),
        |OperationByEndTime(op)| op.id().clone(),
        |OperationByEndTime(op)| op.parents().map_ok(OperationByEndTime).collect_vec(),
    )
    .map_ok(|OperationByEndTime(op)| op)
}
