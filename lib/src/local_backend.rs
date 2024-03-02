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
use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use blake2::{Blake2b512, Digest};
use prost::Message;
use tempfile::NamedTempFile;

use crate::backend::{
    make_root_commit, Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, Conflict,
    ConflictId, ConflictTerm, FileId, MergedTreeId, MillisSinceEpoch, SecureSig, Signature,
    SigningFn, SymlinkId, Timestamp, Tree, TreeId, TreeValue,
};
use crate::content_hash::blake2b_hash;
use crate::file_util::persist_content_addressed_temp_file;
use crate::index::Index;
use crate::merge::MergeBuilder;
use crate::object_id::ObjectId;
use crate::repo_path::{RepoPath, RepoPathComponentBuf};

const COMMIT_ID_LENGTH: usize = 64;
const CHANGE_ID_LENGTH: usize = 16;

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

fn to_other_err(err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> BackendError {
    BackendError::Other(err.into())
}

#[derive(Debug)]
pub struct LocalBackend {
    path: PathBuf,
    root_commit_id: CommitId,
    root_change_id: ChangeId,
    empty_tree_id: TreeId,
}

impl LocalBackend {
    pub fn name() -> &'static str {
        "local"
    }

    pub fn init(store_path: &Path) -> Self {
        fs::create_dir(store_path.join("commits")).unwrap();
        fs::create_dir(store_path.join("trees")).unwrap();
        fs::create_dir(store_path.join("files")).unwrap();
        fs::create_dir(store_path.join("symlinks")).unwrap();
        fs::create_dir(store_path.join("conflicts")).unwrap();
        let backend = Self::load(store_path);
        let empty_tree_id = backend
            .write_tree(RepoPath::root(), &Tree::default())
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

#[async_trait]
impl Backend for LocalBackend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        Self::name()
    }

    fn commit_id_length(&self) -> usize {
        COMMIT_ID_LENGTH
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

    fn concurrency(&self) -> usize {
        1
    }

    async fn read_file(&self, _path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        let path = self.file_path(id);
        let file = File::open(path).map_err(|err| map_not_found_err(err, id))?;
        Ok(Box::new(zstd::Decoder::new(file).map_err(to_other_err)?))
    }

    fn write_file(&self, _path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        let temp_file = NamedTempFile::new_in(&self.path).map_err(to_other_err)?;
        let mut encoder = zstd::Encoder::new(temp_file.as_file(), 0).map_err(to_other_err)?;
        let mut hasher = Blake2b512::new();
        let mut buff: Vec<u8> = vec![0; 1 << 14];
        loop {
            let bytes_read = contents.read(&mut buff).map_err(to_other_err)?;
            if bytes_read == 0 {
                break;
            }
            let bytes = &buff[..bytes_read];
            encoder.write_all(bytes).map_err(to_other_err)?;
            hasher.update(bytes);
        }
        encoder.finish().map_err(to_other_err)?;
        let id = FileId::new(hasher.finalize().to_vec());

        persist_content_addressed_temp_file(temp_file, self.file_path(&id))
            .map_err(to_other_err)?;
        Ok(id)
    }

    async fn read_symlink(&self, _path: &RepoPath, id: &SymlinkId) -> Result<String, BackendError> {
        let path = self.symlink_path(id);
        let target = fs::read_to_string(path).map_err(|err| map_not_found_err(err, id))?;
        Ok(target)
    }

    fn write_symlink(&self, _path: &RepoPath, target: &str) -> Result<SymlinkId, BackendError> {
        let mut temp_file = NamedTempFile::new_in(&self.path).map_err(to_other_err)?;
        temp_file
            .write_all(target.as_bytes())
            .map_err(to_other_err)?;
        let mut hasher = Blake2b512::new();
        hasher.update(target.as_bytes());
        let id = SymlinkId::new(hasher.finalize().to_vec());

        persist_content_addressed_temp_file(temp_file, self.symlink_path(&id))
            .map_err(to_other_err)?;
        Ok(id)
    }

    async fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        let path = self.tree_path(id);
        let buf = fs::read(path).map_err(|err| map_not_found_err(err, id))?;

        let proto = crate::protos::local_store::Tree::decode(&*buf).map_err(to_other_err)?;
        Ok(tree_from_proto(proto))
    }

    fn write_tree(&self, _path: &RepoPath, tree: &Tree) -> BackendResult<TreeId> {
        let temp_file = NamedTempFile::new_in(&self.path).map_err(to_other_err)?;

        let proto = tree_to_proto(tree);
        temp_file
            .as_file()
            .write_all(&proto.encode_to_vec())
            .map_err(to_other_err)?;

        let id = TreeId::new(blake2b_hash(tree).to_vec());

        persist_content_addressed_temp_file(temp_file, self.tree_path(&id))
            .map_err(to_other_err)?;
        Ok(id)
    }

    fn read_conflict(&self, _path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        let path = self.conflict_path(id);
        let buf = fs::read(path).map_err(|err| map_not_found_err(err, id))?;

        let proto = crate::protos::local_store::Conflict::decode(&*buf).map_err(to_other_err)?;
        Ok(conflict_from_proto(proto))
    }

    fn write_conflict(&self, _path: &RepoPath, conflict: &Conflict) -> BackendResult<ConflictId> {
        let temp_file = NamedTempFile::new_in(&self.path).map_err(to_other_err)?;

        let proto = conflict_to_proto(conflict);
        temp_file
            .as_file()
            .write_all(&proto.encode_to_vec())
            .map_err(to_other_err)?;

        let id = ConflictId::new(blake2b_hash(conflict).to_vec());

        persist_content_addressed_temp_file(temp_file, self.conflict_path(&id))
            .map_err(to_other_err)?;
        Ok(id)
    }

    async fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if *id == self.root_commit_id {
            return Ok(make_root_commit(
                self.root_change_id().clone(),
                self.empty_tree_id.clone(),
            ));
        }

        let path = self.commit_path(id);
        let buf = fs::read(path).map_err(|err| map_not_found_err(err, id))?;

        let proto = crate::protos::local_store::Commit::decode(&*buf).map_err(to_other_err)?;
        Ok(commit_from_proto(proto))
    }

    fn write_commit(
        &self,
        mut commit: Commit,
        sign_with: Option<&mut SigningFn>,
    ) -> BackendResult<(CommitId, Commit)> {
        assert!(commit.secure_sig.is_none(), "commit.secure_sig was set");

        if commit.parents.is_empty() {
            return Err(BackendError::Other(
                "Cannot write a commit with no parents".into(),
            ));
        }
        let temp_file = NamedTempFile::new_in(&self.path).map_err(to_other_err)?;

        let mut proto = commit_to_proto(&commit);
        if let Some(sign) = sign_with {
            let data = proto.encode_to_vec();
            let sig = sign(&data).map_err(to_other_err)?;
            proto.secure_sig = Some(sig.clone());
            commit.secure_sig = Some(SecureSig { data, sig });
        }

        temp_file
            .as_file()
            .write_all(&proto.encode_to_vec())
            .map_err(to_other_err)?;

        let id = CommitId::new(blake2b_hash(&commit).to_vec());

        persist_content_addressed_temp_file(temp_file, self.commit_path(&id))
            .map_err(to_other_err)?;
        Ok((id, commit))
    }

    fn gc(&self, _index: &dyn Index, _keep_newer: SystemTime) -> BackendResult<()> {
        Ok(())
    }
}

