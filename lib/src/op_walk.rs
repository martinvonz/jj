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
use std::collections::{HashMap, HashSet};
use std::slice;
use std::sync::Arc;

use itertools::Itertools as _;
use thiserror::Error;

use crate::object_id::{HexPrefix, PrefixResolution};
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
    let get_head_ops = || get_current_head_ops(op_store, op_heads_store);
    resolve_single_op(op_store, get_current_op, get_head_ops, op_str)
}

/// Resolves operation set expression against the loaded repo.
///
/// The "@" symbol will be resolved to the operation the repo was loaded at.
pub fn resolve_op_with_repo(
    repo: &ReadonlyRepo,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    resolve_op_at(repo.op_store(), repo.operation(), op_str)
}

/// Resolves operation set expression at the given head operation.
pub fn resolve_op_at(
    op_store: &Arc<dyn OpStore>,
    head_op: &Operation,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    let get_current_op = || Ok(head_op.clone());
    let get_head_ops = || Ok(vec![head_op.clone()]);
    resolve_single_op(op_store, get_current_op, get_head_ops, op_str)
}

/// Resolves operation set expression with the given "@" symbol resolution
/// callbacks.
fn resolve_single_op(
    op_store: &Arc<dyn OpStore>,
    get_current_op: impl FnOnce() -> Result<Operation, OpsetEvaluationError>,
    get_head_ops: impl FnOnce() -> OpStoreResult<Vec<Operation>>,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    let op_symbol = op_str.trim_end_matches(['-', '+']);
    let op_postfix = &op_str[op_symbol.len()..];
    let head_ops = op_postfix.contains('+').then(get_head_ops).transpose()?;
    let mut operation = match op_symbol {
        "@" => get_current_op(),
        s => resolve_single_op_from_store(op_store, s),
    }?;
    for c in op_postfix.chars() {
        let mut neighbor_ops = match c {
            '-' => operation.parents().try_collect()?,
            '+' => find_child_ops(head_ops.as_ref().unwrap(), operation.id())?,
            _ => unreachable!(),
        };
        operation = match neighbor_ops.len() {
            0 => Err(OpsetResolutionError::EmptyOperations(op_str.to_owned()))?,
            1 => neighbor_ops.pop().unwrap(),
            _ => Err(OpsetResolutionError::MultipleOperations(op_str.to_owned()))?,
        };
    }
    Ok(operation)
}

fn resolve_single_op_from_store(
    op_store: &Arc<dyn OpStore>,
    op_str: &str,
) -> Result<Operation, OpsetEvaluationError> {
    if op_str.is_empty() {
        return Err(OpsetResolutionError::InvalidIdPrefix(op_str.to_owned()).into());
    }
    let prefix = HexPrefix::new(op_str)
        .ok_or_else(|| OpsetResolutionError::InvalidIdPrefix(op_str.to_owned()))?;
    match op_store.resolve_operation_id_prefix(&prefix)? {
        PrefixResolution::NoMatch => {
            Err(OpsetResolutionError::NoSuchOperation(op_str.to_owned()).into())
        }
        PrefixResolution::SingleMatch(op_id) => {
            let data = op_store.read_operation(&op_id)?;
            Ok(Operation::new(op_store.clone(), op_id, data))
        }
        PrefixResolution::AmbiguousMatch => {
            Err(OpsetResolutionError::AmbiguousIdPrefix(op_str.to_owned()).into())
        }
    }
}

/// Loads the current head operations. The returned operations may contain
/// redundant ones which are ancestors of the other heads.
pub fn get_current_head_ops(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &dyn OpHeadsStore,
) -> OpStoreResult<Vec<Operation>> {
    op_heads_store
        .get_op_heads()
        .into_iter()
        .map(|id| -> OpStoreResult<Operation> {
            let data = op_store.read_operation(&id)?;
            Ok(Operation::new(op_store.clone(), id, data))
        })
        .try_collect()
}

