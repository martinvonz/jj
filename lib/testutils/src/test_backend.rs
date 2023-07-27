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

use std::any::Any;
use std::collections::HashMap;
use std::fmt::{Debug, Error, Formatter};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use jj_lib::backend::{
    make_root_commit, Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, Conflict,
    ConflictId, FileId, ObjectId, SymlinkId, Tree, TreeId,
};
use jj_lib::repo_path::RepoPath;

const HASH_LENGTH: usize = 10;
const CHANGE_ID_LENGTH: usize = 16;

static BACKEND_DATA: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<TestBackendData>>>>> =
    OnceLock::new();

fn backend_data() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<TestBackendData>>>> {
    BACKEND_DATA.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Default)]
pub struct TestBackendData {
    commits: HashMap<CommitId, Commit>,
    trees: HashMap<RepoPath, HashMap<TreeId, Tree>>,
    files: HashMap<RepoPath, HashMap<FileId, Vec<u8>>>,
    symlinks: HashMap<RepoPath, HashMap<SymlinkId, String>>,
    conflicts: HashMap<RepoPath, HashMap<ConflictId, Conflict>>,
}

fn get_hash(content: &(impl jj_lib::content_hash::ContentHash + ?Sized)) -> Vec<u8> {
    jj_lib::content_hash::blake2b_hash(content).as_slice()[..HASH_LENGTH].to_vec()
}

/// A commit backend for use in tests. It's meant to be strict, in order to
/// catch bugs where we make the wrong assumptions. For example, unlike both
/// `GitBackend` and `LocalBackend`, this backend doesn't share objects written
/// to different paths (writing a file with contents X to path A will not make
/// it possible to read that contents from path B given the same `FileId`).
pub struct TestBackend {
    root_commit_id: CommitId,
    root_change_id: ChangeId,
    empty_tree_id: TreeId,
    data: Arc<Mutex<TestBackendData>>,
}

impl TestBackend {
    pub fn init(store_path: &Path) -> Self {
        let root_commit_id = CommitId::from_bytes(&[0; HASH_LENGTH]);
        let root_change_id = ChangeId::from_bytes(&[0; CHANGE_ID_LENGTH]);
        let empty_tree_id = TreeId::new(get_hash(&Tree::default()));
        let data = Arc::new(Mutex::new(TestBackendData::default()));
        backend_data()
            .lock()
            .unwrap()
            .insert(store_path.to_path_buf(), data.clone());
        TestBackend {
            root_commit_id,
            root_change_id,
            empty_tree_id,
            data,
        }
    }

    pub fn load(store_path: &Path) -> Self {
        let data = backend_data()
            .lock()
            .unwrap()
            .get(store_path)
            .unwrap()
            .clone();
        let root_commit_id = CommitId::from_bytes(&[0; HASH_LENGTH]);
        let root_change_id = ChangeId::from_bytes(&[0; CHANGE_ID_LENGTH]);
        let empty_tree_id = TreeId::new(get_hash(&Tree::default()));
        TestBackend {
            root_commit_id,
            root_change_id,
            empty_tree_id,
            data,
        }
    }

    fn locked_data(&self) -> MutexGuard<'_, TestBackendData> {
        self.data.lock().unwrap()
    }
}

impl Debug for TestBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("TestBackend").finish_non_exhaustive()
    }
}

impl Backend for TestBackend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "test"
    }

    fn commit_id_length(&self) -> usize {
        HASH_LENGTH
    }

    fn change_id_length(&self) -> usize {
        CHANGE_ID_LENGTH
    }

    fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    fn root_change_id(&self) -> &ChangeId {
        &self.root_change_id
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn read_file(&self, path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        match self
            .locked_data()
            .files
            .get(path)
            .and_then(|items| items.get(id))
            .cloned()
        {
            None => Err(BackendError::ObjectNotFound {
                object_type: "file".to_string(),
                hash: id.hex(),
                source: format!("at path {path:?}").into(),
            }),
            Some(contents) => Ok(Box::new(Cursor::new(contents))),
        }
    }

    fn write_file(&self, path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        let mut bytes = Vec::new();
        contents.read_to_end(&mut bytes).unwrap();
        let id = FileId::new(get_hash(&bytes));
        self.locked_data()
            .files
            .entry(path.clone())
            .or_default()
            .insert(id.clone(), bytes);
        Ok(id)
    }

    fn read_symlink(&self, path: &RepoPath, id: &SymlinkId) -> Result<String, BackendError> {
        match self
            .locked_data()
            .symlinks
            .get(path)
            .and_then(|items| items.get(id))
            .cloned()
        {
            None => Err(BackendError::ObjectNotFound {
                object_type: "symlink".to_string(),
                hash: id.hex(),
                source: format!("at path {path:?}").into(),
            }),
            Some(target) => Ok(target),
        }
    }

    fn write_symlink(&self, path: &RepoPath, target: &str) -> Result<SymlinkId, BackendError> {
        let id = SymlinkId::new(get_hash(target.as_bytes()));
        self.locked_data()
            .symlinks
            .entry(path.clone())
            .or_default()
            .insert(id.clone(), target.to_string());
        Ok(id)
    }

    fn read_tree(&self, path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        if id == &self.empty_tree_id {
            return Ok(Tree::default());
        }
        match self
            .locked_data()
            .trees
            .get(path)
            .and_then(|items| items.get(id))
            .cloned()
        {
            None => Err(BackendError::ObjectNotFound {
                object_type: "tree".to_string(),
                hash: id.hex(),
                source: format!("at path {path:?}").into(),
            }),
            Some(tree) => Ok(tree),
        }
    }

    fn write_tree(&self, path: &RepoPath, contents: &Tree) -> BackendResult<TreeId> {
        let id = TreeId::new(get_hash(contents));
        self.locked_data()
            .trees
            .entry(path.clone())
            .or_default()
            .insert(id.clone(), contents.clone());
        Ok(id)
    }

    fn read_conflict(&self, path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        match self
            .locked_data()
            .conflicts
            .get(path)
            .and_then(|items| items.get(id))
            .cloned()
        {
            None => Err(BackendError::ObjectNotFound {
                object_type: "conflict".to_string(),
                hash: id.hex(),
                source: format!("at path {path:?}").into(),
            }),
            Some(conflict) => Ok(conflict),
        }
    }

    fn write_conflict(&self, path: &RepoPath, contents: &Conflict) -> BackendResult<ConflictId> {
        let id = ConflictId::new(get_hash(contents));
        self.locked_data()
            .conflicts
            .entry(path.clone())
            .or_default()
            .insert(id.clone(), contents.clone());
        Ok(id)
    }

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if id == &self.root_commit_id {
            return Ok(make_root_commit(
                self.root_change_id.clone(),
                self.empty_tree_id.clone(),
            ));
        }
        match self.locked_data().commits.get(id).cloned() {
            None => Err(BackendError::ObjectNotFound {
                object_type: "commit".to_string(),
                hash: id.hex(),
                source: "".into(),
            }),
            Some(commit) => Ok(commit),
        }
    }

    fn write_commit(&self, contents: Commit) -> BackendResult<(CommitId, Commit)> {
        let id = CommitId::new(get_hash(&contents));
        self.locked_data()
            .commits
            .insert(id.clone(), contents.clone());
        Ok((id, contents))
    }
}
