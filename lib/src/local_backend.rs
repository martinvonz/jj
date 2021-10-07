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
use std::path::PathBuf;

use blake2::{Blake2b, Digest};
use protobuf::{Message, ProtobufError};
use tempfile::{NamedTempFile, PersistError};

use crate::backend::{
    Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, Conflict, ConflictId,
    ConflictPart, FileId, MillisSinceEpoch, Signature, SymlinkId, Timestamp, Tree, TreeId,
    TreeValue,
};
use crate::file_util::persist_content_addressed_temp_file;
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

impl From<ProtobufError> for BackendError {
    fn from(err: ProtobufError) -> Self {
        BackendError::Other(err.to_string())
    }
}

#[derive(Debug)]
pub struct LocalBackend {
    path: PathBuf,
    empty_tree_id: TreeId,
}

impl LocalBackend {
    pub fn init(store_path: PathBuf) -> Self {
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

    pub fn load(store_path: PathBuf) -> Self {
        let empty_tree_id = TreeId(hex::decode("786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce").unwrap());
        LocalBackend {
            path: store_path,
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
        let mut hasher = Blake2b::new();
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
        let id = FileId(hasher.finalize().to_vec());

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
        let mut hasher = Blake2b::new();
        hasher.update(&target.as_bytes());
        let id = SymlinkId(hasher.finalize().to_vec());

        persist_content_addressed_temp_file(temp_file, self.symlink_path(&id))?;
        Ok(id)
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        let path = self.tree_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;

        let proto: crate::protos::store::Tree = Message::parse_from_reader(&mut file)?;
        Ok(tree_from_proto(&proto))
    }

    fn write_tree(&self, _path: &RepoPath, tree: &Tree) -> BackendResult<TreeId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = tree_to_proto(tree);
        let mut proto_bytes: Vec<u8> = Vec::new();
        proto.write_to_writer(&mut proto_bytes)?;

        temp_file.as_file().write_all(&proto_bytes)?;

        let id = TreeId(Blake2b::digest(&proto_bytes).to_vec());

        persist_content_addressed_temp_file(temp_file, self.tree_path(&id))?;
        Ok(id)
    }

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        let path = self.commit_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;

        let proto: crate::protos::store::Commit = Message::parse_from_reader(&mut file)?;
        Ok(commit_from_proto(&proto))
    }

    fn write_commit(&self, commit: &Commit) -> BackendResult<CommitId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = commit_to_proto(commit);
        let mut proto_bytes: Vec<u8> = Vec::new();
        proto.write_to_writer(&mut proto_bytes)?;

        temp_file.as_file().write_all(&proto_bytes)?;

        let id = CommitId(Blake2b::digest(&proto_bytes).to_vec());

        persist_content_addressed_temp_file(temp_file, self.commit_path(&id))?;
        Ok(id)
    }

    fn read_conflict(&self, id: &ConflictId) -> BackendResult<Conflict> {
        let path = self.conflict_path(id);
        let mut file = File::open(path).map_err(not_found_to_backend_error)?;

        let proto: crate::protos::store::Conflict = Message::parse_from_reader(&mut file)?;
        Ok(conflict_from_proto(&proto))
    }

    fn write_conflict(&self, conflict: &Conflict) -> BackendResult<ConflictId> {
        let temp_file = NamedTempFile::new_in(&self.path)?;

        let proto = conflict_to_proto(conflict);
        let mut proto_bytes: Vec<u8> = Vec::new();
        proto.write_to_writer(&mut proto_bytes)?;

        temp_file.as_file().write_all(&proto_bytes)?;

        let id = ConflictId(Blake2b::digest(&proto_bytes).to_vec());

        persist_content_addressed_temp_file(temp_file, self.conflict_path(&id))?;
        Ok(id)
    }
}

pub fn commit_to_proto(commit: &Commit) -> crate::protos::store::Commit {
    let mut proto = crate::protos::store::Commit::new();
    for parent in &commit.parents {
        proto.parents.push(parent.0.clone());
    }
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.0.clone());
    }
    proto.set_root_tree(commit.root_tree.0.clone());
    proto.set_change_id(commit.change_id.0.clone());
    proto.set_description(commit.description.clone());
    proto.set_author(signature_to_proto(&commit.author));
    proto.set_committer(signature_to_proto(&commit.committer));
    proto.set_is_open(commit.is_open);
    proto
}

fn commit_from_proto(proto: &crate::protos::store::Commit) -> Commit {
    let commit_id_from_proto = |parent: &Vec<u8>| CommitId(parent.clone());
    let parents = proto.parents.iter().map(commit_id_from_proto).collect();
    let predecessors = proto
        .predecessors
        .iter()
        .map(commit_id_from_proto)
        .collect();
    let root_tree = TreeId(proto.root_tree.to_vec());
    let change_id = ChangeId(proto.change_id.to_vec());
    Commit {
        parents,
        predecessors,
        root_tree,
        change_id,
        description: proto.description.clone(),
        author: signature_from_proto(proto.author.get_ref()),
        committer: signature_from_proto(proto.committer.get_ref()),
        is_open: proto.is_open,
    }
}

