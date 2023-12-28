// Copyright 2023 The Jujutsu Authors
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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

use itertools::Itertools;
use tempfile::NamedTempFile;
use thiserror::Error;

use super::mutable::DefaultMutableIndex;
use super::readonly::{DefaultReadonlyIndex, ReadonlyIndexLoadError, ReadonlyIndexSegment};
use crate::backend::{BackendError, BackendInitError, CommitId, ObjectId};
use crate::commit::CommitByCommitterTimestamp;
use crate::dag_walk;
use crate::file_util::{persist_content_addressed_temp_file, IoResultExt as _, PathError};
use crate::index::{
    Index, IndexReadError, IndexStore, IndexWriteError, MutableIndex, ReadonlyIndex,
};
use crate::op_store::{OpStoreError, OperationId};
use crate::operation::Operation;
use crate::store::Store;

/// Error that may occur during `DefaultIndexStore` initialization.
#[derive(Debug, Error)]
#[error("Failed to initialize index store: {0}")]
pub struct DefaultIndexStoreInitError(#[from] pub PathError);

impl From<DefaultIndexStoreInitError> for BackendInitError {
    fn from(err: DefaultIndexStoreInitError) -> Self {
        BackendInitError(err.into())
    }
}

#[derive(Debug, Error)]
pub enum DefaultIndexStoreError {
    #[error(
        "Failed to associate commit index file with an operation {op_id}: {source}",
        op_id = op_id.hex()
    )]
    AssociateIndex {
        op_id: OperationId,
        source: io::Error,
    },
    #[error("Failed to load associated commit index file name: {0}")]
    LoadAssociation(#[source] io::Error),
    #[error(transparent)]
    LoadIndex(ReadonlyIndexLoadError),
    #[error("Failed to write commit index file: {0}")]
    SaveIndex(#[source] io::Error),
    #[error("Failed to index commits: {0}")]
    IndexCommits(#[source] BackendError),
    #[error(transparent)]
    OpStore(#[from] OpStoreError),
}

#[derive(Debug)]
pub struct DefaultIndexStore {
    dir: PathBuf,
}

impl DefaultIndexStore {
    pub fn name() -> &'static str {
        "default"
    }

    pub fn init(dir: &Path) -> Result<Self, DefaultIndexStoreInitError> {
        let op_dir = dir.join("operations");
        std::fs::create_dir(&op_dir).context(&op_dir)?;
        Ok(DefaultIndexStore {
            dir: dir.to_owned(),
        })
    }

    pub fn load(dir: &Path) -> DefaultIndexStore {
        DefaultIndexStore {
            dir: dir.to_owned(),
        }
    }

    pub fn reinit(&self) -> Result<(), DefaultIndexStoreInitError> {
        let op_dir = self.dir.join("operations");
        std::fs::remove_dir_all(&op_dir).context(&op_dir)?;
        std::fs::create_dir(&op_dir).context(&op_dir)?;
        Ok(())
    }

    fn load_index_segments_at_operation(
        &self,
        op_id: &OperationId,
        commit_id_length: usize,
        change_id_length: usize,
    ) -> Result<Arc<ReadonlyIndexSegment>, DefaultIndexStoreError> {
        let op_id_file = self.dir.join("operations").join(op_id.hex());
        let index_file_id_hex =
            fs::read_to_string(op_id_file).map_err(DefaultIndexStoreError::LoadAssociation)?;
        ReadonlyIndexSegment::load(
            &self.dir,
            index_file_id_hex,
            commit_id_length,
            change_id_length,
        )
        .map_err(DefaultIndexStoreError::LoadIndex)
    }

    #[tracing::instrument(skip(self, store))]
    fn build_index_segments_at_operation(
        &self,
        operation: &Operation,
        store: &Arc<Store>,
    ) -> Result<Arc<ReadonlyIndexSegment>, DefaultIndexStoreError> {
        let view = operation.view()?;
        let operations_dir = self.dir.join("operations");
        let commit_id_length = store.commit_id_length();
        let change_id_length = store.change_id_length();
        let mut new_heads = view.heads().clone();
        let mut parent_op_id: Option<OperationId> = None;
        for op in dag_walk::dfs_ok(
            [Ok(operation.clone())],
            |op: &Operation| op.id().clone(),
            |op: &Operation| op.parents().collect_vec(),
        ) {
            let op = op?;
            // Pick the latest existing ancestor operation as the parent
            // segment. Perhaps, breadth-first search is more appropriate here,
            // but that wouldn't matter in practice as the operation log is
            // mostly linear.
            if parent_op_id.is_none() && operations_dir.join(op.id().hex()).is_file() {
                parent_op_id = Some(op.id().clone());
            }
            // TODO: no need to walk ancestors of the parent_op_id operation
            for head in op.view()?.heads() {
                new_heads.insert(head.clone());
            }
        }
        let maybe_parent_file;
        let mut mutable_index;
        match parent_op_id {
            None => {
                maybe_parent_file = None;
                mutable_index = DefaultMutableIndex::full(commit_id_length, change_id_length);
            }
            Some(parent_op_id) => {
                let parent_file = self.load_index_segments_at_operation(
                    &parent_op_id,
                    commit_id_length,
                    change_id_length,
                )?;
                maybe_parent_file = Some(parent_file.clone());
                mutable_index = DefaultMutableIndex::incremental(parent_file)
            }
        }

        tracing::info!(
            ?maybe_parent_file,
            new_heads_count = new_heads.len(),
            "indexing commits reachable from historical heads"
        );
        // Build a list of ancestors of heads where parents and predecessors come after
        // the commit itself.
        let parent_file_has_id = |id: &CommitId| {
            maybe_parent_file
                .as_ref()
                .map_or(false, |segment| segment.as_composite().has_id(id))
        };
        let commits = dag_walk::topo_order_reverse_ord_ok(
            new_heads
                .iter()
                .filter(|&id| !parent_file_has_id(id))
                .map(|id| store.get_commit(id))
                .map_ok(CommitByCommitterTimestamp),
            |CommitByCommitterTimestamp(commit)| commit.id().clone(),
            |CommitByCommitterTimestamp(commit)| {
                itertools::chain(commit.parent_ids(), commit.predecessor_ids())
                    .filter(|&id| !parent_file_has_id(id))
                    .map(|id| store.get_commit(id))
                    .map_ok(CommitByCommitterTimestamp)
                    .collect_vec()
            },
        )
        .map_err(DefaultIndexStoreError::IndexCommits)?;
        for CommitByCommitterTimestamp(commit) in commits.iter().rev() {
            mutable_index.add_commit(commit);
        }

        let index_file = self.save_mutable_index(mutable_index, operation.id())?;
        tracing::info!(
            ?index_file,
            commits_count = commits.len(),
            "saved new index file"
        );

        Ok(index_file)
    }

    fn save_mutable_index(
        &self,
        mutable_index: DefaultMutableIndex,
        op_id: &OperationId,
    ) -> Result<Arc<ReadonlyIndexSegment>, DefaultIndexStoreError> {
        let index_segment = mutable_index
            .squash_and_save_in(&self.dir)
            .map_err(DefaultIndexStoreError::SaveIndex)?;
        self.associate_file_with_operation(&index_segment, op_id)
            .map_err(|source| DefaultIndexStoreError::AssociateIndex {
                op_id: op_id.to_owned(),
                source,
            })?;
        Ok(index_segment)
    }

    /// Records a link from the given operation to the this index version.
    fn associate_file_with_operation(
        &self,
        index: &ReadonlyIndexSegment,
        op_id: &OperationId,
    ) -> io::Result<()> {
        let mut temp_file = NamedTempFile::new_in(&self.dir)?;
        let file = temp_file.as_file_mut();
        file.write_all(index.name().as_bytes())?;
        persist_content_addressed_temp_file(
            temp_file,
            self.dir.join("operations").join(op_id.hex()),
        )?;
        Ok(())
    }
}

impl IndexStore for DefaultIndexStore {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        Self::name()
    }

    fn get_index_at_op(
        &self,
        op: &Operation,
        store: &Arc<Store>,
    ) -> Result<Box<dyn ReadonlyIndex>, IndexReadError> {
        let op_id_hex = op.id().hex();
        let op_id_file = self.dir.join("operations").join(op_id_hex);
        let index_segment = if op_id_file.exists() {
            match self.load_index_segments_at_operation(
                op.id(),
                store.commit_id_length(),
                store.change_id_length(),
            ) {
                Err(DefaultIndexStoreError::LoadIndex(err)) if err.is_corrupt_or_not_found() => {
                    // If the index was corrupt (maybe it was written in a different format),
                    // we just reindex.
                    // TODO: Move this message to a callback or something.
                    println!("The index was corrupt (maybe the format has changed). Reindexing...");
                    self.reinit().map_err(|err| IndexReadError(err.into()))?;
                    self.build_index_segments_at_operation(op, store)
                }
                result => result,
            }
        } else {
            self.build_index_segments_at_operation(op, store)
        }
        .map_err(|err| IndexReadError(err.into()))?;
        Ok(Box::new(DefaultReadonlyIndex::from_segment(index_segment)))
    }

    fn write_index(
        &self,
        index: Box<dyn MutableIndex>,
        op_id: &OperationId,
    ) -> Result<Box<dyn ReadonlyIndex>, IndexWriteError> {
        let index = index
            .into_any()
            .downcast::<DefaultMutableIndex>()
            .expect("index to merge in must be a DefaultMutableIndex");
        let index_segment = self
            .save_mutable_index(*index, op_id)
            .map_err(|err| IndexWriteError(err.into()))?;
        Ok(Box::new(DefaultReadonlyIndex::from_segment(index_segment)))
    }
}
