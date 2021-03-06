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

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt::{Debug, Error, Formatter};
use std::io::Read;
use std::result::Result;
use std::vec::Vec;

use thiserror::Error;

use crate::repo_path::{DirRepoPath, FileRepoPath};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct CommitId(pub Vec<u8>);

impl Debug for CommitId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("CommitId").field(&self.hex()).finish()
    }
}

impl CommitId {
    pub fn from_hex(hex: &str) -> Self {
        CommitId(hex::decode(hex).unwrap())
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct ChangeId(pub Vec<u8>);

impl Debug for ChangeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("ChangeId").field(&self.hex()).finish()
    }
}

impl ChangeId {
    pub fn from_hex(hex: &str) -> Self {
        ChangeId(hex::decode(hex).unwrap())
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct TreeId(pub Vec<u8>);

impl Debug for TreeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("TreeId").field(&self.hex()).finish()
    }
}

impl TreeId {
    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct FileId(pub Vec<u8>);

impl Debug for FileId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("FileId").field(&self.hex()).finish()
    }
}

impl FileId {
    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct SymlinkId(pub Vec<u8>);

impl Debug for SymlinkId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("SymlinkId").field(&self.hex()).finish()
    }
}

impl SymlinkId {
    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct ConflictId(pub Vec<u8>);

impl Debug for ConflictId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_tuple("ConflictId").field(&self.hex()).finish()
    }
}

impl ConflictId {
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
        let now = chrono::offset::Local::now();
        Self {
            timestamp: MillisSinceEpoch(now.timestamp_millis() as u64),
            tz_offset: now.offset().local_minus_utc() / 60,
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
    pub is_pruned: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ConflictPart {
    // TODO: Store e.g. CommitId here too? Labels (theirs/ours/base)? Would those still be
    //       useful e.g. after rebasing this conflict?
    pub value: TreeValue,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Conflict {
    // A conflict is represented by a list of positive and negative states that need to be applied.
    // In a simple 3-way merge of B and C with merge base A, the conflict will be { add: [B, C],
    // remove: [A] }. Also note that a conflict of the form { add: [A], remove: [] } is the
    // same as non-conflict A.
    pub removes: Vec<ConflictPart>,
    pub adds: Vec<ConflictPart>,
}

impl Conflict {
    // Returns (left,base,right) if this conflict is a 3-way conflict
    pub fn to_three_way(
        &self,
    ) -> Option<(
        Option<ConflictPart>,
        Option<ConflictPart>,
        Option<ConflictPart>,
    )> {
        if self.removes.len() == 1 && self.adds.len() == 2 {
            // Regular (modify/modify) 3-way conflict
            Some((
                Some(self.adds[0].clone()),
                Some(self.removes[0].clone()),
                Some(self.adds[1].clone()),
            ))
        } else if self.removes.is_empty() && self.adds.len() == 2 {
            // Add/add conflict
            Some((Some(self.adds[0].clone()), None, Some(self.adds[1].clone())))
        } else if self.removes.len() == 1 && self.adds.len() == 1 {
            // Modify/delete conflict
            Some((
                Some(self.adds[0].clone()),
                Some(self.removes[0].clone()),
                None,
            ))
        } else {
            None
        }
    }
}

impl Default for Conflict {
    fn default() -> Self {
        Conflict {
            removes: Default::default(),
            adds: Default::default(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("Object not found")]
    NotFound,
    #[error("Error: {0}")]
    Other(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

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
    name: &'a str,
    value: &'a TreeValue,
}

impl<'a> TreeEntry<'a> {
    pub fn new(name: &'a str, value: &'a TreeValue) -> Self {
        TreeEntry { name, value }
    }

    pub fn name(&self) -> &'a str {
        &self.name
    }

    pub fn value(&self) -> &'a TreeValue {
        &self.value
    }
}

pub struct TreeEntriesNonRecursiveIter<'a> {
    iter: std::collections::btree_map::Iter<'a, String, TreeValue>,
}

impl<'a> Iterator for TreeEntriesNonRecursiveIter<'a> {
    type Item = TreeEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|(name, value)| TreeEntry { name, value })
    }
}

#[derive(Debug, Clone)]
pub struct Tree {
    entries: BTreeMap<String, TreeValue>,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl Tree {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> TreeEntriesNonRecursiveIter {
        TreeEntriesNonRecursiveIter {
            iter: self.entries.iter(),
        }
    }

    pub fn set(&mut self, name: String, value: TreeValue) {
        self.entries.insert(name, value);
    }

    pub fn remove<N>(&mut self, name: &N)
    where
        N: Borrow<str> + ?Sized,
    {
        self.entries.remove(name.borrow());
    }

    pub fn entry<N>(&self, name: &N) -> Option<TreeEntry>
    where
        N: Borrow<str> + ?Sized,
    {
        self.entries
            .get_key_value(name.borrow())
            .map(|(name, value)| TreeEntry { name, value })
    }

    pub fn value<N>(&self, name: &N) -> Option<&TreeValue>
    where
        N: Borrow<str> + ?Sized,
    {
        self.entries.get(name.borrow())
    }
}

pub trait Store: Send + Sync + Debug {
    fn hash_length(&self) -> usize;

    fn git_repo(&self) -> Option<git2::Repository>;

    fn read_file(&self, path: &FileRepoPath, id: &FileId) -> StoreResult<Box<dyn Read>>;

    fn write_file(&self, path: &FileRepoPath, contents: &mut dyn Read) -> StoreResult<FileId>;

    fn read_symlink(&self, path: &FileRepoPath, id: &SymlinkId) -> StoreResult<String>;

    fn write_symlink(&self, path: &FileRepoPath, target: &str) -> StoreResult<SymlinkId>;

    fn empty_tree_id(&self) -> &TreeId;

    fn read_tree(&self, path: &DirRepoPath, id: &TreeId) -> StoreResult<Tree>;

    fn write_tree(&self, path: &DirRepoPath, contents: &Tree) -> StoreResult<TreeId>;

    fn read_commit(&self, id: &CommitId) -> StoreResult<Commit>;

    fn write_commit(&self, contents: &Commit) -> StoreResult<CommitId>;

    // TODO: Pass in the paths here too even though they are unused, just like for
    // files and trees?
    fn read_conflict(&self, id: &ConflictId) -> StoreResult<Conflict>;

    fn write_conflict(&self, contents: &Conflict) -> StoreResult<ConflictId>;
}
