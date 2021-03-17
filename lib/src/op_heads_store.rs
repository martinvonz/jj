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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::lock::FileLock;
use crate::op_store::{OpStore, OperationId, OperationMetadata};
use crate::operation::Operation;
use crate::repo::RepoLoader;
use crate::store::{CommitId, Timestamp};
use crate::transaction::UnpublishedOperation;
use crate::{dag_walk, op_store};

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
        let operation_metadata =
            OperationMetadata::new("initialize repo".to_string(), Timestamp::now());
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

    pub fn get_single_op_head(
        &self,
        repo_loader: &RepoLoader,
    ) -> Result<Operation, OpHeadResolutionError> {
        let mut op_heads = self.get_op_heads();

        if op_heads.is_empty() {
            return Err(OpHeadResolutionError::NoHeads);
        }

        let op_store = repo_loader.op_store();

        if op_heads.len() == 1 {
            let operation_id = op_heads.pop().unwrap();
            let operation = op_store.read_operation(&operation_id).unwrap();
            return Ok(Operation::new(op_store.clone(), operation_id, operation));
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
            return Ok(Operation::new(op_store.clone(), op_head_id, op_head));
        }

        let op_heads: Vec<_> = op_head_ids
            .iter()
            .map(|op_id: &OperationId| {
                let data = op_store.read_operation(op_id).unwrap();
                Operation::new(op_store.clone(), op_id.clone(), data)
            })
            .collect();
        let mut op_heads = self.handle_ancestor_ops(op_heads);

        // Return without creating a merge operation
        if op_heads.len() == 1 {
            return Ok(op_heads.pop().unwrap());
        }

        let merge_operation = merge_op_heads(repo_loader, op_heads)?.leave_unpublished();
        self.add_op_head(merge_operation.id());
        for old_op_head_id in merge_operation.parent_ids() {
            self.remove_op_head(old_op_head_id);
        }
        Ok(merge_operation)
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
            self.remove_op_head(&removed_op_head);
        }
        op_heads.into_iter().collect()
    }
}

fn merge_op_heads(
    repo_loader: &RepoLoader,
    mut op_heads: Vec<Operation>,
) -> Result<UnpublishedOperation, OpHeadResolutionError> {
    op_heads.sort_by_key(|op| op.store_operation().metadata.end_time.timestamp.clone());
    let base_repo = repo_loader.load_at(&op_heads[0]).unwrap();
    let mut tx = base_repo.start_transaction("resolve concurrent operations");
    let merged_repo = tx.mut_repo();
    let neighbors_fn = |op: &Operation| op.parents();
    for (i, other_op_head) in op_heads.iter().enumerate().skip(1) {
        let ancestor_op = dag_walk::closest_common_node(
            op_heads[0..i].to_vec(),
            vec![other_op_head.clone()],
            &neighbors_fn,
            &|op: &Operation| op.id().clone(),
        )
        .unwrap();
        let base_repo = repo_loader.load_at(&ancestor_op).unwrap();
        let other_repo = repo_loader.load_at(&other_op_head).unwrap();
        merged_repo.merge(&base_repo, &other_repo);
    }
    let op_parent_ids = op_heads.iter().map(|op| op.id().clone()).collect();
    tx.set_parents(op_parent_ids);
    // TODO: We already have the resulting View in this case but Operation cannot
    // keep it. Teach Operation to have a cached View so the caller won't have
    // to re-read it from the store (by calling Operation::view())?
    Ok(tx.write())
}
