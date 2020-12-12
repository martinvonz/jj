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

use std::fmt::{Debug, Error, Formatter};
use std::io::Cursor;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use git2::Oid;
use protobuf::Message;

use crate::repo_path::{DirRepoPath, FileRepoPath};
use crate::store::{
    ChangeId, Commit, CommitId, Conflict, ConflictId, ConflictPart, FileId, MillisSinceEpoch,
    Signature, Store, StoreError, StoreResult, SymlinkId, Timestamp, Tree, TreeId, TreeValue,
};

const NOTES_REF: &str = "refs/notes/jj/commits";
const NOTES_REF_LOCK: &str = "refs/notes/jj/commits.lock";
const CONFLICT_SUFFIX: &str = ".jjconflict";

impl From<git2::Error> for StoreError {
    fn from(err: git2::Error) -> Self {
        match err.code() {
            git2::ErrorCode::NotFound => StoreError::NotFound,
            _other => StoreError::Other(err.to_string()),
        }
    }
}

pub struct GitStore {
    repo: Mutex<git2::Repository>,
    empty_tree_id: TreeId,
}

impl GitStore {
    pub fn load(path: PathBuf) -> Self {
        let repo = Mutex::new(git2::Repository::open(path).unwrap());
        let empty_tree_id =
            TreeId(hex::decode("4b825dc642cb6eb9a060e54bf8d69288fbee4904").unwrap());
        GitStore {
            repo,
            empty_tree_id,
        }
    }
}

fn signature_from_git(signature: git2::Signature) -> Signature {
    let name = signature.name().unwrap_or("<no name>").to_owned();
    let email = signature.email().unwrap_or("<no email>").to_owned();
    let timestamp = MillisSinceEpoch((signature.when().seconds() * 1000) as u64);
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

fn signature_to_git(signature: &Signature) -> git2::Signature {
    let name = &signature.name;
    let email = &signature.email;
    let time = git2::Time::new(
        (signature.timestamp.timestamp.0 / 1000) as i64,
        signature.timestamp.tz_offset,
    );
    git2::Signature::new(&name, &email, &time).unwrap()
}

fn serialize_note(commit: &Commit) -> String {
    let mut proto = protos::store::Commit::new();
    proto.is_open = commit.is_open;
    proto.is_pruned = commit.is_pruned;
    proto.change_id = commit.change_id.0.to_vec();
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.0.to_vec());
    }
    let bytes = proto.write_to_bytes().unwrap();
    hex::encode(bytes)
}

fn deserialize_note(commit: &mut Commit, note: &str) {
    let bytes = hex::decode(note).unwrap();
    let mut cursor = Cursor::new(bytes);
    let proto: protos::store::Commit = protobuf::parse_from_reader(&mut cursor).unwrap();
    commit.is_open = proto.is_open;
    commit.is_pruned = proto.is_pruned;
    commit.change_id = ChangeId(proto.change_id);
    for predecessor in &proto.predecessors {
        commit.predecessors.push(CommitId(predecessor.clone()));
    }
}

impl Debug for GitStore {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("GitStore")
            .field("path", &self.repo.lock().unwrap().path())
            .finish()
    }
}

impl Store for GitStore {
    fn hash_length(&self) -> usize {
        20
    }

    fn read_file(&self, _path: &FileRepoPath, id: &FileId) -> StoreResult<Box<dyn Read>> {
        if id.0.len() != self.hash_length() {
            return Err(StoreError::NotFound);
        }
        let locked_repo = self.repo.lock().unwrap();
        let blob = locked_repo
            .find_blob(Oid::from_bytes(id.0.as_slice()).unwrap())
            .unwrap();
        let content = blob.content().to_owned();
        Ok(Box::new(Cursor::new(content)))
    }

    fn write_file(&self, _path: &FileRepoPath, contents: &mut dyn Read) -> StoreResult<FileId> {
        let mut bytes = Vec::new();
        contents.read_to_end(&mut bytes).unwrap();
        let locked_repo = self.repo.lock().unwrap();
        let oid = locked_repo.blob(bytes.as_slice()).unwrap();
        Ok(FileId(oid.as_bytes().to_vec()))
    }

