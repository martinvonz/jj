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

use std::any::Any;
use std::fmt::{Debug, Error, Formatter};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use git2::Oid;
use itertools::Itertools;
use prost::Message;

use crate::backend::{
    make_root_commit, Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, Conflict,
    ConflictId, ConflictTerm, FileId, MillisSinceEpoch, ObjectId, Signature, SymlinkId, Timestamp,
    Tree, TreeId, TreeValue,
};
use crate::repo_path::{RepoPath, RepoPathComponent};
use crate::stacked_table::{ReadonlyTable, TableSegment, TableStore};

const HASH_LENGTH: usize = 20;
const CHANGE_ID_LENGTH: usize = 16;
/// Ref namespace used only for preventing GC.
pub const NO_GC_REF_NAMESPACE: &str = "refs/jj/keep/";
const CONFLICT_SUFFIX: &str = ".jjconflict";

pub struct GitBackend {
    repo: Mutex<git2::Repository>,
    root_commit_id: CommitId,
    root_change_id: ChangeId,
    empty_tree_id: TreeId,
    extra_metadata_store: TableStore,
    cached_extra_metadata: Mutex<Option<Arc<ReadonlyTable>>>,
}

impl GitBackend {
    fn new(repo: git2::Repository, extra_metadata_store: TableStore) -> Self {
        let root_commit_id = CommitId::from_bytes(&[0; HASH_LENGTH]);
        let root_change_id = ChangeId::from_bytes(&[0; CHANGE_ID_LENGTH]);
        let empty_tree_id = TreeId::from_hex("4b825dc642cb6eb9a060e54bf8d69288fbee4904");
        GitBackend {
            repo: Mutex::new(repo),
            root_commit_id,
            root_change_id,
            empty_tree_id,
            extra_metadata_store,
            cached_extra_metadata: Mutex::new(None),
        }
    }

    pub fn init_internal(store_path: &Path) -> Self {
        let git_repo = git2::Repository::init_bare(store_path.join("git")).unwrap();
        let extra_path = store_path.join("extra");
        std::fs::create_dir(&extra_path).unwrap();
        let mut git_target_file = File::create(store_path.join("git_target")).unwrap();
        git_target_file.write_all(b"git").unwrap();
        let extra_metadata_store = TableStore::init(extra_path, HASH_LENGTH);
        GitBackend::new(git_repo, extra_metadata_store)
    }

    pub fn init_external(store_path: &Path, git_repo_path: &Path) -> Self {
        let extra_path = store_path.join("extra");
        std::fs::create_dir(&extra_path).unwrap();
        let mut git_target_file = File::create(store_path.join("git_target")).unwrap();
        git_target_file
            .write_all(git_repo_path.to_str().unwrap().as_bytes())
            .unwrap();
        let repo = git2::Repository::open(store_path.join(git_repo_path)).unwrap();
        let extra_metadata_store = TableStore::init(extra_path, HASH_LENGTH);
        GitBackend::new(repo, extra_metadata_store)
    }

    pub fn load(store_path: &Path) -> Self {
        let mut git_target_file = File::open(store_path.join("git_target")).unwrap();
        let mut buf = Vec::new();
        git_target_file.read_to_end(&mut buf).unwrap();
        let git_repo_path_str = String::from_utf8(buf).unwrap();
        let git_repo_path = store_path.join(git_repo_path_str).canonicalize().unwrap();
        let repo = git2::Repository::open(git_repo_path).unwrap();
        let extra_metadata_store = TableStore::load(store_path.join("extra"), HASH_LENGTH);
        GitBackend::new(repo, extra_metadata_store)
    }

    pub fn git_repo(&self) -> MutexGuard<'_, git2::Repository> {
        self.repo.lock().unwrap()
    }

    pub fn git_repo_clone(&self) -> git2::Repository {
        let path = self.repo.lock().unwrap().path().to_owned();
        git2::Repository::open(path).unwrap()
    }
}