pub fn commit_to_proto(commit: &Commit) -> crate::protos::local_store::Commit {
    let mut proto = crate::protos::local_store::Commit::default();
    for parent in &commit.parents {
        proto.parents.push(parent.to_bytes());
    }
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.to_bytes());
    }
    match &commit.root_tree {
        MergedTreeId::Legacy(tree_id) => {
            proto.root_tree = vec![tree_id.to_bytes()];
        }
        MergedTreeId::Merge(tree_ids) => {
            proto.uses_tree_conflict_format = true;
            proto.root_tree = tree_ids.iter().map(|id| id.to_bytes()).collect();
        }
    }
    proto.change_id = commit.change_id.to_bytes();
    proto.description = commit.description.clone();
    proto.author = Some(signature_to_proto(&commit.author));
    proto.committer = Some(signature_to_proto(&commit.committer));
    proto
}

fn commit_from_proto(mut proto: crate::protos::local_store::Commit) -> Commit {
    // Note how .take() sets the secure_sig field to None before we encode the data.
    // Needs to be done first since proto is partially moved a bunch below
    let secure_sig = proto.secure_sig.take().map(|sig| SecureSig {
        data: proto.encode_to_vec(),
        sig,
    });

    let parents = proto.parents.into_iter().map(CommitId::new).collect();
    let predecessors = proto.predecessors.into_iter().map(CommitId::new).collect();
    let root_tree = if proto.uses_tree_conflict_format {
        let merge_builder: MergeBuilder<_> = proto.root_tree.into_iter().map(TreeId::new).collect();
        MergedTreeId::Merge(merge_builder.build())
    } else {
        assert_eq!(proto.root_tree.len(), 1);
        MergedTreeId::Legacy(TreeId::new(proto.root_tree[0].to_vec()))
    };
    let change_id = ChangeId::new(proto.change_id);
    Commit {
        parents,
        predecessors,
        root_tree,
        change_id,
        description: proto.description,
        author: signature_from_proto(proto.author.unwrap_or_default()),
        committer: signature_from_proto(proto.committer.unwrap_or_default()),
        secure_sig,
    }
}