    fn read_symlink(&self, _path: &FileRepoPath, id: &SymlinkId) -> Result<String, StoreError> {
        if id.0.len() != self.hash_length() {
            return Err(StoreError::NotFound);
        }
        let locked_repo = self.repo.lock().unwrap();
        let blob = locked_repo
            .find_blob(Oid::from_bytes(id.0.as_slice()).unwrap())
            .unwrap();
        let target = String::from_utf8(blob.content().to_owned()).unwrap();
        Ok(target)
    }

    fn write_symlink(&self, _path: &FileRepoPath, target: &str) -> Result<SymlinkId, StoreError> {
        let locked_repo = self.repo.lock().unwrap();
        let oid = locked_repo.blob(target.as_bytes()).unwrap();
        Ok(SymlinkId(oid.as_bytes().to_vec()))
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn read_tree(&self, _path: &DirRepoPath, id: &TreeId) -> StoreResult<Tree> {
        if id == &self.empty_tree_id {
            return Ok(Tree::default());
        }
        if id.0.len() != self.hash_length() {
            return Err(StoreError::NotFound);
        }

        let locked_repo = self.repo.lock().unwrap();
        let git_tree = locked_repo
            .find_tree(Oid::from_bytes(id.0.as_slice()).unwrap())
            .unwrap();
        let mut tree = Tree::default();
        for entry in git_tree.iter() {
            let name = entry.name().unwrap();
            let (name, value) = match entry.kind().unwrap() {
                git2::ObjectType::Tree => {
                    let id = TreeId(entry.id().as_bytes().to_vec());
                    (entry.name().unwrap(), TreeValue::Tree(id))
                }
                git2::ObjectType::Blob => match entry.filemode() {
                    0o100644 => {
                        let id = FileId(entry.id().as_bytes().to_vec());
                        if name.ends_with(CONFLICT_SUFFIX) {
                            (
                                &name[0..name.len() - CONFLICT_SUFFIX.len()],
                                TreeValue::Conflict(ConflictId(entry.id().as_bytes().to_vec())),
                            )
                        } else {
                            (
                                name,
                                TreeValue::Normal {
                                    id,
                                    executable: false,
                                },
                            )
                        }
                    }
                    0o100755 => {
                        let id = FileId(entry.id().as_bytes().to_vec());
                        (
                            name,
                            TreeValue::Normal {
                                id,
                                executable: true,
                            },
                        )
                    }
                    0o120000 => {
                        let id = SymlinkId(entry.id().as_bytes().to_vec());
                        (name, TreeValue::Symlink(id))
                    }
                    mode => panic!("unexpected file mode {:?}", mode),
                },
                git2::ObjectType::Commit => {
                    let id = CommitId(entry.id().as_bytes().to_vec());
                    (name, TreeValue::GitSubmodule(id))
                }
                kind => panic!("unexpected object type {:?}", kind),
            };
            tree.set(name.to_string(), value);
        }
        Ok(tree)
    }

    fn write_tree(&self, _path: &DirRepoPath, contents: &Tree) -> StoreResult<TreeId> {
        let locked_repo = self.repo.lock().unwrap();
        let mut builder = locked_repo.treebuilder(None).unwrap();
        for entry in contents.entries() {
            let name = entry.name().to_owned();
            let (name, id, filemode) = match entry.value() {
                TreeValue::Normal {
                    id,
                    executable: false,
                } => (name, &id.0, 0o100644),
                TreeValue::Normal {
                    id,
                    executable: true,
                } => (name, &id.0, 0o100755),
                TreeValue::Symlink(id) => (name, &id.0, 0o120000),
                TreeValue::Tree(id) => (name, &id.0, 0o040000),
                TreeValue::GitSubmodule(id) => (name, &id.0, 0o160000),
                TreeValue::Conflict(id) => (name + CONFLICT_SUFFIX, &id.0, 0o100644),
            };
            builder
                .insert(name, Oid::from_bytes(id).unwrap(), filemode)
                .unwrap();
        }
        let oid = builder.write().unwrap();
        Ok(TreeId(oid.as_bytes().to_vec()))
    }

    fn read_commit(&self, id: &CommitId) -> StoreResult<Commit> {
        if id.0.len() != self.hash_length() {
            return Err(StoreError::NotFound);
        }

        let locked_repo = self.repo.lock().unwrap();
        let git_commit_id = Oid::from_bytes(id.0.as_slice())?;
        let commit = locked_repo.find_commit(git_commit_id)?;
        let change_id = ChangeId(id.0.clone().as_slice()[0..16].to_vec());
        let parents: Vec<_> = commit
            .parent_ids()
            .map(|oid| CommitId(oid.as_bytes().to_vec()))
            .collect();
        let tree_id = TreeId(commit.tree_id().as_bytes().to_vec());
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
            is_open: false,
            is_pruned: false,
        };

        let maybe_note = locked_repo.find_note(Some(NOTES_REF), git_commit_id).ok();
        if let Some(note) = maybe_note {
            deserialize_note(&mut commit, note.message().unwrap());
        }

        Ok(commit)
    }