fn signature_from_git(signature: git2::Signature) -> Signature {
    let name = signature.name().unwrap_or("<no name>").to_owned();
    let email = signature.email().unwrap_or("<no email>").to_owned();
    let timestamp = MillisSinceEpoch(signature.when().seconds() * 1000);
    let tz_offset = signature.when().offset_minutes();
    Signature {
        name,
        email,
        timestamp: Timestamp {
            timestamp,
            tz_offset,
        },
    }
}

fn signature_to_git(signature: &Signature) -> git2::Signature<'static> {
    let name = &signature.name;
    let email = &signature.email;
    let time = git2::Time::new(
        signature.timestamp.timestamp.0.div_euclid(1000),
        signature.timestamp.tz_offset,
    );
    git2::Signature::new(name, email, &time).unwrap()
}

fn serialize_extras(commit: &Commit) -> Vec<u8> {
    let mut proto = crate::protos::store::Commit {
        change_id: commit.change_id.to_bytes(),
        ..Default::default()
    };
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.to_bytes());
    }
    proto.encode_to_vec()
}

fn deserialize_extras(commit: &mut Commit, bytes: &[u8]) {
    let proto = crate::protos::store::Commit::decode(bytes).unwrap();
    commit.change_id = ChangeId::new(proto.change_id);
    for predecessor in &proto.predecessors {
        commit.predecessors.push(CommitId::from_bytes(predecessor));
    }
}

/// Creates a random ref in refs/jj/. Used for preventing GC of commits we
/// create.
fn create_no_gc_ref() -> String {
    let random_bytes: [u8; 16] = rand::random();
    format!("{NO_GC_REF_NAMESPACE}{}", hex::encode(random_bytes))
}

fn validate_git_object_id(id: &impl ObjectId) -> Result<git2::Oid, BackendError> {
    if id.as_bytes().len() != HASH_LENGTH {
        return Err(BackendError::InvalidHashLength {
            expected: HASH_LENGTH,
            actual: id.as_bytes().len(),
            object_type: id.object_type(),
            hash: id.hex(),
        });
    }
    let oid = git2::Oid::from_bytes(id.as_bytes()).map_err(|err| BackendError::InvalidHash {
        object_type: id.object_type(),
        hash: id.hex(),
        source: Box::new(err),
    })?;
    Ok(oid)
}

fn map_not_found_err(err: git2::Error, id: &impl ObjectId) -> BackendError {
    if err.code() == git2::ErrorCode::NotFound {
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

impl Debug for GitBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("GitStore")
            .field("path", &self.repo.lock().unwrap().path())
            .finish()
    }
}