fn tree_to_proto(tree: &Tree) -> crate::protos::local_store::Tree {
    let mut proto = crate::protos::local_store::Tree::default();
    for entry in tree.entries() {
        proto.entries.push(crate::protos::local_store::tree::Entry {
            name: entry.name().as_str().to_owned(),
            value: Some(tree_value_to_proto(entry.value())),
        });
    }
    proto
}

fn tree_from_proto(proto: crate::protos::local_store::Tree) -> Tree {
    let mut tree = Tree::default();
    for proto_entry in proto.entries {
        let value = tree_value_from_proto(proto_entry.value.unwrap());
        tree.set(RepoPathComponentBuf::from(proto_entry.name), value);
    }
    tree
}

fn tree_value_to_proto(value: &TreeValue) -> crate::protos::local_store::TreeValue {
    let mut proto = crate::protos::local_store::TreeValue::default();
    match value {
        TreeValue::File { id, executable } => {
            proto.value = Some(crate::protos::local_store::tree_value::Value::File(
                crate::protos::local_store::tree_value::File {
                    id: id.to_bytes(),
                    executable: *executable,
                },
            ));
        }
        TreeValue::Symlink(id) => {
            proto.value = Some(crate::protos::local_store::tree_value::Value::SymlinkId(
                id.to_bytes(),
            ));
        }
        TreeValue::GitSubmodule(_id) => {
            panic!("cannot store git submodules");
        }
        TreeValue::Tree(id) => {
            proto.value = Some(crate::protos::local_store::tree_value::Value::TreeId(
                id.to_bytes(),
            ));
        }
        TreeValue::Conflict(id) => {
            proto.value = Some(crate::protos::local_store::tree_value::Value::ConflictId(
                id.to_bytes(),
            ));
        }
    }
    proto
}

fn tree_value_from_proto(proto: crate::protos::local_store::TreeValue) -> TreeValue {
    match proto.value.unwrap() {
        crate::protos::local_store::tree_value::Value::TreeId(id) => {
            TreeValue::Tree(TreeId::new(id))
        }
        crate::protos::local_store::tree_value::Value::File(
            crate::protos::local_store::tree_value::File { id, executable, .. },
        ) => TreeValue::File {
            id: FileId::new(id),
            executable,
        },
        crate::protos::local_store::tree_value::Value::SymlinkId(id) => {
            TreeValue::Symlink(SymlinkId::new(id))
        }
        crate::protos::local_store::tree_value::Value::ConflictId(id) => {
            TreeValue::Conflict(ConflictId::new(id))
        }
    }
}

