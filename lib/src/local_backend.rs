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

use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use blake2::{Blake2b512, Digest};
use tempfile::{NamedTempFile, PersistError};
use thrift::protocol::{TCompactInputProtocol, TCompactOutputProtocol, TSerializable};

use crate::backend::{
    make_root_commit, Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, Conflict,
    ConflictId, ConflictPart, FileId, MillisSinceEpoch, Signature, SymlinkId, Timestamp, Tree,
    TreeId, TreeValue,
};
use crate::content_hash::ContentHash;
use crate::file_util::persist_content_addressed_temp_file;
use crate::local_backend_model;
use crate::repo_path::{RepoPath, RepoPathComponent};

impl From<std::io::Error> for BackendError {
    fn from(err: std::io::Error) -> Self {
        BackendError::Other(err.to_string())
    }
}

impl From<PersistError> for BackendError {
    fn from(err: PersistError) -> Self {
        BackendError::Other(err.to_string())
    }
}

impl From<thrift::Error> for BackendError {
    fn from(err: thrift::Error) -> Self {
        BackendError::Other(err.to_string())
    }
}

#[derive(Debug)]
pub struct LocalBackend {
    path: PathBuf,
    root_commit_id: CommitId,
    empty_tree_id: TreeId,
}

impl LocalBackend {
    pub fn init(store_path: &Path) -> Self {
        fs::create_dir(store_path.join("commits")).unwrap();
        fs::create_dir(store_path.join("trees")).unwrap();
        fs::create_dir(store_path.join("files")).unwrap();
        fs::create_dir(store_path.join("symlinks")).unwrap();
        fs::create_dir(store_path.join("conflicts")).unwrap();
        let backend = Self::load(store_path);
        let empty_tree_id = backend
            .write_tree(&RepoPath::root(), &Tree::default())
            .unwrap();
        assert_eq!(empty_tree_id, backend.empty_tree_id);
        backend
    }

    pub fn load(store_path: &Path) -> Self {
        let root_commit_id = CommitId::from_bytes(&[0; 64]);
        let empty_tree_id = TreeId::from_hex("482ae5a29fbe856c7272f2071b8b0f0359ee2d89ff392b8a900643fbd0836eccd067b8bf41909e206c90d45d6e7d8b6686b93ecaee5fe1a9060d87b672101310");
        LocalBackend {
            path: store_path.to_path_buf(),
            root_commit_id,
            empty_tree_id,
        }
    }

    fn file_path(&self, id: &FileId) -> PathBuf {
        self.path.join("files").join(id.hex())
    }

    fn symlink_path(&self, id: &SymlinkId) -> PathBuf {
        self.path.join("symlinks").join(id.hex())
    }

    fn tree_path(&self, id: &TreeId) -> PathBuf {
        self.path.join("trees").join(id.hex())
    }

    fn commit_path(&self, id: &CommitId) -> PathBuf {
        self.path.join("commits").join(id.hex())
    }

    fn conflict_path(&self, id: &ConflictId) -> PathBuf {
        self.path.join("conflicts").join(id.hex())
    }
}

fn not_found_to_backend_error(err: std::io::Error) -> BackendError {
    if err.kind() == ErrorKind::NotFound {
        BackendError::NotFound
    } else {
        BackendError::from(err)
    }
}

impl Backend for LocalBackend {
    fn name(&self) -> &str {
        "local"
    }

    fn hash_length(&self) -> usize {
        64
    }

    fn git_repo(&self) -> Option<git2::Repository> {
        None
    }

    fn read_file(&self, _path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        let path = self.file_path(id);
        let file = File::open(path).map_err(not_found_to_backend_error)?;
        Ok(Box::new(zstd::Decoder::new(file)?))
    }

    fn write_file(&self, _path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;
        let mut encoder = zstd::Encoder::new(temp_file.as_file(), 0)?;
        let mut hasher = Blake2b512::new();
        loop {
            let mut buff: Vec<u8> = Vec::with_capacity(1 << 14);
            let bytes_read;
            unsafe {
                buff.set_len(1 << 14);
                bytes_read = contents.read(&mut buff)?;
                buff.set_len(bytes_read);
            }
            if bytes_read == 0 {
                break;
            }
            encoder.write_all(&buff)?;
            hasher.update(&buff);
        }
        encoder.finish()?;
        let id = FileId::new(hasher.finalize().to_vec());

        persist_content_addressed_temp_file(temp_file, self.file_path(&id))?;
        Ok(id)
    }