impl Backend for GitBackend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "git"
    }

    fn commit_id_length(&self) -> usize {
        HASH_LENGTH
    }

    fn change_id_length(&self) -> usize {
        CHANGE_ID_LENGTH
    }

    fn read_file(&self, _path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        let git_blob_id = validate_git_object_id(id)?;
        let locked_repo = self.repo.lock().unwrap();
        let blob = locked_repo
            .find_blob(git_blob_id)
            .map_err(|err| map_not_found_err(err, id))?;
        let content = blob.content().to_owned();
        Ok(Box::new(Cursor::new(content)))
    }

    fn write_file(&self, _path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        let mut bytes = Vec::new();
        contents.read_to_end(&mut bytes).unwrap();
        let locked_repo = self.repo.lock().unwrap();
        let oid = locked_repo
            .blob(&bytes)
            .map_err(|err| BackendError::WriteObject {
                object_type: "file",
                source: Box::new(err),
            })?;
        Ok(FileId::new(oid.as_bytes().to_vec()))
    }

    fn read_symlink(&self, _path: &RepoPath, id: &SymlinkId) -> Result<String, BackendError> {
        let git_blob_id = validate_git_object_id(id)?;
        let locked_repo = self.repo.lock().unwrap();
        let blob = locked_repo
            .find_blob(git_blob_id)
            .map_err(|err| map_not_found_err(err, id))?;
        let target = String::from_utf8(blob.content().to_owned()).map_err(|err| {
            BackendError::InvalidUtf8 {
                object_type: id.object_type(),
                hash: id.hex(),
                source: err,
            }
        })?;
        Ok(target)
    }

    fn write_symlink(&self, _path: &RepoPath, target: &str) -> Result<SymlinkId, BackendError> {
        let locked_repo = self.repo.lock().unwrap();
        let oid = locked_repo
            .blob(target.as_bytes())
            .map_err(|err| BackendError::WriteObject {
                object_type: "symlink",
                source: Box::new(err),
            })?;
        Ok(SymlinkId::new(oid.as_bytes().to_vec()))
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
        if id == &self.empty_tree_id {
            return Ok(Tree::default());
        }
        let git_tree_id = validate_git_object_id(id)?;

        let locked_repo = self.repo.lock().unwrap();
        let git_tree = locked_repo.find_tree(git_tree_id).unwrap();
        let mut tree = Tree::default();
        for entry in git_tree.iter() {
            let name = entry.name().unwrap();
            let (name, value) = match entry.kind().unwrap() {
                git2::ObjectType::Tree => {
                    let id = TreeId::from_bytes(entry.id().as_bytes());
                    (entry.name().unwrap(), TreeValue::Tree(id))
                }
                git2::ObjectType::Blob => match entry.filemode() {
                    0o100644 => {
                        let id = FileId::from_bytes(entry.id().as_bytes());
                        if name.ends_with(CONFLICT_SUFFIX) {
                            (
                                &name[0..name.len() - CONFLICT_SUFFIX.len()],
                                TreeValue::Conflict(ConflictId::from_bytes(entry.id().as_bytes())),
                            )
                        } else {
                            (
                                name,
                                TreeValue::File {
                                    id,
                                    executable: false,
                                },
                            )
                        }
                    }
                    0o100755 => {
                        let id = FileId::from_bytes(entry.id().as_bytes());
                        (
                            name,
                            TreeValue::File {
                                id,
                                executable: true,
                            },
                        )
                    }
                    0o120000 => {
                        let id = SymlinkId::from_bytes(entry.id().as_bytes());
                        (name, TreeValue::Symlink(id))
                    }
                    mode => panic!("unexpected file mode {mode:?}"),
                },
                git2::ObjectType::Commit => {
                    let id = CommitId::from_bytes(entry.id().as_bytes());
                    (name, TreeValue::GitSubmodule(id))
                }
                kind => panic!("unexpected object type {kind:?}"),
            };
            tree.set(RepoPathComponent::from(name), value);
        }
        Ok(tree)
    }

    fn write_tree(&self, _path: &RepoPath, contents: &Tree) -> BackendResult<TreeId> {
        let locked_repo = self.repo.lock().unwrap();
        let mut builder = locked_repo.treebuilder(None).unwrap();
        for entry in contents.entries() {
            let name = entry.name().string();
            let (name, id, filemode) = match entry.value() {
                TreeValue::File {
                    id,
                    executable: false,
                } => (name, id.as_bytes(), 0o100644),
                TreeValue::File {
                    id,
                    executable: true,
                } => (name, id.as_bytes(), 0o100755),
                TreeValue::Symlink(id) => (name, id.as_bytes(), 0o120000),
                TreeValue::Tree(id) => (name, id.as_bytes(), 0o040000),
                TreeValue::GitSubmodule(id) => (name, id.as_bytes(), 0o160000),
                TreeValue::Conflict(id) => (
                    entry.name().string() + CONFLICT_SUFFIX,
                    id.as_bytes(),
                    0o100644,
                ),
            };
            builder
                .insert(name, Oid::from_bytes(id).unwrap(), filemode)
                .unwrap();
        }
        let oid = builder.write().map_err(|err| BackendError::WriteObject {
            object_type: "tree",
            source: Box::new(err),
        })?;
        Ok(TreeId::from_bytes(oid.as_bytes()))
    }

    fn read_conflict(&self, _path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        let mut file = self.read_file(
            &RepoPath::from_internal_string("unused"),
            &FileId::new(id.to_bytes()),
        )?;
        let mut data = String::new();
        file.read_to_string(&mut data)?;
        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        Ok(Conflict {
            removes: conflict_term_list_from_json(json.get("removes").unwrap()),
            adds: conflict_term_list_from_json(json.get("adds").unwrap()),
        })
    }

    fn write_conflict(&self, _path: &RepoPath, conflict: &Conflict) -> BackendResult<ConflictId> {
        let json = serde_json::json!({
            "removes": conflict_term_list_to_json(&conflict.removes),
            "adds": conflict_term_list_to_json(&conflict.adds),
        });
        let json_string = json.to_string();
        let bytes = json_string.as_bytes();
        let locked_repo = self.repo.lock().unwrap();
        let oid = locked_repo
            .blob(bytes)
            .map_err(|err| BackendError::WriteObject {
                object_type: "conflict",
                source: Box::new(err),
            })?;
        Ok(ConflictId::from_bytes(oid.as_bytes()))
    }

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if *id == self.root_commit_id {
            return Ok(make_root_commit(
                self.root_change_id().clone(),
                self.empty_tree_id.clone(),
            ));
        }
        let git_commit_id = validate_git_object_id(id)?;

        let locked_repo = self.repo.lock().unwrap();
        let commit = locked_repo
            .find_commit(git_commit_id)
            .map_err(|err| map_not_found_err(err, id))?;
        // We reverse the bits of the commit id to create the change id. We don't want
        // to use the first bytes unmodified because then it would be ambiguous
        // if a given hash prefix refers to the commit id or the change id. It
        // would have been enough to pick the last 16 bytes instead of the
        // leading 16 bytes to address that. We also reverse the bits to make it less
        // likely that users depend on any relationship between the two ids.
        let change_id = ChangeId::new(
            id.as_bytes()[4..HASH_LENGTH]
                .iter()
                .rev()
                .map(|b| b.reverse_bits())
                .collect(),
        );
        let mut parents = commit
            .parent_ids()
            .map(|oid| CommitId::from_bytes(oid.as_bytes()))
            .collect_vec();
        if parents.is_empty() {
            parents.push(self.root_commit_id.clone());
        };
        let tree_id = TreeId::from_bytes(commit.tree_id().as_bytes());
        let description = commit.message().unwrap_or("<no message>").to_owned();
        let author = signature_from_git(commit.author());
        let committer = signature_from_git(commit.committer());

        let mut commit = Commit {
            parents,
            predecessors: vec![],
            root_tree: tree_id,
            change_id,
            description,
            author,
            committer,
        };

        let table = {
            let mut locked_head = self.cached_extra_metadata.lock().unwrap();
            match locked_head.as_ref() {
                Some(head) => Ok(head.clone()),
                None => self.extra_metadata_store.get_head().map(|x| {
                    *locked_head = Some(x.clone());
                    x
                }),
            }
        }
        .map_err(|err| BackendError::Other(format!("Failed to read non-git metadata: {err}")))?;
        let maybe_extras = table.get_value(git_commit_id.as_bytes());
        if let Some(extras) = maybe_extras {
            deserialize_extras(&mut commit, extras);
        }

        Ok(commit)
    }

    fn write_commit(&self, contents: Commit) -> BackendResult<(CommitId, Commit)> {
        let locked_repo = self.repo.lock().unwrap();
        let git_tree_id = validate_git_object_id(&contents.root_tree)?;
        let git_tree = locked_repo
            .find_tree(git_tree_id)
            .map_err(|err| map_not_found_err(err, &contents.root_tree))?;
        let author = signature_to_git(&contents.author);
        let mut committer = signature_to_git(&contents.committer);
        let message = &contents.description;
        if contents.parents.is_empty() {
            return Err(BackendError::Other(
                "Cannot write a commit with no parents".to_string(),
            ));
        }
        let mut parents = vec![];
        for parent_id in &contents.parents {
            if *parent_id == self.root_commit_id {
                // Git doesn't have a root commit, so if the parent is the root commit, we don't
                // add it to the list of parents to write in the Git commit. We also check that
                // there are no other parents since Git cannot represent a merge between a root
                // commit and another commit.
                if contents.parents.len() > 1 {
                    return Err(BackendError::Other(
                        "The Git backend does not support creating merge commits with the root \
                         commit as one of the parents."
                            .to_string(),
                    ));
                }
            } else {
                let git_commit_id = validate_git_object_id(parent_id)?;
                let parent_git_commit = locked_repo
                    .find_commit(git_commit_id)
                    .map_err(|err| map_not_found_err(err, parent_id))?;
                parents.push(parent_git_commit);
            }
        }
        let parent_refs = parents.iter().collect_vec();
        let extras = serialize_extras(&contents);
        let mut mut_table = self
            .extra_metadata_store
            .get_head()
            .unwrap()
            .start_mutation();
        let id = loop {
            let git_id = locked_repo
                .commit(
                    Some(&create_no_gc_ref()),
                    &author,
                    &committer,
                    message,
                    &git_tree,
                    &parent_refs,
                )
                .map_err(|err| BackendError::WriteObject {
                    object_type: "commit",
                    source: Box::new(err),
                })?;
            let id = CommitId::from_bytes(git_id.as_bytes());
            match mut_table.get_value(id.as_bytes()) {
                Some(existing_extras) if existing_extras != extras => {
                    // It's possible a commit already exists with the same commit id but different
                    // change id. Adjust the timestamp until this is no longer the case.
                    let new_when = git2::Time::new(
                        committer.when().seconds() - 1,
                        committer.when().offset_minutes(),
                    );
                    committer = git2::Signature::new(
                        committer.name().unwrap(),
                        committer.email().unwrap(),
                        &new_when,
                    )
                    .unwrap();
                }
                _ => {
                    break id;
                }
            }
        };
        mut_table.add_entry(id.to_bytes(), extras);
        self.extra_metadata_store
            .save_table(mut_table)
            .map_err(|err| {
                BackendError::Other(format!("Failed to write non-git metadata: {err}"))
            })?;
        *self.cached_extra_metadata.lock().unwrap() = None;
        Ok((id, contents))
    }
}

