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

use itertools::Itertools as _;

use crate::dag_walk;
use crate::op_store::OpStoreResult;
use crate::operation::Operation;

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
pub fn walk_ancestors(head_op: &Operation) -> impl Iterator<Item = OpStoreResult<Operation>> {
    // Lazily load operations based on timestamp-based heuristic. This works so long
    // as the operation history is mostly linear.
    dag_walk::topo_order_reverse_lazy_ok(
        [Ok(OperationByEndTime(head_op.clone()))],
        |OperationByEndTime(op)| op.id().clone(),
        |OperationByEndTime(op)| op.parents().map_ok(OperationByEndTime).collect_vec(),
    )
    .map_ok(|OperationByEndTime(op)| op)
}