fn tree_to_proto(tree: &Tree) -> crate::protos::store::Tree {
    let mut proto = crate::protos::store::Tree::new();
    for entry in tree.entries() {
        let mut proto_entry = crate::protos::store::Tree_Entry::new();
        proto_entry.set_name(entry.name().string());
        proto_entry.set_value(tree_value_to_proto(entry.value()));
        proto.entries.push(proto_entry);
    }
    proto
}

fn tree_from_proto(proto: &crate::protos::store::Tree) -> Tree {
    let mut tree = Tree::default();
    for proto_entry in proto.entries.iter() {
        let value = tree_value_from_proto(proto_entry.value.as_ref().unwrap());
        tree.set(RepoPathComponent::from(proto_entry.name.as_str()), value);
    }
    tree
}

fn tree_value_to_proto(value: &TreeValue) -> crate::protos::store::TreeValue {
    let mut proto = crate::protos::store::TreeValue::new();
    match value {
        TreeValue::Normal { id, executable } => {
            let mut file = crate::protos::store::TreeValue_NormalFile::new();
            file.set_id(id.0.clone());
            file.set_executable(*executable);
            proto.set_normal_file(file);
        }
        TreeValue::Symlink(id) => {
            proto.set_symlink_id(id.0.clone());
        }
        TreeValue::GitSubmodule(_id) => {
            panic!("cannot store git submodules");
        }
        TreeValue::Tree(id) => {
            proto.set_tree_id(id.0.clone());
        }
        TreeValue::Conflict(id) => {
            proto.set_conflict_id(id.0.clone());
        }
    };
    proto
}

fn tree_value_from_proto(proto: &crate::protos::store::TreeValue) -> TreeValue {
    match proto.value.as_ref().unwrap() {
        crate::protos::store::TreeValue_oneof_value::tree_id(id) => {
            TreeValue::Tree(TreeId(id.clone()))
        }
        crate::protos::store::TreeValue_oneof_value::normal_file(
            crate::protos::store::TreeValue_NormalFile { id, executable, .. },
        ) => TreeValue::Normal {
            id: FileId(id.clone()),
            executable: *executable,
        },
        crate::protos::store::TreeValue_oneof_value::symlink_id(id) => {
            TreeValue::Symlink(SymlinkId(id.clone()))
        }
        crate::protos::store::TreeValue_oneof_value::conflict_id(id) => {
            TreeValue::Conflict(ConflictId(id.clone()))
        }
    }
}

fn signature_to_proto(signature: &Signature) -> crate::protos::store::Commit_Signature {
    let mut proto = crate::protos::store::Commit_Signature::new();
    proto.set_name(signature.name.clone());
    proto.set_email(signature.email.clone());
    let mut timestamp_proto = crate::protos::store::Commit_Timestamp::new();
    timestamp_proto.set_millis_since_epoch(signature.timestamp.timestamp.0);
    timestamp_proto.set_tz_offset(signature.timestamp.tz_offset);
    proto.set_timestamp(timestamp_proto);
    proto
}

fn signature_from_proto(proto: &crate::protos::store::Commit_Signature) -> Signature {
    let timestamp = proto.get_timestamp();
    Signature {
        name: proto.name.clone(),
        email: proto.email.clone(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(timestamp.millis_since_epoch),
            tz_offset: timestamp.tz_offset,
        },
    }
}

fn conflict_to_proto(conflict: &Conflict) -> crate::protos::store::Conflict {
    let mut proto = crate::protos::store::Conflict::new();
    for part in &conflict.adds {
        proto.adds.push(conflict_part_to_proto(part));
    }
    for part in &conflict.removes {
        proto.removes.push(conflict_part_to_proto(part));
    }
    proto
}

fn conflict_from_proto(proto: &crate::protos::store::Conflict) -> Conflict {
    let mut conflict = Conflict::default();
    for part in &proto.removes {
        conflict.removes.push(conflict_part_from_proto(part))
    }
    for part in &proto.adds {
        conflict.adds.push(conflict_part_from_proto(part))
    }
    conflict
}

fn conflict_part_from_proto(proto: &crate::protos::store::Conflict_Part) -> ConflictPart {
    ConflictPart {
        value: tree_value_from_proto(proto.content.as_ref().unwrap()),
    }
}

fn conflict_part_to_proto(part: &ConflictPart) -> crate::protos::store::Conflict_Part {
    let mut proto = crate::protos::store::Conflict_Part::new();
    proto.set_content(tree_value_to_proto(&part.value));
    proto
}