fn conflict_term_list_to_json(parts: &[ConflictTerm]) -> serde_json::Value {
    serde_json::Value::Array(parts.iter().map(conflict_term_to_json).collect())
}

fn conflict_term_list_from_json(json: &serde_json::Value) -> Vec<ConflictTerm> {
    json.as_array()
        .unwrap()
        .iter()
        .map(conflict_term_from_json)
        .collect()
}

fn conflict_term_to_json(part: &ConflictTerm) -> serde_json::Value {
    serde_json::json!({
        "value": tree_value_to_json(&part.value),
    })
}

fn conflict_term_from_json(json: &serde_json::Value) -> ConflictTerm {
    let json_value = json.get("value").unwrap();
    ConflictTerm {
        value: tree_value_from_json(json_value),
    }
}

fn tree_value_to_json(value: &TreeValue) -> serde_json::Value {
    match value {
        TreeValue::File { id, executable } => serde_json::json!({
             "file": {
                 "id": id.hex(),
                 "executable": executable,
             },
        }),
        TreeValue::Symlink(id) => serde_json::json!({
             "symlink_id": id.hex(),
        }),
        TreeValue::Tree(id) => serde_json::json!({
             "tree_id": id.hex(),
        }),
        TreeValue::GitSubmodule(id) => serde_json::json!({
             "submodule_id": id.hex(),
        }),
        TreeValue::Conflict(id) => serde_json::json!({
             "conflict_id": id.hex(),
        }),
    }
}

