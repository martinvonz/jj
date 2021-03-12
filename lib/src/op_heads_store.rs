// Copyright 2021 Google LLC
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

use crate::dag_walk;
use crate::index::MutableIndex;
use crate::index_store::IndexStore;
use crate::lock::FileLock;
use crate::op_store;
use crate::op_store::{OpStore, OperationId, OperationMetadata};
use crate::operation::Operation;
use crate::store_wrapper::StoreWrapper;
use crate::view;
use std::path::PathBuf;
use std::sync::Arc;

use crate::store::CommitId;
use thiserror::Error;

/// Manages the very set of current heads of the operation log. The store is
/// simply a directory where each operation id is a file with that name (and no
/// content).
pub struct OpHeadsStore {
    dir: PathBuf,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpHeadResolutionError {
    #[error("Operation log has no heads")]
    NoHeads,
}

impl OpHeadsStore {
    pub fn init(
        dir: PathBuf,
        op_store: &Arc<dyn OpStore>,
        checkout: CommitId,
    ) -> (Self, OperationId, op_store::View) {
        let mut root_view = op_store::View::new(checkout.clone());
        root_view.head_ids.insert(checkout);
        let root_view_id = op_store.write_view(&root_view).unwrap();
        let operation_metadata = OperationMetadata::new("initialize repo".to_string());
        let init_operation = op_store::Operation {
            view_id: root_view_id,
            parents: vec![],
            metadata: operation_metadata,
        };
        let init_operation_id = op_store.write_operation(&init_operation).unwrap();

        let op_heads_store = OpHeadsStore { dir };
        op_heads_store.add_op_head(&init_operation_id);
        (op_heads_store, init_operation_id, root_view)
    }

    pub fn load(dir: PathBuf) -> OpHeadsStore {
        OpHeadsStore { dir }
    }

    pub fn add_op_head(&self, id: &OperationId) {
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
                op_heads.push(OperationId(op_head));
            }
        }
        op_heads
    }

    fn lock(&self) -> FileLock {
        FileLock::lock(self.dir.join("lock"))
    }

    pub fn update_op_heads(&self, op: &Operation) {
        let _op_heads_lock = self.lock();
        self.add_op_head(op.id());
        for old_parent_id in op.parent_ids() {
            self.remove_op_head(old_parent_id);
        }
    }

    // TODO: Introduce context objects (like commit::Commit) so we won't have to
    // pass around OperationId and Operation separately like we do here.
    pub fn get_single_op_head(
        &self,
        store: &StoreWrapper,
        op_store: &Arc<dyn OpStore>,
        index_store: &Arc<IndexStore>,
    ) -> Result<(OperationId, op_store::Operation, op_store::View), OpHeadResolutionError> {
        let mut op_heads = self.get_op_heads();

        if op_heads.is_empty() {
            return Err(OpHeadResolutionError::NoHeads);
        }

        if op_heads.len() == 1 {
            let operation_id = op_heads.pop().unwrap();
            let operation = op_store.read_operation(&operation_id).unwrap();
            let view = op_store.read_view(&operation.view_id).unwrap();
            return Ok((operation_id, operation, view));
        }

        // There are multiple heads. We take a lock, then check if there are still
        // multiple heads (it's likely that another process was in the process of
        // deleting on of them). If there are still multiple heads, we attempt to
        // merge all the views into one. We then write that view and a corresponding
        // operation to the op-store.
        // Note that the locking isn't necessary for correctness; we take the lock
        // only to avoid other concurrent processes from doing the same work (and
        // producing another set of divergent heads).
        let _lock = self.lock();
        let op_head_ids = self.get_op_heads();

        if op_head_ids.is_empty() {
            return Err(OpHeadResolutionError::NoHeads);
        }

        if op_head_ids.len() == 1 {
            let op_head_id = op_head_ids[0].clone();
            let op_head = op_store.read_operation(&op_head_id).unwrap();
            // Return early so we don't write a merge operation with a single parent
            let view = op_store.read_view(&op_head.view_id).unwrap();
            return Ok((op_head_id, op_head, view));
        }

        let op_heads: Vec<_> = op_head_ids
            .iter()
            .map(|op_id: &OperationId| {
                let data = op_store.read_operation(op_id).unwrap();
                Operation::new(op_store.clone(), op_id.clone(), data)
            })
            .collect();
        let neighbors_fn = |op: &Operation| op.parents();
        // Remove ancestors so we don't create merge operation with an operation and its
        // ancestor
        let op_heads =
            dag_walk::unreachable(op_heads, &neighbors_fn, &|op: &Operation| op.id().clone());
        let op_heads: Vec<_> = op_heads.into_iter().collect();

        let (merge_operation_id, merge_operation, merged_view) =
            merge_op_heads(store, op_store, index_store, op_heads)?;
        self.add_op_head(&merge_operation_id);
        for old_op_head_id in op_head_ids {
            // The merged one will be in the input to the merge if it's a "fast-forward"
            // merge.
            if old_op_head_id != merge_operation_id {
                self.remove_op_head(&old_op_head_id);
            }
        }
        Ok((merge_operation_id, merge_operation, merged_view))
    }
}

fn merge_op_heads(
    store: &StoreWrapper,
    op_store: &Arc<dyn OpStore>,
    index_store: &Arc<IndexStore>,
    mut op_heads: Vec<Operation>,
) -> Result<(OperationId, op_store::Operation, op_store::View), OpHeadResolutionError> {
    op_heads.sort_by_key(|op| op.store_operation().metadata.end_time.timestamp.clone());
    let first_op_head = op_heads[0].clone();
    let mut merged_view = op_store.read_view(first_op_head.view().id()).unwrap();

    // Return without creating a merge operation
    if op_heads.len() == 1 {
        return Ok((
            op_heads[0].id().clone(),
            first_op_head.store_operation().clone(),
            merged_view,
        ));
    }

    let neighbors_fn = |op: &Operation| op.parents();
    let base_index = index_store.get_index_at_op(&first_op_head, store);
    let mut index = MutableIndex::incremental(base_index);
    for (i, other_op_head) in op_heads.iter().enumerate().skip(1) {
        let other_index = index_store.get_index_at_op(other_op_head, store);
        index.merge_in(&other_index);
        let ancestor_op = dag_walk::closest_common_node(
            op_heads[0..i].to_vec(),
            vec![other_op_head.clone()],
            &neighbors_fn,
            &|op: &Operation| op.id().clone(),
        )
        .unwrap();
        merged_view = view::merge_views(
            store,
            &merged_view,
            ancestor_op.view().store_view(),
            other_op_head.view().store_view(),
        );
    }
    let merged_index = index_store.write_index(index).unwrap();
    let merged_view_id = op_store.write_view(&merged_view).unwrap();
    let operation_metadata = OperationMetadata::new("resolve concurrent operations".to_string());
    let op_parent_ids = op_heads.iter().map(|op| op.id().clone()).collect();
    let merge_operation = op_store::Operation {
        view_id: merged_view_id,
        parents: op_parent_ids,
        metadata: operation_metadata,
    };
    let merge_operation_id = op_store.write_operation(&merge_operation).unwrap();
    index_store
        .associate_file_with_operation(merged_index.as_ref(), &merge_operation_id)
        .unwrap();
    Ok((merge_operation_id, merge_operation, merged_view))
}