    fn write_commit(&self, contents: &Commit) -> StoreResult<CommitId> {
        // TODO: We shouldn't have to create an in-memory index just to write an
        // object...
        let locked_repo = self.repo.lock().unwrap();
        let git_tree = locked_repo.find_tree(Oid::from_bytes(contents.root_tree.0.as_slice())?)?;
        let author = signature_to_git(&contents.author);
        let committer = signature_to_git(&contents.committer);
        let message = &contents.description;

        let mut parents = vec![];
        for parent_id in &contents.parents {
            let parent_git_commit =
                locked_repo.find_commit(Oid::from_bytes(parent_id.0.as_slice())?)?;
            parents.push(parent_git_commit);
        }
        let parent_refs: Vec<_> = parents.iter().collect();
        let git_id =
            locked_repo.commit(None, &author, &committer, &message, &git_tree, &parent_refs)?;
        let id = CommitId(git_id.as_bytes().to_vec());
        let note = serialize_note(contents);

        // TODO: Include the extra commit data in commit headers instead of a ref.
        // Unfortunately, it doesn't seem like libgit2-rs supports that. Perhaps
        // we'll have to serialize/deserialize the commit data ourselves.
        loop {
            let note_status = locked_repo.note(
                &committer,
                &committer,
                Some(NOTES_REF),
                git_id,
                &note,
                false,
            );
            match note_status {
                Err(err) if err.message().contains(NOTES_REF_LOCK) => {
                    // It seems that libgit2 doesn't retry when .git/refs/notes/jj/commits.lock
                    // already exists.
                    // TODO: Report this to libgit2.
                    let retry_delay = Duration::from_millis(10);
                    std::thread::sleep(retry_delay);
                }
                Err(err) => {
                    return Err(StoreError::from(err));
                }
                Ok(_) => {
                    break;
                }
            }
        }

        Ok(id)
    }

    fn read_conflict(&self, id: &ConflictId) -> StoreResult<Conflict> {
        let mut file = self.read_file(&FileRepoPath::from("unused"), &FileId(id.0.clone()))?;
        let mut data = String::new();
        file.read_to_string(&mut data)?;
        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        Ok(Conflict {
            removes: conflict_part_list_from_json(json.get("removes").unwrap()),
            adds: conflict_part_list_from_json(json.get("adds").unwrap()),
        })
    }

    fn write_conflict(&self, conflict: &Conflict) -> StoreResult<ConflictId> {
        let json = serde_json::json!({
            "removes": conflict_part_list_to_json(&conflict.removes),
            "adds": conflict_part_list_to_json(&conflict.adds),
        });
        let json_string = json.to_string();
        let mut bytes = json_string.as_bytes();
        // TODO: add a ref pointing to it so it won't get GC'd
        let file_id = self.write_file(&FileRepoPath::from("unused"), &mut bytes)?;
        Ok(ConflictId(file_id.0))
    }
}

fn conflict_part_list_to_json(parts: &[ConflictPart]) -> serde_json::Value {
    serde_json::Value::Array(parts.iter().map(conflict_part_to_json).collect())
}

fn conflict_part_list_from_json(json: &serde_json::Value) -> Vec<ConflictPart> {
    json.as_array()
        .unwrap()
        .iter()
        .map(conflict_part_from_json)
        .collect()
}

fn conflict_part_to_json(part: &ConflictPart) -> serde_json::Value {
    serde_json::json!({
        "value": tree_value_to_json(&part.value),
    })
}

fn conflict_part_from_json(json: &serde_json::Value) -> ConflictPart {
    let json_value = json.get("value").unwrap();
    ConflictPart {
        value: tree_value_from_json(json_value),
    }
}