    fn read_symlink(&self, _path: &RepoPath, id: &SymlinkId) -> Result<String, BackendError> {
        let path = self.symlink_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;
        let mut target = String::new();
        file.read_to_string(&mut target).unwrap();
        Ok(target)
    }

    fn write_symlink(&self, _path: &RepoPath, target: &str) -> Result<SymlinkId, BackendError> {
        let mut temp_file = NamedTempFile::new_in(&self.path)?;
        temp_file.write_all(target.as_bytes())?;
        let mut hasher = Blake2b512::new();
        hasher.update(target.as_bytes());
        let id = SymlinkId::new(hasher.finalize().to_vec());

        persist_content_addressed_temp_file(temp_file, self.symlink_path(&id))?;
        Ok(id)
    }

    fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        let path = self.tree_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;
        let thrift_tree = read_thrift(&mut file).unwrap();
        Ok(tree_from_thrift(&thrift_tree))
    }

    fn write_tree(&self, _path: &RepoPath, tree: &Tree) -> BackendResult<TreeId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let thrift_tree = tree_to_thrift(tree);
        write_thrift(&thrift_tree, &mut temp_file.as_file())?;

        let id = TreeId::new(hash(tree).to_vec());
        persist_content_addressed_temp_file(temp_file, self.tree_path(&id))?;
        Ok(id)
    }

    fn read_conflict(&self, _path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        let path = self.conflict_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;
        let thrift_conflict = read_thrift(&mut file)?;
        Ok(conflict_from_thrift(&thrift_conflict))
    }

    fn write_conflict(&self, _path: &RepoPath, conflict: &Conflict) -> BackendResult<ConflictId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let thrift_conflict = conflict_to_thrift(conflict);
        write_thrift(&thrift_conflict, &mut temp_file.as_file())?;

        let id = ConflictId::new(hash(conflict).to_vec());
        persist_content_addressed_temp_file(temp_file, self.conflict_path(&id))?;
        Ok(id)
    }

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if *id == self.root_commit_id {
            return Ok(make_root_commit(self.empty_tree_id.clone()));
        }

        let path = self.commit_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;
        let thrift_commit = read_thrift(&mut file).unwrap();
        Ok(commit_from_thrift(&thrift_commit))
    }

    fn write_commit(&self, commit: &Commit) -> BackendResult<CommitId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let thrift_commit = commit_to_thrift(commit);
        write_thrift(&thrift_commit, &mut temp_file.as_file())?;

        let id = CommitId::new(hash(commit).to_vec());
        persist_content_addressed_temp_file(temp_file, self.commit_path(&id))?;
        Ok(id)
    }
}

fn read_thrift<T: TSerializable>(input: &mut impl Read) -> BackendResult<T> {
    let mut protocol = TCompactInputProtocol::new(input);
    Ok(TSerializable::read_from_in_protocol(&mut protocol).unwrap())
}

fn write_thrift<T: TSerializable>(thrift_object: &T, output: &mut impl Write) -> BackendResult<()> {
    let mut protocol = TCompactOutputProtocol::new(output);
    thrift_object.write_to_out_protocol(&mut protocol)?;
    Ok(())
}

fn commit_to_thrift(commit: &Commit) -> local_backend_model::Commit {
    let mut parents = vec![];
    for parent in &commit.parents {
        parents.push(parent.to_bytes());
    }
    let mut predecessors = vec![];
    for predecessor in &commit.predecessors {
        predecessors.push(predecessor.to_bytes());
    }
    let root_tree = commit.root_tree.to_bytes();
    let change_id = commit.change_id.to_bytes();
    let description = commit.description.clone();
    let author = signature_to_thrift(&commit.author);
    let committer = signature_to_thrift(&commit.committer);
    local_backend_model::Commit::new(
        parents,
        predecessors,
        root_tree,
        change_id,
        description,
        author,
        committer,
    )
}

fn commit_from_thrift(thrift_commit: &local_backend_model::Commit) -> Commit {
    let commit_id_from_thrift = |parent: &Vec<u8>| CommitId::new(parent.clone());
    let parents = thrift_commit
        .parents
        .iter()
        .map(commit_id_from_thrift)
        .collect();
    let predecessors = thrift_commit
        .predecessors
        .iter()
        .map(commit_id_from_thrift)
        .collect();
    let root_tree = TreeId::new(thrift_commit.root_tree.to_vec());
    let change_id = ChangeId::new(thrift_commit.change_id.to_vec());
    Commit {
        parents,
        predecessors,
        root_tree,
        change_id,
        description: thrift_commit.description.clone(),
        author: signature_from_thrift(&thrift_commit.author),
        committer: signature_from_thrift(&thrift_commit.committer),
    }
}

