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

use std::cmp::min;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::commit::Commit;
use crate::dag_walk;
use crate::lock::FileLock;
use crate::op_store;
use crate::op_store::{OpStore, OpStoreResult, OperationId, OperationMetadata};
use crate::operation::Operation;
use crate::simple_op_store::SimpleOpStore;
use crate::store::{CommitId, Timestamp};
use crate::store_wrapper::StoreWrapper;

pub trait View {
    fn checkout(&self) -> &CommitId;
    fn heads<'a>(&'a self) -> Box<dyn Iterator<Item = &'a CommitId> + 'a>;
    fn op_store(&self) -> Arc<dyn OpStore>;
    fn base_op_head_id(&self) -> &OperationId;

    fn get_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        let data = self.op_store().read_operation(id)?;
        Ok(Operation::new(self.op_store().clone(), id.clone(), data))
    }

    fn base_op_head(&self) -> Operation {
        self.get_operation(self.base_op_head_id()).unwrap()
    }
}

pub struct ReadonlyView {
    store: Arc<StoreWrapper>,
    path: PathBuf,
    op_store: Arc<SimpleOpStore>,
    op_id: OperationId,
    data: op_store::View,
}

pub struct MutableView {
    store: Arc<StoreWrapper>,
    path: PathBuf,
    op_store: Arc<SimpleOpStore>,
    base_op_head_id: OperationId,
    data: op_store::View,
}

