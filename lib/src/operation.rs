// Copyright 2020 The Jujutsu Authors
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

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt::{Debug, Error, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::backend::CommitId;
use crate::op_store::{OpStore, OperationId, ViewId};
use crate::{dag_walk, op_store};

#[derive(Clone)]
pub struct Operation {
    op_store: Arc<dyn OpStore>,
    id: OperationId,
    data: op_store::Operation,
}

impl Debug for Operation {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("Operation").field("id", &self.id).finish()
    }
}

impl PartialEq for Operation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Operation {}

impl Ord for Operation {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for Operation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.id.cmp(&other.id))
    }
}

impl Hash for Operation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl Operation {
    pub fn new(op_store: Arc<dyn OpStore>, id: OperationId, data: op_store::Operation) -> Self {
        Operation { op_store, id, data }
    }

    pub fn op_store(&self) -> Arc<dyn OpStore> {
        self.op_store.clone()
    }

    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn parent_ids(&self) -> &Vec<OperationId> {
        &self.data.parents
    }

    pub fn parents(&self) -> Vec<Operation> {
        let mut parents = Vec::new();
        for parent_id in &self.data.parents {
            let data = self.op_store.read_operation(parent_id).unwrap();
            parents.push(Operation::new(
                self.op_store.clone(),
                parent_id.clone(),
                data,
            ));
        }
        parents
    }

    pub fn view(&self) -> View {
        let data = self.op_store.read_view(&self.data.view_id).unwrap();
        View::new(self.op_store.clone(), self.data.view_id.clone(), data)
    }

    pub fn store_operation(&self) -> &op_store::Operation {
        &self.data
    }
}

#[derive(Clone)]
pub struct View {
    op_store: Arc<dyn OpStore>,
    id: ViewId,
    data: op_store::View,
}

impl Debug for View {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("View").field("id", &self.id).finish()
    }
}

impl PartialEq for View {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for View {}

impl Ord for View {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for View {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.id.cmp(&other.id))
    }
}

impl Hash for View {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl View {
    pub fn new(op_store: Arc<dyn OpStore>, id: ViewId, data: op_store::View) -> Self {
        View { op_store, id, data }
    }

    pub fn op_store(&self) -> Arc<dyn OpStore> {
        self.op_store.clone()
    }

    pub fn id(&self) -> &ViewId {
        &self.id
    }

    pub fn store_view(&self) -> &op_store::View {
        &self.data
    }

    pub fn take_store_view(self) -> op_store::View {
        self.data
    }

    pub fn heads(&self) -> &HashSet<CommitId> {
        &self.data.head_ids
    }
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

/// Walks `head_op` and its ancestors in reverse topological order.
pub fn walk_ancestors(head_op: &Operation) -> impl Iterator<Item = Operation> {
    // Lazily load operations based on timestamp-based heuristic. This works so long
    // as the operation history is mostly linear.
    dag_walk::topo_order_reverse_lazy(
        vec![OperationByEndTime(head_op.clone())],
        |OperationByEndTime(op)| op.id().clone(),
        |OperationByEndTime(op)| op.parents().into_iter().map(OperationByEndTime),
    )
    .map(|OperationByEndTime(op)| op)
}
