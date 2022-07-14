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

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, RwLock};

use crate::backend;
use crate::backend::{
    Backend, BackendResult, ChangeId, CommitId, Conflict, ConflictId, FileId, MillisSinceEpoch,
    Signature, SymlinkId, Timestamp, TreeId,
};
use crate::commit::Commit;
use crate::repo_path::RepoPath;
use crate::tree::Tree;
use crate::tree_builder::TreeBuilder;

/// Wraps the low-level backend and makes it return more convenient types. Also
/// adds the root commit and adds caching.
#[derive(Debug)]
pub struct Store {
    backend: Box<dyn Backend>,
    root_commit_id: CommitId,
    commit_cache: RwLock<HashMap<CommitId, Arc<backend::Commit>>>,
    tree_cache: RwLock<HashMap<(RepoPath, TreeId), Arc<backend::Tree>>>,
}

impl Store {
    pub fn new(backend: Box<dyn Backend>) -> Arc<Self> {
        let root_commit_id = CommitId::new(vec![0; backend.hash_length()]);
        Arc::new(Store {
            backend,
            root_commit_id,
            commit_cache: Default::default(),
            tree_cache: Default::default(),
        })
    }

    pub fn hash_length(&self) -> usize {
        self.backend.hash_length()
    }

    pub fn git_repo(&self) -> Option<git2::Repository> {
        self.backend.git_repo()
    }

    pub fn empty_tree_id(&self) -> &TreeId {
        self.backend.empty_tree_id()
    }

    pub fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    pub fn root_commit(self: &Arc<Self>) -> Commit {
        self.get_commit(&self.root_commit_id).unwrap()
    }

    pub fn get_commit(self: &Arc<Self>, id: &CommitId) -> BackendResult<Commit> {
        let data = self.get_backend_commit(id)?;
        Ok(Commit::new(self.clone(), id.clone(), data))
    }

    fn make_root_commit(&self) -> backend::Commit {
        let timestamp = Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        };
        let signature = Signature {
            name: String::new(),
            email: String::new(),
            timestamp,
        };
        let change_id = ChangeId::new(vec![0; 16]);
        backend::Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: self.backend.empty_tree_id().clone(),
            change_id,
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            is_open: false,
        }
    }

    fn get_backend_commit(&self, id: &CommitId) -> BackendResult<Arc<backend::Commit>> {
        {
            let read_locked_cached = self.commit_cache.read().unwrap();
            if let Some(data) = read_locked_cached.get(id).cloned() {
                return Ok(data);
            }
        }
        let commit = if id == self.root_commit_id() {
            self.make_root_commit()
        } else {
            self.backend.read_commit(id)?
        };
        let data = Arc::new(commit);
        let mut write_locked_cache = self.commit_cache.write().unwrap();
        write_locked_cache.insert(id.clone(), data.clone());
        Ok(data)
    }

    pub fn write_commit(self: &Arc<Self>, commit: backend::Commit) -> Commit {
        let commit_id = self.backend.write_commit(&commit).unwrap();
        let data = Arc::new(commit);
        {
            let mut write_locked_cache = self.commit_cache.write().unwrap();
            write_locked_cache.insert(commit_id.clone(), data.clone());
        }

        Commit::new(self.clone(), commit_id, data)
    }

    pub fn get_tree(self: &Arc<Self>, dir: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        let data = self.get_backend_tree(dir, id)?;
        Ok(Tree::new(self.clone(), dir.clone(), id.clone(), data))
    }

    fn get_backend_tree(&self, dir: &RepoPath, id: &TreeId) -> BackendResult<Arc<backend::Tree>> {
        let key = (dir.clone(), id.clone());
        {
            let read_locked_cache = self.tree_cache.read().unwrap();
            if let Some(data) = read_locked_cache.get(&key).cloned() {
                return Ok(data);
            }
        }
        let data = Arc::new(self.backend.read_tree(dir, id)?);
        let mut write_locked_cache = self.tree_cache.write().unwrap();
        write_locked_cache.insert(key, data.clone());
        Ok(data)
    }

    pub fn write_tree(&self, path: &RepoPath, contents: &backend::Tree) -> BackendResult<TreeId> {
        // TODO: This should also do caching like write_commit does.
        self.backend.write_tree(path, contents)
    }

    pub fn read_file(&self, path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        self.backend.read_file(path, id)
    }

    pub fn write_file(&self, path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        self.backend.write_file(path, contents)
    }

    pub fn read_symlink(&self, path: &RepoPath, id: &SymlinkId) -> BackendResult<String> {
        self.backend.read_symlink(path, id)
    }

    pub fn write_symlink(&self, path: &RepoPath, contents: &str) -> BackendResult<SymlinkId> {
        self.backend.write_symlink(path, contents)
    }

    pub fn read_conflict(&self, path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        self.backend.read_conflict(path, id)
    }

    pub fn write_conflict(
        &self,
        path: &RepoPath,
        contents: &Conflict,
    ) -> BackendResult<ConflictId> {
        self.backend.write_conflict(path, contents)
    }

    pub fn tree_builder(self: &Arc<Self>, base_tree_id: TreeId) -> TreeBuilder {
        TreeBuilder::new(self.clone(), base_tree_id)
    }
}
