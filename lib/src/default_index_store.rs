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

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use itertools::Itertools;
use tempfile::NamedTempFile;

use crate::backend::CommitId;
use crate::commit::Commit;
use crate::dag_walk;
use crate::file_util::persist_content_addressed_temp_file;
use crate::index::{Index, IndexLoadError, MutableIndex, ReadonlyIndex};
use crate::index_store::{IndexStore, IndexWriteError};
use crate::op_store::OperationId;
use crate::operation::Operation;
use crate::store::Store;

#[derive(Debug)]
pub struct DefaultIndexStore {
    dir: PathBuf,
}

impl DefaultIndexStore {
    pub fn init(dir: &Path) -> Self {
        std::fs::create_dir(dir.join("operations")).unwrap();
        DefaultIndexStore {
            dir: dir.to_owned(),
        }
    }

    pub fn load(dir: &Path) -> DefaultIndexStore {
        DefaultIndexStore {
            dir: dir.to_owned(),
        }
    }

    fn load_index_at_operation(
        &self,
        commit_id_length: usize,
        change_id_length: usize,
        op_id: &OperationId,
    ) -> Result<Arc<ReadonlyIndex>, IndexLoadError> {
        let op_id_file = self.dir.join("operations").join(op_id.hex());
        let mut buf = vec![];
        File::open(op_id_file)
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        let index_file_id_hex = String::from_utf8(buf).unwrap();
        let index_file_path = self.dir.join(&index_file_id_hex);
        let mut index_file = File::open(index_file_path).unwrap();
        ReadonlyIndex::load_from(
            &mut index_file,
            self.dir.to_owned(),
            index_file_id_hex,
            commit_id_length,
            change_id_length,
        )
    }

    fn index_at_operation(
        &self,
        store: &Arc<Store>,
        operation: &Operation,
    ) -> io::Result<Arc<ReadonlyIndex>> {
        let view = operation.view();
        let operations_dir = self.dir.join("operations");
        let commit_id_length = store.commit_id_length();
        let change_id_length = store.change_id_length();
        let mut new_heads = view.heads().clone();
        let mut parent_op_id: Option<OperationId> = None;
        for op in dag_walk::bfs(
            vec![operation.clone()],
            Box::new(|op: &Operation| op.id().clone()),
            Box::new(|op: &Operation| op.parents()),
        ) {
            if operations_dir.join(op.id().hex()).is_file() {
                if parent_op_id.is_none() {
                    parent_op_id = Some(op.id().clone())
                }
            } else {
                for head in op.view().heads() {
                    new_heads.insert(head.clone());
                }
            }
        }
        let mut data;
        let maybe_parent_file;
        match parent_op_id {
            None => {
                maybe_parent_file = None;
                data = MutableIndex::full(commit_id_length, change_id_length);
            }
            Some(parent_op_id) => {
                let parent_file = self
                    .load_index_at_operation(commit_id_length, change_id_length, &parent_op_id)
                    .unwrap();
                maybe_parent_file = Some(parent_file.clone());
                data = MutableIndex::incremental(parent_file)
            }
        }

        let mut heads = new_heads.into_iter().collect_vec();
        heads.sort();
        let commits = topo_order_earlier_first(store, heads, maybe_parent_file);

        for commit in &commits {
            data.add_commit(commit);
        }

        let index_file = data.save_in(self.dir.clone())?;

        self.associate_file_with_operation(&index_file, operation.id())?;

        Ok(index_file)
    }

    /// Records a link from the given operation to the this index version.
    fn associate_file_with_operation(
        &self,
        index: &ReadonlyIndex,
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
    fn name(&self) -> &str {
        "default"
    }

    fn get_index_at_op(&self, op: &Operation, store: &Arc<Store>) -> Arc<ReadonlyIndex> {
        let op_id_hex = op.id().hex();
        let op_id_file = self.dir.join("operations").join(op_id_hex);
        if op_id_file.exists() {
            match self.load_index_at_operation(
                store.commit_id_length(),
                store.change_id_length(),
                op.id(),
            ) {
                Err(IndexLoadError::IndexCorrupt(_)) => {
                    // If the index was corrupt (maybe it was written in a different format),
                    // we just reindex.
                    // TODO: Move this message to a callback or something.
                    println!("The index was corrupt (maybe the format has changed). Reindexing...");
                    std::fs::remove_dir_all(self.dir.join("operations")).unwrap();
                    std::fs::create_dir(self.dir.join("operations")).unwrap();
                    self.index_at_operation(store, op).unwrap()
                }
                result => result.unwrap(),
            }
        } else {
            self.index_at_operation(store, op).unwrap()
        }
    }

    fn write_index(
        &self,
        index: MutableIndex,
        op_id: &OperationId,
    ) -> Result<Arc<ReadonlyIndex>, IndexWriteError> {
        let index = index.save_in(self.dir.clone()).map_err(|err| {
            IndexWriteError::Other(format!("Failed to write commit index file: {err:?}"))
        })?;
        self.associate_file_with_operation(&index, op_id)
            .map_err(|err| {
                IndexWriteError::Other(format!(
                    "Failed to associate commit index file with a operation {op_id:?}: {err:?}"
                ))
            })?;
        Ok(index)
    }
}

// Returns the ancestors of heads with parents and predecessors come before the
// commit itself
fn topo_order_earlier_first(
    store: &Arc<Store>,
    heads: Vec<CommitId>,
    parent_file: Option<Arc<ReadonlyIndex>>,
) -> Vec<Commit> {
    // First create a list of all commits in topological order with
    // children/successors first (reverse of what we want)
    let mut work = vec![];
    for head in &heads {
        work.push(store.get_commit(head).unwrap());
    }
    let mut commits = vec![];
    let mut visited = HashSet::new();
    let mut in_parent_file = HashSet::new();
    let parent_file_source = parent_file.as_ref().map(|file| file.as_ref());
    while let Some(commit) = work.pop() {
        if parent_file_source.map_or(false, |index| index.has_id(commit.id())) {
            in_parent_file.insert(commit.id().clone());
            continue;
        } else if !visited.insert(commit.id().clone()) {
            continue;
        }

        work.extend(commit.parents());
        work.extend(commit.predecessors());
        commits.push(commit);
    }
    drop(visited);

    // Now create the topological order with earlier commits first. If we run into
    // any commits whose parents/predecessors have not all been indexed, put
    // them in the map of waiting commit (keyed by the commit they're waiting
    // for). Note that the order in the graph doesn't really have to be
    // topological, but it seems like a useful property to have.

    // Commits waiting for their parents/predecessors to be added
    let mut waiting = HashMap::new();

    let mut result = vec![];
    let mut visited = in_parent_file;
    while let Some(commit) = commits.pop() {
        let mut waiting_for_earlier_commit = false;
        for earlier in commit
            .parent_ids()
            .iter()
            .chain(commit.predecessor_ids().iter())
        {
            if !visited.contains(earlier) {
                waiting
                    .entry(earlier.clone())
                    .or_insert_with(Vec::new)
                    .push(commit.clone());
                waiting_for_earlier_commit = true;
                break;
            }
        }
        if !waiting_for_earlier_commit {
            visited.insert(commit.id().clone());
            if let Some(dependents) = waiting.remove(commit.id()) {
                commits.extend(dependents);
            }
            result.push(commit);
        }
    }
    assert!(waiting.is_empty());
    result
}
