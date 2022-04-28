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

use std::collections::BTreeMap;
use std::fmt::{Debug, Error, Formatter};
use std::io::Read;
use std::result::Result;
use std::vec::Vec;

use thiserror::Error;

use crate::repo_path::{RepoPath, RepoPathComponent};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct CommitId(Vec<u8>);

impl Debug for CommitId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("CommitId").field(&self.hex()).finish()
    }
}

impl CommitId {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn from_hex(hex: &str) -> Self {
        Self(hex::decode(hex).unwrap())
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct ChangeId(Vec<u8>);

impl Debug for ChangeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("ChangeId").field(&self.hex()).finish()
    }
}

impl ChangeId {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn from_hex(hex: &str) -> Self {
        Self(hex::decode(hex).unwrap())
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct TreeId(Vec<u8>);

impl Debug for TreeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("TreeId").field(&self.hex()).finish()
    }
}

impl TreeId {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct FileId(Vec<u8>);

impl Debug for FileId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("FileId").field(&self.hex()).finish()
    }
}

impl FileId {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct SymlinkId(Vec<u8>);

impl Debug for SymlinkId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("SymlinkId").field(&self.hex()).finish()
    }
}

impl SymlinkId {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct ConflictId(Vec<u8>);

impl Debug for ConflictId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("ConflictId").field(&self.hex()).finish()
    }
}

impl ConflictId {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

pub enum Phase {
    Public,
    Draft,
}

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord)]
pub struct MillisSinceEpoch(pub u64);

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord)]
pub struct Timestamp {
    pub timestamp: MillisSinceEpoch,
    // time zone offset in minutes
    pub tz_offset: i32,
}

impl Timestamp {
    pub fn now() -> Self {
        Self::from_datetime(chrono::offset::Local::now())
    }

    pub fn from_datetime<Tz: chrono::TimeZone<Offset = chrono::offset::FixedOffset>>(
        datetime: chrono::DateTime<Tz>,
    ) -> Self {
        Self {
            timestamp: MillisSinceEpoch(datetime.timestamp_millis() as u64),
            tz_offset: datetime.offset().local_minus_utc() / 60,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub parents: Vec<CommitId>,
    pub predecessors: Vec<CommitId>,
    pub root_tree: TreeId,
    pub change_id: ChangeId,
    pub description: String,
    pub author: Signature,
    pub committer: Signature,
    pub is_open: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ConflictPart {
    // TODO: Store e.g. CommitId here too? Labels (theirs/ours/base)? Would those still be
    //       useful e.g. after rebasing this conflict?
    pub value: TreeValue,
}

#[derive(Default, Debug, PartialEq, Eq, Clone)]
pub struct Conflict {
    // A conflict is represented by a list of positive and negative states that need to be applied.
    // In a simple 3-way merge of B and C with merge base A, the conflict will be { add: [B, C],
    // remove: [A] }. Also note that a conflict of the form { add: [A], remove: [] } is the
    // same as non-conflict A.
    pub removes: Vec<ConflictPart>,
    pub adds: Vec<ConflictPart>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BackendError {
    #[error("Object not found")]
    NotFound,
    #[error("Error: {0}")]
    Other(String),
}

pub type BackendResult<T> = Result<T, BackendError>;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum TreeValue {
    Normal { id: FileId, executable: bool },
    Symlink(SymlinkId),
    Tree(TreeId),
    GitSubmodule(CommitId),
    Conflict(ConflictId),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TreeEntry<'a> {
    name: &'a RepoPathComponent,
    value: &'a TreeValue,
}

impl<'a> TreeEntry<'a> {
    pub fn new(name: &'a RepoPathComponent, value: &'a TreeValue) -> Self {
        TreeEntry { name, value }
    }

    pub fn name(&self) -> &'a RepoPathComponent {
        self.name
    }

    pub fn value(&self) -> &'a TreeValue {
        self.value
    }
}

pub struct TreeEntriesNonRecursiveIterator<'a> {
    iter: std::collections::btree_map::Iter<'a, RepoPathComponent, TreeValue>,
}

impl<'a> Iterator for TreeEntriesNonRecursiveIterator<'a> {
    type Item = TreeEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|(name, value)| TreeEntry { name, value })
    }
}

#[derive(Default, Debug, Clone)]
pub struct Tree {
    entries: BTreeMap<RepoPathComponent, TreeValue>,
}

impl Tree {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> TreeEntriesNonRecursiveIterator {
        TreeEntriesNonRecursiveIterator {
            iter: self.entries.iter(),
        }
    }

    pub fn set(&mut self, name: RepoPathComponent, value: TreeValue) {
        self.entries.insert(name, value);
    }

    pub fn remove(&mut self, name: &RepoPathComponent) {
        self.entries.remove(name);
    }

    pub fn entry(&self, name: &RepoPathComponent) -> Option<TreeEntry> {
        self.entries
            .get_key_value(name)
            .map(|(name, value)| TreeEntry { name, value })
    }

    pub fn value(&self, name: &RepoPathComponent) -> Option<&TreeValue> {
        self.entries.get(name)
    }
}

pub trait Backend: Send + Sync + Debug {
    fn hash_length(&self) -> usize;

    fn git_repo(&self) -> Option<git2::Repository>;

    fn read_file(&self, path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>>;

    fn write_file(&self, path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId>;

    fn read_symlink(&self, path: &RepoPath, id: &SymlinkId) -> BackendResult<String>;

    fn write_symlink(&self, path: &RepoPath, target: &str) -> BackendResult<SymlinkId>;

    fn empty_tree_id(&self) -> &TreeId;

    fn read_tree(&self, path: &RepoPath, id: &TreeId) -> BackendResult<Tree>;

    fn write_tree(&self, path: &RepoPath, contents: &Tree) -> BackendResult<TreeId>;

    fn read_conflict(&self, path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict>;

    fn write_conflict(&self, path: &RepoPath, contents: &Conflict) -> BackendResult<ConflictId>;

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit>;

    fn write_commit(&self, contents: &Commit) -> BackendResult<CommitId>;
}
