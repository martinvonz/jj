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

use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use blake2::{Blake2b512, Digest};
use prost::Message;
use tempfile::{NamedTempFile, PersistError};

use crate::backend::{
    make_root_commit, Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, Conflict,
    ConflictId, ConflictTerm, FileId, MillisSinceEpoch, ObjectId, Signature, SymlinkId, Timestamp,
    Tree, TreeId, TreeValue,
};
use crate::content_hash::blake2b_hash;
use crate::file_util::persist_content_addressed_temp_file;
use crate::repo_path::{RepoPath, RepoPathComponent};
use crate::signer::Signer;

const COMMIT_ID_LENGTH: usize = 64;
const CHANGE_ID_LENGTH: usize = 16;

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

impl From<prost::DecodeError> for BackendError {
    fn from(err: prost::DecodeError) -> Self {
        BackendError::Other(err.to_string())
    }
}

fn map_not_found_err(err: std::io::Error, id: &impl ObjectId) -> BackendError {
    if err.kind() == std::io::ErrorKind::NotFound {
        BackendError::ObjectNotFound {
            object_type: id.object_type(),
            hash: id.hex(),
            source: Box::new(err),
        }
    } else {
        BackendError::ReadObject {
            object_type: id.object_type(),
            hash: id.hex(),
            source: Box::new(err),
        }
    }
}

#[derive(Debug)]
pub struct LocalBackend {
    path: PathBuf,
    root_commit_id: CommitId,
    root_change_id: ChangeId,
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
        let root_commit_id = CommitId::from_bytes(&[0; COMMIT_ID_LENGTH]);
        let root_change_id = ChangeId::from_bytes(&[0; CHANGE_ID_LENGTH]);
        let empty_tree_id = TreeId::from_hex("482ae5a29fbe856c7272f2071b8b0f0359ee2d89ff392b8a900643fbd0836eccd067b8bf41909e206c90d45d6e7d8b6686b93ecaee5fe1a9060d87b672101310");
        LocalBackend {
            path: store_path.to_path_buf(),
            root_commit_id,
            root_change_id,
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

impl Backend for LocalBackend {
    fn name(&self) -> &str {
        "local"
    }

    fn commit_id_length(&self) -> usize {
        COMMIT_ID_LENGTH
    }

    fn change_id_length(&self) -> usize {
        CHANGE_ID_LENGTH
    }

    fn git_repo(&self) -> Option<git2::Repository> {
        None
    }

    fn read_file(&self, _path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        let path = self.file_path(id);
        let file = File::open(path).map_err(|err| map_not_found_err(err, id))?;
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
        let mut file = File::open(path).map_err(|err| map_not_found_err(err, id))?;
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

    fn root_change_id(&self) -> &ChangeId {
        &self.root_change_id
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        let path = self.tree_path(id);
        let buf = fs::read(path).map_err(|err| map_not_found_err(err, id))?;

        let proto = crate::protos::store::Tree::decode(&*buf)?;
        Ok(tree_from_proto(proto))
    }

    fn write_tree(&self, _path: &RepoPath, tree: &Tree) -> BackendResult<TreeId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = tree_to_proto(tree);
        temp_file.as_file().write_all(&proto.encode_to_vec())?;

        let id = TreeId::new(blake2b_hash(tree).to_vec());

        persist_content_addressed_temp_file(temp_file, self.tree_path(&id))?;
        Ok(id)
    }

    fn read_conflict(&self, _path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        let path = self.conflict_path(id);
        let buf = fs::read(path).map_err(|err| map_not_found_err(err, id))?;

        let proto = crate::protos::store::Conflict::decode(&*buf)?;
        Ok(conflict_from_proto(proto))
    }

    fn write_conflict(&self, _path: &RepoPath, conflict: &Conflict) -> BackendResult<ConflictId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = conflict_to_proto(conflict);
        temp_file.as_file().write_all(&proto.encode_to_vec())?;

        let id = ConflictId::new(blake2b_hash(conflict).to_vec());

        persist_content_addressed_temp_file(temp_file, self.conflict_path(&id))?;
        Ok(id)
    }

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if *id == self.root_commit_id {
            return Ok(make_root_commit(
                self.root_change_id().clone(),
                self.empty_tree_id.clone(),
            ));
        }

        let path = self.commit_path(id);
        let buf = fs::read(path).map_err(|err| map_not_found_err(err, id))?;

        let proto = crate::protos::store::Commit::decode(&*buf)?;
        Ok(commit_from_proto(proto))
    }

    fn write_commit(
        &self,
        commit: &Commit,
        _signer: Option<&dyn Signer>,
    ) -> BackendResult<CommitId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = commit_to_proto(commit);
        temp_file.as_file().write_all(&proto.encode_to_vec())?;

        let id = CommitId::new(blake2b_hash(commit).to_vec());

        persist_content_addressed_temp_file(temp_file, self.commit_path(&id))?;
        Ok(id)
    }
}

pub fn commit_to_proto(commit: &Commit) -> crate::protos::store::Commit {
    let mut proto = crate::protos::store::Commit::default();
    for parent in &commit.parents {
        proto.parents.push(parent.to_bytes());
    }
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.to_bytes());
    }
    proto.root_tree = commit.root_tree.to_bytes();
    proto.change_id = commit.change_id.to_bytes();
    proto.description = commit.description.clone();
    proto.author = Some(signature_to_proto(&commit.author));
    proto.committer = Some(signature_to_proto(&commit.committer));
    proto
}

