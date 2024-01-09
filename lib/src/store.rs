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

#![allow(missing_docs)]

use std::any::Any;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::io::Read;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use pollster::FutureExt;

use crate::backend::{
    self, Backend, BackendResult, ChangeId, CommitId, ConflictId, FileId, MergedTreeId, SigningFn,
    SymlinkId, TreeId,
};
use crate::commit::Commit;
use crate::index::Index;
use crate::merge::{Merge, MergedTreeValue};
use crate::merged_tree::MergedTree;
use crate::repo_path::{RepoPath, RepoPathBuf};
use crate::signing::Signer;
use crate::tree::Tree;
use crate::tree_builder::TreeBuilder;

/// Wraps the low-level backend and makes it return more convenient types. Also
/// adds caching.
pub struct Store {
    backend: Box<dyn Backend>,
    signer: Signer,
    commit_cache: RwLock<HashMap<CommitId, Arc<backend::Commit>>>,
    tree_cache: RwLock<HashMap<(RepoPathBuf, TreeId), Arc<backend::Tree>>>,
    use_tree_conflict_format: bool,
}

impl Debug for Store {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Store")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Store {
    pub fn new(
        backend: Box<dyn Backend>,
        signer: Signer,
        use_tree_conflict_format: bool,
    ) -> Arc<Self> {
        Arc::new(Store {
            backend,
            signer,
            commit_cache: Default::default(),
            tree_cache: Default::default(),
            use_tree_conflict_format,
        })
    }

    pub fn backend_impl(&self) -> &dyn Any {
        self.backend.as_any()
    }

    pub fn signer(&self) -> &Signer {
        &self.signer
    }

    /// Whether new tree should be written using the tree-level format.
    pub fn use_tree_conflict_format(&self) -> bool {
        self.use_tree_conflict_format
    }

    pub fn commit_id_length(&self) -> usize {
        self.backend.commit_id_length()
    }

    pub fn change_id_length(&self) -> usize {
        self.backend.change_id_length()
    }

    pub fn root_commit_id(&self) -> &CommitId {
        self.backend.root_commit_id()
    }

    pub fn root_change_id(&self) -> &ChangeId {
        self.backend.root_change_id()
    }

    pub fn empty_tree_id(&self) -> &TreeId {
        self.backend.empty_tree_id()
    }

    pub fn concurrency(&self) -> usize {
        self.backend.concurrency()
    }

    pub fn empty_merged_tree_id(&self) -> MergedTreeId {
        MergedTreeId::Legacy(self.backend.empty_tree_id().clone())
    }

    pub fn root_commit(self: &Arc<Self>) -> Commit {
        self.get_commit(self.backend.root_commit_id()).unwrap()
    }

    pub fn get_commit(self: &Arc<Self>, id: &CommitId) -> BackendResult<Commit> {
        self.get_commit_async(id).block_on()
    }

    pub async fn get_commit_async(self: &Arc<Self>, id: &CommitId) -> BackendResult<Commit> {
        let data = self.get_backend_commit(id).await?;
        Ok(Commit::new(self.clone(), id.clone(), data))
    }

    async fn get_backend_commit(&self, id: &CommitId) -> BackendResult<Arc<backend::Commit>> {
        {
            let read_locked_cached = self.commit_cache.read().unwrap();
            if let Some(data) = read_locked_cached.get(id).cloned() {
                return Ok(data);
            }
        }
        let commit = self.backend.read_commit(id).await?;
        let data = Arc::new(commit);
        let mut write_locked_cache = self.commit_cache.write().unwrap();
        write_locked_cache.insert(id.clone(), data.clone());
        Ok(data)
    }

    pub fn write_commit(
        self: &Arc<Self>,
        commit: backend::Commit,
        sign_with: Option<&mut SigningFn>,
    ) -> BackendResult<Commit> {
        assert!(!commit.parents.is_empty());

        let (commit_id, commit) = self.backend.write_commit(commit, sign_with)?;
        let data = Arc::new(commit);
        {
            let mut write_locked_cache = self.commit_cache.write().unwrap();
            write_locked_cache.insert(commit_id.clone(), data.clone());
        }

        Ok(Commit::new(self.clone(), commit_id, data))
    }

