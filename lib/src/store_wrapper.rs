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
use std::sync::{Arc, RwLock, Weak};

use crate::commit::Commit;
use crate::repo_path::{DirRepoPath, FileRepoPath};
use crate::store;
use crate::store::{
    ChangeId, CommitId, Conflict, ConflictId, FileId, MillisSinceEpoch, Signature, Store,
    StoreResult, SymlinkId, Timestamp, TreeId,
};
use crate::tree::Tree;
use crate::tree_builder::TreeBuilder;

/// Wraps the low-level store and makes it return more convenient types. Also
/// adds the root commit and adds caching.
/// TODO: Come up with a better name, possibly by renaming the current Store
/// trait to something else.
#[derive(Debug)]
pub struct StoreWrapper {
    weak_self: Option<Weak<StoreWrapper>>,
    store: Box<dyn Store>,
    root_commit_id: CommitId,
    commit_cache: RwLock<HashMap<CommitId, Arc<store::Commit>>>,
    tree_cache: RwLock<HashMap<(DirRepoPath, TreeId), Arc<store::Tree>>>,
}

impl StoreWrapper {
    pub fn new(store: Box<dyn Store>) -> Arc<Self> {
        let root_commit_id = CommitId(vec![0; store.hash_length()]);
        let mut wrapper = Arc::new(StoreWrapper {
            weak_self: None,
            store,
            root_commit_id,
            commit_cache: Default::default(),
            tree_cache: Default::default(),
        });
        let weak_self = Arc::downgrade(&wrapper);
        let mut ref_mut = unsafe { Arc::get_mut_unchecked(&mut wrapper) };
        ref_mut.weak_self = Some(weak_self);
        wrapper
    }

    pub fn hash_length(&self) -> usize {
        self.store.hash_length()
    }

    pub fn git_repo(&self) -> Option<git2::Repository> {
        self.store.git_repo()
    }

    pub fn empty_tree_id(&self) -> &TreeId {
        self.store.empty_tree_id()
    }

    pub fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    pub fn root_commit(&self) -> Commit {
        self.get_commit(&self.root_commit_id).unwrap()
    }

    pub fn get_commit(&self, id: &CommitId) -> StoreResult<Commit> {
        let data = self.get_store_commit(id)?;
        Ok(Commit::new(
            self.weak_self.as_ref().unwrap().upgrade().unwrap(),
            id.clone(),
            data,
        ))
    }

    fn make_root_commit(&self) -> store::Commit {
        let timestamp = Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        };
        let signature = Signature {
            name: String::new(),
            email: String::new(),
            timestamp,
        };
        let change_id = ChangeId(vec![0; 16]);
        store::Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: self.store.empty_tree_id().clone(),
            change_id,
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            is_open: false,
            is_pruned: false,
        }
    }

    fn get_store_commit(&self, id: &CommitId) -> StoreResult<Arc<store::Commit>> {
        {
            let read_locked_cached = self.commit_cache.read().unwrap();
            if let Some(data) = read_locked_cached.get(id).cloned() {
                return Ok(data);
            }
        }
        let commit = if id == self.root_commit_id() {
            self.make_root_commit()
        } else {
            self.store.read_commit(id)?
        };
        let data = Arc::new(commit);
        let mut write_locked_cache = self.commit_cache.write().unwrap();
        write_locked_cache.insert(id.clone(), data.clone());
        Ok(data)
    }

    pub fn write_commit(&self, commit: store::Commit) -> Commit {
        let commit_id = self.store.write_commit(&commit).unwrap();
        let data = Arc::new(commit);
        {
            let mut write_locked_cache = self.commit_cache.write().unwrap();
            write_locked_cache.insert(commit_id.clone(), data.clone());
        }
        let commit = Commit::new(
            self.weak_self.as_ref().unwrap().upgrade().unwrap(),
            commit_id,
            data,
        );
        commit
    }

    pub fn get_tree(&self, dir: &DirRepoPath, id: &TreeId) -> StoreResult<Tree> {
        let data = self.get_store_tree(dir, id)?;
        Ok(Tree::new(
            self.weak_self.as_ref().unwrap().upgrade().unwrap(),
            dir.clone(),
            id.clone(),
            data,
        ))
    }

    fn get_store_tree(&self, dir: &DirRepoPath, id: &TreeId) -> StoreResult<Arc<store::Tree>> {
        let key = (dir.clone(), id.clone());
        {
            let read_locked_cache = self.tree_cache.read().unwrap();
            if let Some(data) = read_locked_cache.get(&key).cloned() {
                return Ok(data);
            }
        }
        let data = Arc::new(self.store.read_tree(dir, id)?);
        let mut write_locked_cache = self.tree_cache.write().unwrap();
        write_locked_cache.insert(key, data.clone());
        Ok(data)
    }

    pub fn write_tree(&self, path: &DirRepoPath, contents: &store::Tree) -> StoreResult<TreeId> {
        // TODO: This should also do caching like write_commit does.
        self.store.write_tree(path, contents)
    }

    pub fn read_file(&self, path: &FileRepoPath, id: &FileId) -> StoreResult<Box<dyn Read>> {
        self.store.read_file(path, id)
    }

    pub fn write_file(&self, path: &FileRepoPath, contents: &mut dyn Read) -> StoreResult<FileId> {
        self.store.write_file(path, contents)
    }

    pub fn read_symlink(&self, path: &FileRepoPath, id: &SymlinkId) -> StoreResult<String> {
        self.store.read_symlink(path, id)
    }

    pub fn write_symlink(&self, path: &FileRepoPath, contents: &str) -> StoreResult<SymlinkId> {
        self.store.write_symlink(path, contents)
    }

    pub fn read_conflict(&self, id: &ConflictId) -> StoreResult<Conflict> {
        self.store.read_conflict(id)
    }

    pub fn write_conflict(&self, contents: &Conflict) -> StoreResult<ConflictId> {
        self.store.write_conflict(contents)
    }

    pub fn tree_builder(&self, base_tree_id: TreeId) -> TreeBuilder {
        TreeBuilder::new(
            self.weak_self.as_ref().unwrap().upgrade().unwrap(),
            base_tree_id,
        )
    }
}