fn commit_from_proto(proto: crate::protos::store::Commit) -> Commit {
    let parents = proto.parents.into_iter().map(CommitId::new).collect();
    let predecessors = proto.predecessors.into_iter().map(CommitId::new).collect();
    let root_tree = TreeId::new(proto.root_tree);
    let change_id = ChangeId::new(proto.change_id);
    Commit {
        parents,
        predecessors,
        root_tree,
        change_id,
        description: proto.description,
        author: signature_from_proto(proto.author.unwrap_or_default()),
        committer: signature_from_proto(proto.committer.unwrap_or_default()),
        sig: None, // todo: implement signature storage for native backend
    }
}

fn tree_to_proto(tree: &Tree) -> crate::protos::store::Tree {
    let mut proto = crate::protos::store::Tree::default();
    for entry in tree.entries() {
        proto.entries.push(crate::protos::store::tree::Entry {
            name: entry.name().string(),
            value: Some(tree_value_to_proto(entry.value())),
        });
    }
    proto
}

fn tree_from_proto(proto: crate::protos::store::Tree) -> Tree {
    let mut tree = Tree::default();
    for proto_entry in proto.entries {
        let value = tree_value_from_proto(proto_entry.value.unwrap());
        tree.set(RepoPathComponent::from(proto_entry.name), value);
    }
    tree
}

fn tree_value_to_proto(value: &TreeValue) -> crate::protos::store::TreeValue {
    let mut proto = crate::protos::store::TreeValue::default();
    match value {
        TreeValue::File { id, executable } => {
            proto.value = Some(crate::protos::store::tree_value::Value::File(
                crate::protos::store::tree_value::File {
                    id: id.to_bytes(),
                    executable: *executable,
                },
            ));
        }
        TreeValue::Symlink(id) => {
            proto.value = Some(crate::protos::store::tree_value::Value::SymlinkId(
                id.to_bytes(),
            ));
        }
        TreeValue::GitSubmodule(_id) => {
            panic!("cannot store git submodules");
        }
        TreeValue::Tree(id) => {
            proto.value = Some(crate::protos::store::tree_value::Value::TreeId(
                id.to_bytes(),
            ));
        }
        TreeValue::Conflict(id) => {
            proto.value = Some(crate::protos::store::tree_value::Value::ConflictId(
                id.to_bytes(),
            ));
        }
    }
    proto
}

fn tree_value_from_proto(proto: crate::protos::store::TreeValue) -> TreeValue {
    match proto.value.unwrap() {
        crate::protos::store::tree_value::Value::TreeId(id) => TreeValue::Tree(TreeId::new(id)),
        crate::protos::store::tree_value::Value::File(crate::protos::store::tree_value::File {
            id,
            executable,
            ..
        }) => TreeValue::File {
            id: FileId::new(id),
            executable,
        },
        crate::protos::store::tree_value::Value::SymlinkId(id) => {
            TreeValue::Symlink(SymlinkId::new(id))
        }
        crate::protos::store::tree_value::Value::ConflictId(id) => {
            TreeValue::Conflict(ConflictId::new(id))
        }
    }
}

fn signature_to_proto(signature: &Signature) -> crate::protos::store::commit::Signature {
    crate::protos::store::commit::Signature {
        name: signature.name.clone(),
        email: signature.email.clone(),
        timestamp: Some(crate::protos::store::commit::Timestamp {
            millis_since_epoch: signature.timestamp.timestamp.0,
            tz_offset: signature.timestamp.tz_offset,
        }),
    }
}

fn signature_from_proto(proto: crate::protos::store::commit::Signature) -> Signature {
    let timestamp = proto.timestamp.unwrap_or_default();
    Signature {
        name: proto.name,
        email: proto.email,
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(timestamp.millis_since_epoch),
            tz_offset: timestamp.tz_offset,
        },
    }
}

fn conflict_to_proto(conflict: &Conflict) -> crate::protos::store::Conflict {
    let mut proto = crate::protos::store::Conflict::default();
    for term in &conflict.adds {
        proto.adds.push(conflict_term_to_proto(term));
    }
    for term in &conflict.removes {
        proto.removes.push(conflict_term_to_proto(term));
    }
    proto
}

fn conflict_from_proto(proto: crate::protos::store::Conflict) -> Conflict {
    let mut conflict = Conflict::default();
    for term in proto.removes {
        conflict.removes.push(conflict_term_from_proto(term))
    }
    for term in proto.adds {
        conflict.adds.push(conflict_term_from_proto(term))
    }
    conflict
}

fn conflict_term_from_proto(proto: crate::protos::store::conflict::Term) -> ConflictTerm {
    ConflictTerm {
        value: tree_value_from_proto(proto.content.unwrap()),
    }
}

fn conflict_term_to_proto(part: &ConflictTerm) -> crate::protos::store::conflict::Term {
    crate::protos::store::conflict::Term {
        content: Some(tree_value_to_proto(&part.value)),
    }
}