fn signature_to_proto(signature: &Signature) -> crate::protos::local_store::commit::Signature {
    crate::protos::local_store::commit::Signature {
        name: signature.name.clone(),
        email: signature.email.clone(),
        timestamp: Some(crate::protos::local_store::commit::Timestamp {
            millis_since_epoch: signature.timestamp.timestamp.0,
            tz_offset: signature.timestamp.tz_offset,
        }),
    }
}

fn signature_from_proto(proto: crate::protos::local_store::commit::Signature) -> Signature {
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

fn conflict_to_proto(conflict: &Conflict) -> crate::protos::local_store::Conflict {
    let mut proto = crate::protos::local_store::Conflict::default();
    for term in &conflict.removes {
        proto.removes.push(conflict_term_to_proto(term));
    }
    for term in &conflict.adds {
        proto.adds.push(conflict_term_to_proto(term));
    }
    proto
}

fn conflict_from_proto(proto: crate::protos::local_store::Conflict) -> Conflict {
    let mut conflict = Conflict::default();
    for term in proto.removes {
        conflict.removes.push(conflict_term_from_proto(term))
    }
    for term in proto.adds {
        conflict.adds.push(conflict_term_from_proto(term))
    }
    conflict
}

fn conflict_term_from_proto(proto: crate::protos::local_store::conflict::Term) -> ConflictTerm {
    ConflictTerm {
        value: tree_value_from_proto(proto.content.unwrap()),
    }
}

fn conflict_term_to_proto(part: &ConflictTerm) -> crate::protos::local_store::conflict::Term {
    crate::protos::local_store::conflict::Term {
        content: Some(tree_value_to_proto(&part.value)),
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pollster::FutureExt;

    use super::*;

    /// Test that parents get written correctly
    #[test]
    fn write_commit_parents() {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();

        let backend = LocalBackend::init(store_path);
        let mut commit = Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: MergedTreeId::resolved(backend.empty_tree_id().clone()),
            change_id: ChangeId::from_hex("abc123"),
            description: "".to_string(),
            author: create_signature(),
            committer: create_signature(),
            secure_sig: None,
        };

        // No parents
        commit.parents = vec![];
        assert_matches!(
            backend.write_commit(commit.clone(), None),
            Err(BackendError::Other(err)) if err.to_string().contains("no parents")
        );

        // Only root commit as parent
        commit.parents = vec![backend.root_commit_id().clone()];
        let first_id = backend.write_commit(commit.clone(), None).unwrap().0;
        let first_commit = backend.read_commit(&first_id).block_on().unwrap();
        assert_eq!(first_commit, commit);

        // Only non-root commit as parent
        commit.parents = vec![first_id.clone()];
        let second_id = backend.write_commit(commit.clone(), None).unwrap().0;
        let second_commit = backend.read_commit(&second_id).block_on().unwrap();
        assert_eq!(second_commit, commit);

        // Merge commit
        commit.parents = vec![first_id.clone(), second_id.clone()];
        let merge_id = backend.write_commit(commit.clone(), None).unwrap().0;
        let merge_commit = backend.read_commit(&merge_id).block_on().unwrap();
        assert_eq!(merge_commit, commit);

        // Merge commit with root as one parent
        commit.parents = vec![first_id, backend.root_commit_id().clone()];
        let root_merge_id = backend.write_commit(commit.clone(), None).unwrap().0;
        let root_merge_commit = backend.read_commit(&root_merge_id).block_on().unwrap();
        assert_eq!(root_merge_commit, commit);
    }

    fn create_signature() -> Signature {
        Signature {
            name: "Someone".to_string(),
            email: "someone@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        }
    }
}