fn tree_value_to_json(value: &TreeValue) -> serde_json::Value {
    match value {
        TreeValue::Normal { id, executable } => serde_json::json!({
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
        TreeValue::Normal {
            id: FileId(bytes_vec_from_json(json_file.get("id").unwrap())),
            executable: json_file.get("executable").unwrap().as_bool().unwrap(),
        }
    } else if let Some(json_id) = json.get("symlink_id") {
        TreeValue::Symlink(SymlinkId(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("tree_id") {
        TreeValue::Tree(TreeId(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("submodule_id") {
        TreeValue::GitSubmodule(CommitId(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("conflict_id") {
        TreeValue::Conflict(ConflictId(bytes_vec_from_json(json_id)))
    } else {
        panic!("unexpected json value in conflict: {:#?}", json);
    }
}

fn bytes_vec_from_json(value: &serde_json::Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap()).unwrap()
}

#[cfg(test)]
mod tests {

    use crate::store::{FileId, MillisSinceEpoch};

    use super::*;

    #[test]
    fn read_plain_git_commit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let git_repo_path = temp_dir.path();
        let git_repo = git2::Repository::init(git_repo_path.clone()).unwrap();

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
        let commit_id = CommitId(git_commit_id.as_bytes().to_vec());

        let store = GitStore::load(git_repo_path.to_owned());
        let commit = store.read_commit(&commit_id).unwrap();
        assert_eq!(
            &commit.change_id,
            &ChangeId(commit_id.0.as_slice()[0..16].to_vec())
        );
        assert_eq!(commit.parents, vec![]);
        assert_eq!(commit.predecessors, vec![]);
        assert_eq!(commit.root_tree.0.as_slice(), root_tree_id.as_bytes());
        assert_eq!(commit.is_open, false);
        assert_eq!(commit.is_pruned, false);
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
                &DirRepoPath::root(),
                &TreeId(root_tree_id.as_bytes().to_vec()),
            )
            .unwrap();
        let mut root_entries = root_tree.entries();
        let dir = root_entries.next().unwrap();
        assert_eq!(root_entries.next(), None);
        assert_eq!(dir.name(), "dir");
        assert_eq!(
            dir.value(),
            &TreeValue::Tree(TreeId(dir_tree_id.as_bytes().to_vec()))
        );

        let dir_tree = store
            .read_tree(
                &DirRepoPath::from("dir/"),
                &TreeId(dir_tree_id.as_bytes().to_vec()),
            )
            .unwrap();
        let mut files = dir_tree.entries();
        let normal_file = files.next().unwrap();
        let symlink = files.next().unwrap();
        assert_eq!(files.next(), None);
        assert_eq!(normal_file.name(), "normal");
        assert_eq!(
            normal_file.value(),
            &TreeValue::Normal {
                id: FileId(blob1.as_bytes().to_vec()),
                executable: false
            }
        );
        assert_eq!(symlink.name(), "symlink");
        assert_eq!(
            symlink.value(),
            &TreeValue::Symlink(SymlinkId(blob2.as_bytes().to_vec()))
        );
    }

    #[test]
    fn overlapping_git_commit_id() {
        let temp_dir = tempfile::tempdir().unwrap();
        let git_repo_path = temp_dir.path();
        git2::Repository::init(git_repo_path.clone()).unwrap();
        let store = GitStore::load(git_repo_path.to_owned());
        let signature = Signature {
            name: "Someone".to_string(),
            email: "someone@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        };
        let commit1 = Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: store.empty_tree_id().clone(),
            change_id: ChangeId(vec![]),
            description: "initial".to_string(),
            author: signature.clone(),
            committer: signature,
            is_open: false,
            is_pruned: false,
        };
        let commit_id1 = store.write_commit(&commit1).unwrap();
        let mut commit2 = commit1;
        commit2.predecessors.push(commit_id1.clone());
        let expected_error_message = format!("note for '{}' exists already", commit_id1.hex());
        match store.write_commit(&commit2) {
            Ok(_) => {
                panic!("expectedly successfully wrote two commits with the same git commit object")
            }
            Err(StoreError::Other(message)) if message.contains(&expected_error_message) => {}
            Err(err) => panic!("unexpected error: {:?}", err),
        };
    }
}