fn tree_to_thrift(tree: &Tree) -> local_backend_model::Tree {
    let mut entries = vec![];
    for entry in tree.entries() {
        let name = entry.name().string();
        let value = tree_value_to_thrift(entry.value());
        let thrift_entry = local_backend_model::TreeEntry::new(name, value);
        entries.push(thrift_entry);
    }
    local_backend_model::Tree::new(entries)
}

fn tree_from_thrift(thrift_tree: &local_backend_model::Tree) -> Tree {
    let mut tree = Tree::default();
    for thrift_tree_entry in &thrift_tree.entries {
        let value = tree_value_from_thrift(&thrift_tree_entry.value);
        tree.set(
            RepoPathComponent::from(thrift_tree_entry.name.as_str()),
            value,
        );
    }
    tree
}

fn tree_value_to_thrift(value: &TreeValue) -> local_backend_model::TreeValue {
    match value {
        TreeValue::File { id, executable } => {
            let file = local_backend_model::File::new(id.to_bytes(), *executable);
            local_backend_model::TreeValue::File(file)
        }
        TreeValue::Symlink(id) => local_backend_model::TreeValue::SymlinkId(id.to_bytes()),
        TreeValue::GitSubmodule(_id) => {
            panic!("cannot store git submodules");
        }
        TreeValue::Tree(id) => local_backend_model::TreeValue::TreeId(id.to_bytes()),
        TreeValue::Conflict(id) => local_backend_model::TreeValue::ConflictId(id.to_bytes()),
    }
}

fn tree_value_from_thrift(thrift_tree_value: &local_backend_model::TreeValue) -> TreeValue {
    match thrift_tree_value {
        local_backend_model::TreeValue::File(file) => TreeValue::File {
            id: FileId::from_bytes(&file.id),
            executable: file.executable,
        },
        local_backend_model::TreeValue::SymlinkId(id) => {
            TreeValue::Symlink(SymlinkId::from_bytes(id))
        }
        local_backend_model::TreeValue::TreeId(id) => TreeValue::Tree(TreeId::from_bytes(id)),
        local_backend_model::TreeValue::ConflictId(id) => {
            TreeValue::Conflict(ConflictId::from_bytes(id))
        }
    }
}

fn signature_to_thrift(signature: &Signature) -> local_backend_model::Signature {
    let timestamp = local_backend_model::Timestamp::new(
        signature.timestamp.timestamp.0,
        signature.timestamp.tz_offset,
    );
    local_backend_model::Signature::new(signature.name.clone(), signature.email.clone(), timestamp)
}

fn signature_from_thrift(thrift: &local_backend_model::Signature) -> Signature {
    let timestamp = &thrift.timestamp;
    Signature {
        name: thrift.name.clone(),
        email: thrift.email.clone(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(timestamp.millis_since_epoch),
            tz_offset: timestamp.tz_offset,
        },
    }
}

fn conflict_to_thrift(conflict: &Conflict) -> local_backend_model::Conflict {
    let mut removes = vec![];
    for part in &conflict.removes {
        removes.push(conflict_part_to_thrift(part));
    }
    let mut adds = vec![];
    for part in &conflict.adds {
        adds.push(conflict_part_to_thrift(part));
    }
    local_backend_model::Conflict::new(removes, adds)
}

fn conflict_from_thrift(thrift: &local_backend_model::Conflict) -> Conflict {
    let mut conflict = Conflict::default();
    for part in &thrift.removes {
        conflict.removes.push(conflict_part_from_thrift(part))
    }
    for part in &thrift.adds {
        conflict.adds.push(conflict_part_from_thrift(part))
    }
    conflict
}

fn conflict_part_from_thrift(thrift: &local_backend_model::ConflictPart) -> ConflictPart {
    ConflictPart {
        value: tree_value_from_thrift(&thrift.content),
    }
}

fn conflict_part_to_thrift(part: &ConflictPart) -> local_backend_model::ConflictPart {
    local_backend_model::ConflictPart::new(tree_value_to_thrift(&part.value))
}

fn hash(x: &impl ContentHash) -> digest::Output<Blake2b512> {
    let mut hasher = Blake2b512::default();
    x.hash(&mut hasher);
    hasher.finalize()
}