fn heads_of_set(
    store: &StoreWrapper,
    commit_ids: impl Iterator<Item = CommitId>,
) -> HashSet<CommitId> {
    let mut visited = HashSet::new();
    let mut work = vec![];
    let mut oldest = std::u64::MAX;
    let mut heads: HashSet<CommitId> = commit_ids.collect();
    for commit_id in &heads {
        let commit = store.get_commit(commit_id).unwrap();
        oldest = min(oldest, commit.committer().timestamp.timestamp.0);
        work.push(commit);
    }
    // Assume clock skew less than a month:
    // TODO: use generation numbers here
    let threshold = oldest.saturating_sub(1000 * 3600 * 24 * 30);
    while !work.is_empty() {
        let commit = work.pop().unwrap();
        if visited.contains(commit.id()) {
            continue;
        }
        visited.insert(commit.id().clone());

        for parent in commit.parents() {
            if parent.committer().timestamp.timestamp.0 < threshold {
                continue;
            }
            heads.remove(parent.id());
            work.push(parent);
        }
    }
    heads
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpHeadResolutionError {
    #[error("Operation log has no heads")]
    NoHeads,
}

fn add_op_head(op_heads_dir: &PathBuf, id: &OperationId) {
    std::fs::write(op_heads_dir.join(id.hex()), "").unwrap();
}

fn remove_op_head(op_heads_dir: &PathBuf, id: &OperationId) {
    // It's fine if the old head was not found. It probably means
    // that we're on a distributed file system where the locking
    // doesn't work. We'll probably end up with two current
    // heads. We'll detect that next time we load the view.
    std::fs::remove_file(op_heads_dir.join(id.hex())).ok();
}

fn get_op_heads(op_heads_dir: &PathBuf) -> Vec<OperationId> {
    let mut op_heads = vec![];
    for op_head_entry in std::fs::read_dir(op_heads_dir).unwrap() {
        let op_head_file_name = op_head_entry.unwrap().file_name();
        let op_head_file_name = op_head_file_name.to_str().unwrap();
        if let Ok(op_head) = hex::decode(op_head_file_name) {
            op_heads.push(OperationId(op_head));
        }
    }
    op_heads
}

pub fn merge_views(
    store: &StoreWrapper,
    left: &op_store::View,
    base: &op_store::View,
    right: &op_store::View,
) -> op_store::View {
    let mut result = left.clone();
    if right.checkout == base.checkout || right.checkout == left.checkout {
        // Keep the left side
    } else if left.checkout == base.checkout {
        result.checkout = right.checkout.clone();
    } else {
        // TODO: Return an error here. Or should we just pick one of the sides
        // and emit a warning?
    }

    for removed_head in base.head_ids.difference(&right.head_ids) {
        result.head_ids.remove(removed_head);
    }
    for added_head in right.head_ids.difference(&base.head_ids) {
        result.head_ids.insert(added_head.clone());
    }
    result.head_ids = heads_of_set(store, result.head_ids.into_iter());
    // TODO: Should it be considered a conflict if a commit-head is removed on one
    // side while a child or successor is created on another side? Maybe a
    // warning?

    result
}

// TODO: Introduce context objects (like commit::Commit) so we won't have to
// pass around OperationId and Operation separately like we do here.
fn get_single_op_head(
    store: &StoreWrapper,
    op_store: &dyn OpStore,
    op_heads_dir: &PathBuf,
) -> Result<(OperationId, op_store::Operation, op_store::View), OpHeadResolutionError> {
    let mut op_heads = get_op_heads(&op_heads_dir);

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
    let _lock = FileLock::lock(op_heads_dir.join("lock"));
    let op_heads = get_op_heads(&op_heads_dir);

    if op_heads.is_empty() {
        return Err(OpHeadResolutionError::NoHeads);
    }

    if op_heads.len() == 1 {
        let op_head_id = op_heads[0].clone();
        let op_head = op_store.read_operation(&op_head_id).unwrap();
        // Return early so we don't write a merge operation with a single parent
        let view = op_store.read_view(&op_head.view_id).unwrap();
        return Ok((op_head_id, op_head, view));
    }

    let (merge_operation_id, merge_operation, merged_view) =
        merge_op_heads(store, op_store, &op_heads)?;
    add_op_head(&op_heads_dir, &merge_operation_id);
    for old_op_head_id in op_heads {
        // The merged one will be in the input to the merge if it's a "fast-forward"
        // merge.
        if old_op_head_id != merge_operation_id {
            remove_op_head(&op_heads_dir, &old_op_head_id);
        }
    }
    Ok((merge_operation_id, merge_operation, merged_view))
}

fn merge_op_heads(
    store: &StoreWrapper,
    op_store: &dyn OpStore,
    op_heads: &[OperationId],
) -> Result<(OperationId, op_store::Operation, op_store::View), OpHeadResolutionError> {
    let neighbors_fn = |op_id: &OperationId| op_store.read_operation(op_id).unwrap().parents;
    // Remove ancestors so we don't create merge operation with an operation and its
    // ancestor
    let op_heads = dag_walk::unreachable(
        op_heads.iter().cloned(),
        &neighbors_fn,
        &|op_id: &OperationId| op_id.clone(),
    );
    let mut op_heads: Vec<_> = op_heads.into_iter().collect();
    op_heads.sort_by_key(|op_id| op_id.0.clone());
    let first_op_head = op_store.read_operation(&op_heads[0]).unwrap();
    let mut merged_view = op_store.read_view(&first_op_head.view_id).unwrap();

    // Return without creating a merge operation
    if op_heads.len() == 1 {
        return Ok((op_heads[0].clone(), first_op_head, merged_view));
    }

    for (i, other_op_head_id) in op_heads.iter().enumerate().skip(1) {
        let ancestor_op_id = dag_walk::closest_common_node(
            op_heads[0..i].to_vec(),
            vec![other_op_head_id.clone()],
            &neighbors_fn,
            &|op_id: &OperationId| op_id.clone(),
        )
        .unwrap();
        let ancestor_op = op_store.read_operation(&ancestor_op_id).unwrap();
        let ancestor_view = op_store.read_view(&ancestor_op.view_id).unwrap();
        let other_op = op_store.read_operation(other_op_head_id).unwrap();
        let other_view = op_store.read_view(&other_op.view_id).unwrap();
        merged_view = merge_views(store, &merged_view, &ancestor_view, &other_view);
    }
    let merged_view_id = op_store.write_view(&merged_view).unwrap();
    let operation_metadata = OperationMetadata::new("resolve concurrent operations".to_string());
    let merge_operation = op_store::Operation {
        view_id: merged_view_id,
        parents: op_heads,
        metadata: operation_metadata,
    };
    let merge_operation_id = op_store.write_operation(&merge_operation).unwrap();
    Ok((merge_operation_id, merge_operation, merged_view))
}

impl View for ReadonlyView {
    fn checkout(&self) -> &CommitId {
        &self.data.checkout
    }

    fn heads<'a>(&'a self) -> Box<dyn Iterator<Item = &'a CommitId> + 'a> {
        Box::new(self.data.head_ids.iter())
    }

    fn op_store(&self) -> Arc<dyn OpStore> {
        self.op_store.clone()
    }

    fn base_op_head_id(&self) -> &OperationId {
        &self.op_id
    }
}