    pub fn get_tree(self: &Arc<Self>, dir: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        self.get_tree_async(dir, id).block_on()
    }

    pub async fn get_tree_async(
        self: &Arc<Self>,
        dir: &RepoPath,
        id: &TreeId,
    ) -> BackendResult<Tree> {
        let data = self.get_backend_tree(dir, id).await?;
        Ok(Tree::new(self.clone(), dir.to_owned(), id.clone(), data))
    }

    async fn get_backend_tree(
        &self,
        dir: &RepoPath,
        id: &TreeId,
    ) -> BackendResult<Arc<backend::Tree>> {
        let key = (dir.to_owned(), id.clone());
        {
            let read_locked_cache = self.tree_cache.read().unwrap();
            if let Some(data) = read_locked_cache.get(&key).cloned() {
                return Ok(data);
            }
        }
        let data = self.backend.read_tree(dir, id).await?;
        let data = Arc::new(data);
        let mut write_locked_cache = self.tree_cache.write().unwrap();
        write_locked_cache.insert(key, data.clone());
        Ok(data)
    }

    pub fn get_root_tree(self: &Arc<Self>, id: &MergedTreeId) -> BackendResult<MergedTree> {
        match &id {
            MergedTreeId::Legacy(id) => {
                let tree = self.get_tree(RepoPath::root(), id)?;
                Ok(MergedTree::Legacy(tree))
            }
            MergedTreeId::Merge(ids) => {
                let trees = ids.try_map(|id| self.get_tree(RepoPath::root(), id))?;
                Ok(MergedTree::Merge(trees))
            }
        }
    }

    pub fn write_tree(
        self: &Arc<Self>,
        path: &RepoPath,
        tree: backend::Tree,
    ) -> BackendResult<Tree> {
        let tree_id = self.backend.write_tree(path, &tree)?;
        let data = Arc::new(tree);
        {
            let mut write_locked_cache = self.tree_cache.write().unwrap();
            write_locked_cache.insert((path.to_owned(), tree_id.clone()), data.clone());
        }

        Ok(Tree::new(self.clone(), path.to_owned(), tree_id, data))
    }

    pub fn read_file(&self, path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        self.read_file_async(path, id).block_on()
    }

    pub async fn read_file_async(
        &self,
        path: &RepoPath,
        id: &FileId,
    ) -> BackendResult<Box<dyn Read>> {
        self.backend.read_file(path, id).await
    }

    pub fn write_file(&self, path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        self.backend.write_file(path, contents)
    }

    pub fn read_symlink(&self, path: &RepoPath, id: &SymlinkId) -> BackendResult<String> {
        self.read_symlink_async(path, id).block_on()
    }

    pub async fn read_symlink_async(
        &self,
        path: &RepoPath,
        id: &SymlinkId,
    ) -> BackendResult<String> {
        self.backend.read_symlink(path, id).await
    }

    pub fn write_symlink(&self, path: &RepoPath, contents: &str) -> BackendResult<SymlinkId> {
        self.backend.write_symlink(path, contents)
    }

    pub fn read_conflict(
        &self,
        path: &RepoPath,
        id: &ConflictId,
    ) -> BackendResult<MergedTreeValue> {
        let backend_conflict = self.backend.read_conflict(path, id)?;
        Ok(Merge::from_backend_conflict(backend_conflict))
    }

    pub fn write_conflict(
        &self,
        path: &RepoPath,
        contents: &MergedTreeValue,
    ) -> BackendResult<ConflictId> {
        self.backend
            .write_conflict(path, &contents.clone().into_backend_conflict())
    }

    pub fn tree_builder(self: &Arc<Self>, base_tree_id: TreeId) -> TreeBuilder {
        TreeBuilder::new(self.clone(), base_tree_id)
    }

    pub fn gc(&self, index: &dyn Index, keep_newer: SystemTime) -> BackendResult<()> {
        self.backend.gc(index, keep_newer)
    }
}