fn tree_value_from_json(json: &serde_json::Value) -> TreeValue {
    if let Some(json_file) = json.get("file") {
        TreeValue::File {
            id: FileId::new(bytes_vec_from_json(json_file.get("id").unwrap())),
            executable: json_file.get("executable").unwrap().as_bool().unwrap(),
        }
    } else if let Some(json_id) = json.get("symlink_id") {
        TreeValue::Symlink(SymlinkId::new(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("tree_id") {
        TreeValue::Tree(TreeId::new(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("submodule_id") {
        TreeValue::GitSubmodule(CommitId::new(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("conflict_id") {
        TreeValue::Conflict(ConflictId::new(bytes_vec_from_json(json_id)))
    } else {
        panic!("unexpected json value in conflict: {json:#?}");
    }
}

fn bytes_vec_from_json(value: &serde_json::Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap()).unwrap()
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;
    use crate::backend::{FileId, MillisSinceEpoch};

    #[test]
    fn read_plain_git_commit() {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(&git_repo_path).unwrap();

        // Add a commit with some files in
        let blob1 = git_repo.blob(b"content1").unwrap();
        let blob2 = git_repo.blob(b"normal").unwrap();
        let mut dir_tree_builder = git_repo.treebuilder(None).unwrap();
        dir_tree_builder.insert("normal", blob1, 0o100644).unwrap();
        dir_tree_builder.insert("symlink", blob2, 0o120000).unwrap();
        let dir_tree_id = dir_tree_builder.write().unwrap();
        let mut root_tree_builder = git_repo.treebuilder(None).unwrap();
        root_tree_builder
            .insert("dir", dir_tree_id, 0o040000)
            .unwrap();
        let root_tree_id = root_tree_builder.write().unwrap();
        let git_author = git2::Signature::new(
            "git author",
            "git.author@example.com",
            &git2::Time::new(1000, 60),
        )
        .unwrap();
        let git_committer = git2::Signature::new(
            "git committer",
            "git.committer@example.com",
            &git2::Time::new(2000, -480),
        )
        .unwrap();
        let git_tree = git_repo.find_tree(root_tree_id).unwrap();
        let git_commit_id = git_repo
            .commit(
                None,
                &git_author,
                &git_committer,
                "git commit message",
                &git_tree,
                &[],
            )
            .unwrap();
        let commit_id = CommitId::from_hex("efdcea5ca4b3658149f899ca7feee6876d077263");
        // The change id is the leading reverse bits of the commit id
        let change_id = ChangeId::from_hex("c64ee0b6e16777fe53991f9281a6cd25");
        // Check that the git commit above got the hash we expect
        assert_eq!(git_commit_id.as_bytes(), commit_id.as_bytes());

        let store = GitBackend::init_external(store_path, &git_repo_path);
        let commit = store.read_commit(&commit_id).unwrap();
        assert_eq!(&commit.change_id, &change_id);
        assert_eq!(commit.parents, vec![CommitId::from_bytes(&[0; 20])]);
        assert_eq!(commit.predecessors, vec![]);
        assert_eq!(commit.root_tree.as_bytes(), root_tree_id.as_bytes());
        assert_eq!(commit.description, "git commit message");
        assert_eq!(commit.author.name, "git author");
        assert_eq!(commit.author.email, "git.author@example.com");
        assert_eq!(
            commit.author.timestamp.timestamp,
            MillisSinceEpoch(1000 * 1000)
        );
        assert_eq!(commit.author.timestamp.tz_offset, 60);
        assert_eq!(commit.committer.name, "git committer");
        assert_eq!(commit.committer.email, "git.committer@example.com");
        assert_eq!(
            commit.committer.timestamp.timestamp,
            MillisSinceEpoch(2000 * 1000)
        );
        assert_eq!(commit.committer.timestamp.tz_offset, -480);

        let root_tree = store
            .read_tree(
                &RepoPath::root(),
                &TreeId::from_bytes(root_tree_id.as_bytes()),
            )
            .unwrap();
        let mut root_entries = root_tree.entries();
        let dir = root_entries.next().unwrap();
        assert_eq!(root_entries.next(), None);
        assert_eq!(dir.name().as_str(), "dir");
        assert_eq!(
            dir.value(),
            &TreeValue::Tree(TreeId::from_bytes(dir_tree_id.as_bytes()))
        );

        let dir_tree = store
            .read_tree(
                &RepoPath::from_internal_string("dir"),
                &TreeId::from_bytes(dir_tree_id.as_bytes()),
            )
            .unwrap();
        let mut entries = dir_tree.entries();
        let file = entries.next().unwrap();
        let symlink = entries.next().unwrap();
        assert_eq!(entries.next(), None);
        assert_eq!(file.name().as_str(), "normal");
        assert_eq!(
            file.value(),
            &TreeValue::File {
                id: FileId::from_bytes(blob1.as_bytes()),
                executable: false
            }
        );
        assert_eq!(symlink.name().as_str(), "symlink");
        assert_eq!(
            symlink.value(),
            &TreeValue::Symlink(SymlinkId::from_bytes(blob2.as_bytes()))
        );
    }

    /// Test that parents get written correctly
    #[test]
    fn git_commit_parents() {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(&git_repo_path).unwrap();

        let backend = GitBackend::init_external(store_path, &git_repo_path);
        let mut commit = Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: backend.empty_tree_id().clone(),
            change_id: ChangeId::from_hex("abc123"),
            description: "".to_string(),
            author: create_signature(),
            committer: create_signature(),
        };

        // No parents
        commit.parents = vec![];
        assert_matches!(
            backend.write_commit(commit.clone()),
            Err(BackendError::Other(message)) if message.contains("no parents")
        );

        // Only root commit as parent
        commit.parents = vec![backend.root_commit_id().clone()];
        let first_id = backend.write_commit(commit.clone()).unwrap().0;
        let first_commit = backend.read_commit(&first_id).unwrap();
        assert_eq!(first_commit, commit);
        let first_git_commit = git_repo.find_commit(git_id(&first_id)).unwrap();
        assert_eq!(first_git_commit.parent_ids().collect_vec(), vec![]);

        // Only non-root commit as parent
        commit.parents = vec![first_id.clone()];
        let second_id = backend.write_commit(commit.clone()).unwrap().0;
        let second_commit = backend.read_commit(&second_id).unwrap();
        assert_eq!(second_commit, commit);
        let second_git_commit = git_repo.find_commit(git_id(&second_id)).unwrap();
        assert_eq!(
            second_git_commit.parent_ids().collect_vec(),
            vec![git_id(&first_id)]
        );

        // Merge commit
        commit.parents = vec![first_id.clone(), second_id.clone()];
        let merge_id = backend.write_commit(commit.clone()).unwrap().0;
        let merge_commit = backend.read_commit(&merge_id).unwrap();
        assert_eq!(merge_commit, commit);
        let merge_git_commit = git_repo.find_commit(git_id(&merge_id)).unwrap();
        assert_eq!(
            merge_git_commit.parent_ids().collect_vec(),
            vec![git_id(&first_id), git_id(&second_id)]
        );

        // Merge commit with root as one parent
        commit.parents = vec![first_id, backend.root_commit_id().clone()];
        assert_matches!(
            backend.write_commit(commit),
            Err(BackendError::Other(message)) if message.contains("root commit")
        );
    }

    #[test]
    fn commit_has_ref() {
        let temp_dir = testutils::new_temp_dir();
        let store = GitBackend::init_internal(temp_dir.path());
        let signature = Signature {
            name: "Someone".to_string(),
            email: "someone@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        };
        let commit = Commit {
            parents: vec![store.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: store.empty_tree_id().clone(),
            change_id: ChangeId::new(vec![]),
            description: "initial".to_string(),
            author: signature.clone(),
            committer: signature,
        };
        let commit_id = store.write_commit(commit).unwrap().0;
        let git_refs = store
            .git_repo()
            .references_glob("refs/jj/keep/*")
            .unwrap()
            .map(|git_ref| git_ref.unwrap().target().unwrap())
            .collect_vec();
        assert_eq!(git_refs, vec![git_id(&commit_id)]);
    }

    #[test]
    fn overlapping_git_commit_id() {
        let temp_dir = testutils::new_temp_dir();
        let store = GitBackend::init_internal(temp_dir.path());
        let commit1 = Commit {
            parents: vec![store.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: store.empty_tree_id().clone(),
            change_id: ChangeId::new(vec![]),
            description: "initial".to_string(),
            author: create_signature(),
            committer: create_signature(),
        };
        let (commit_id1, mut commit2) = store.write_commit(commit1).unwrap();
        commit2.predecessors.push(commit_id1.clone());
        // `write_commit` should prevent the ids from being the same by changing the
        // committer timestamp of the commit it actually writes.
        assert_ne!(store.write_commit(commit2).unwrap().0, commit_id1);
    }

    fn git_id(commit_id: &CommitId) -> Oid {
        Oid::from_bytes(commit_id.as_bytes()).unwrap()
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