impl ReadonlyView {
    pub fn init(store: Arc<StoreWrapper>, path: PathBuf, checkout: CommitId) -> Self {
        std::fs::create_dir(path.join("op_store")).unwrap();

        let op_store = Arc::new(SimpleOpStore::init(path.join("op_store")));
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

        let op_heads_dir = path.join("op_heads");
        std::fs::create_dir(&op_heads_dir).unwrap();
        add_op_head(&op_heads_dir, &init_operation_id);

        ReadonlyView {
            store,
            path,
            op_store,
            op_id: init_operation_id,
            data: root_view,
        }
    }

    pub fn load(store: Arc<StoreWrapper>, path: PathBuf) -> Self {
        let op_store = Arc::new(SimpleOpStore::load(path.join("op_store")));
        let op_heads_dir = path.join("op_heads");
        let (op_id, _operation, view) =
            get_single_op_head(&store, op_store.as_ref(), &op_heads_dir).unwrap();
        ReadonlyView {
            store,
            path,
            op_store,
            op_id,
            data: view,
        }
    }

    pub fn reload(&mut self) -> OperationId {
        let op_heads_dir = self.path.join("op_heads");
        let (op_id, _operation, view) =
            get_single_op_head(&self.store, self.op_store.as_ref(), &op_heads_dir).unwrap();
        self.op_id = op_id;
        self.data = view;
        self.op_id.clone()
    }

    pub fn reload_at(&mut self, operation: &Operation) {
        self.op_id = operation.id().clone();
        self.data = operation.view().take_store_view();
    }

    pub fn start_modification(&self) -> MutableView {
        // TODO: Avoid the cloning of the sets here.
        MutableView {
            store: self.store.clone(),
            path: self.path.clone(),
            op_store: self.op_store.clone(),
            base_op_head_id: self.op_id.clone(),
            data: self.data.clone(),
        }
    }
}

impl View for MutableView {
    fn checkout(&self) -> &CommitId {
        &self.data.checkout
    }

    fn heads<'a>(&'a self) -> Box<dyn Iterator<Item = &'a CommitId> + 'a> {
        Box::new(self.data.head_ids.iter())
    }

    fn op_store(&self) -> Arc<dyn OpStore> {
        self.op_store.clone()
    }

    fn base_op_head_id(&self) -> &OperationId {
        &self.base_op_head_id
    }
}

impl MutableView {
    pub fn set_checkout(&mut self, id: CommitId) {
        self.data.checkout = id;
    }

    pub fn add_head(&mut self, head: &Commit) {
        self.data.head_ids.insert(head.id().clone());
        for parent in head.parents() {
            self.data.head_ids.remove(parent.id());
        }
    }

    pub fn remove_head(&mut self, head: &Commit) {
        self.data.head_ids.remove(head.id());
        for parent in head.parents() {
            self.data.head_ids.insert(parent.id().clone());
        }
    }

    pub fn set_view(&mut self, data: op_store::View) {
        self.data = data;
    }

    pub fn save(mut self, description: String, operation_start_time: Timestamp) -> Operation {
        let op_heads_dir = self.path.join("op_heads");

        // First write the current view whether or not there have been any concurrent
        // operations. We'll later create a merge operation if necessary.
        self.data.head_ids = heads_of_set(&self.store, self.heads().cloned());
        let view_id = self.op_store.write_view(&self.data).unwrap();
        let mut operation_metadata = OperationMetadata::new(description);
        operation_metadata.start_time = operation_start_time;
        let operation = op_store::Operation {
            view_id,
            parents: vec![self.base_op_head_id.clone()],
            metadata: operation_metadata,
        };
        let old_op_head_id = self.base_op_head_id.clone();
        let new_op_head_id = self.op_store.write_operation(&operation).unwrap();

        // Update .jj/view/op_heads/.
        {
            let _op_heads_lock = FileLock::lock(op_heads_dir.join("lock"));
            add_op_head(&op_heads_dir, &new_op_head_id);
            remove_op_head(&op_heads_dir, &old_op_head_id);
        }

        Operation::new(self.op_store, new_op_head_id, operation)
    }
}