/// Looks up children of the `root_op_id` by traversing from the `head_ops`.
///
/// This will be slow if the `root_op_id` is far away (or unreachable) from the
/// `head_ops`.
fn find_child_ops(
    head_ops: &[Operation],
    root_op_id: &OperationId,
) -> OpStoreResult<Vec<Operation>> {
    walk_ancestors(head_ops)
        .take_while(|res| res.as_ref().map_or(true, |op| op.id() != root_op_id))
        .filter_ok(|op| op.parent_ids().iter().any(|id| id == root_op_id))
        .try_collect()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct OperationByEndTime(Operation);

impl Ord for OperationByEndTime {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_end_time = &self.0.metadata().end_time;
        let other_end_time = &other.0.metadata().end_time;
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
    // Emit the latest head first to stabilize the order.
    let mut head_ops = head_ops
        .iter()
        .cloned()
        .map(OperationByEndTime)
        .collect_vec();
    head_ops.sort_unstable_by(|op1, op2| op1.cmp(op2).reverse());
    // Lazily load operations based on timestamp-based heuristic. This works so long
    // as the operation history is mostly linear.
    dag_walk::topo_order_reverse_lazy_ok(
        head_ops.into_iter().map(Ok),
        |OperationByEndTime(op)| op.id().clone(),
        |OperationByEndTime(op)| op.parents().map_ok(OperationByEndTime).collect_vec(),
    )
    .map_ok(|OperationByEndTime(op)| op)
}

/// Stats about `reparent_range()`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReparentStats {
    /// New head operation ids.
    pub new_head_ids: Vec<OperationId>,
    /// The number of rewritten operations.
    pub rewritten_count: usize,
    /// The number of ancestor operations that become unreachable from the
    /// rewritten heads.
    pub unreachable_count: usize,
}

/// Reparents the operation range `root_ops..head_ops` onto the `dest_op`.
///
/// Returns the new head operation ids as well as some stats. If the old
/// operation heads are remapped to the new heads, the operations within the
/// range `dest_op..root_ops` become unreachable.
///
/// If the source operation range `root_ops..head_ops` was empty, the
/// `new_head_ids` will be `[dest_op.id()]`, meaning the `dest_op` is the head.
// TODO: Find better place to host this function. It might be an OpStore method.
pub fn reparent_range(
    op_store: &dyn OpStore,
    root_ops: &[Operation],
    head_ops: &[Operation],
    dest_op: &Operation,
) -> OpStoreResult<ReparentStats> {
    // Calculate ::root_ops to exclude them from the source range and count the
    // number of operations that become unreachable.
    let mut unwanted_ids: HashSet<_> = walk_ancestors(root_ops)
        .map_ok(|op| op.id().clone())
        .try_collect()?;
    let ops_to_reparent: Vec<_> = walk_ancestors(head_ops)
        .filter_ok(|op| !unwanted_ids.contains(op.id()))
        .try_collect()?;
    for op in walk_ancestors(slice::from_ref(dest_op)) {
        unwanted_ids.remove(op?.id());
    }
    let unreachable_ids = unwanted_ids;

    assert!(
        ops_to_reparent
            .last()
            .map_or(true, |op| op.id() != op_store.root_operation_id()),
        "root operation cannot be rewritten"
    );
    let mut rewritten_ids = HashMap::new();
    for old_op in ops_to_reparent.into_iter().rev() {
        let mut data = old_op.store_operation().clone();
        let mut dest_once = Some(dest_op.id());
        data.parents = data
            .parents
            .iter()
            .filter_map(|id| rewritten_ids.get(id).or_else(|| dest_once.take()))
            .cloned()
            .collect();
        let new_id = op_store.write_operation(&data)?;
        rewritten_ids.insert(old_op.id().clone(), new_id);
    }

    let mut dest_once = Some(dest_op.id());
    let new_head_ids = head_ops
        .iter()
        .filter_map(|op| rewritten_ids.get(op.id()).or_else(|| dest_once.take()))
        .cloned()
        .collect();
    Ok(ReparentStats {
        new_head_ids,
        rewritten_count: rewritten_ids.len(),
        unreachable_count: unreachable_ids.len(),
    })
}
