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
use std::collections::BTreeMap;
use std::fmt::{Debug, Error, Formatter};
use std::io::Read;
use std::result::Result;
use std::vec::Vec;

use thiserror::Error;

use crate::content_hash::ContentHash;
use crate::repo_path::{RepoPath, RepoPathComponent};

pub trait ObjectId {
    fn new(value: Vec<u8>) -> Self;
    fn object_type(&self) -> String;
    fn from_bytes(bytes: &[u8]) -> Self;
    fn as_bytes(&self) -> &[u8];
    fn to_bytes(&self) -> Vec<u8>;
    fn from_hex(hex: &str) -> Self;
    fn hex(&self) -> String;
}

macro_rules! id_type {
    ($vis:vis $name:ident) => {
        content_hash! {
            #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
            $vis struct $name(Vec<u8>);
        }
        impl_id_type!($name);
    };
}

macro_rules! impl_id_type {
    ($name:ident) => {
        impl Debug for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
                f.debug_tuple(stringify!($name)).field(&self.hex()).finish()
            }
        }

        impl crate::backend::ObjectId for $name {
            fn new(value: Vec<u8>) -> Self {
                Self(value)
            }

            fn object_type(&self) -> String {
                stringify!($name)
                    .strip_suffix("Id")
                    .unwrap()
                    .to_ascii_lowercase()
                    .to_string()
            }

            fn from_bytes(bytes: &[u8]) -> Self {
                Self(bytes.to_vec())
            }

            fn as_bytes(&self) -> &[u8] {
                &self.0
            }

            fn to_bytes(&self) -> Vec<u8> {
                self.0.clone()
            }

            fn from_hex(hex: &str) -> Self {
                Self(hex::decode(hex).unwrap())
            }

            fn hex(&self) -> String {
                hex::encode(&self.0)
            }
        }
    };
}

id_type!(pub CommitId);
id_type!(pub ChangeId);
id_type!(pub TreeId);
id_type!(pub FileId);
id_type!(pub SymlinkId);
id_type!(pub ConflictId);

pub enum Phase {
    Public,
    Draft,
}

content_hash! {
    #[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord)]
    pub struct MillisSinceEpoch(pub i64);
}

content_hash! {
    #[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord)]
    pub struct Timestamp {
        pub timestamp: MillisSinceEpoch,
        // time zone offset in minutes
        pub tz_offset: i32,
    }
}

impl Timestamp {
    pub fn now() -> Self {
        Self::from_datetime(chrono::offset::Local::now())
    }

    pub fn from_datetime<Tz: chrono::TimeZone<Offset = chrono::offset::FixedOffset>>(
        datetime: chrono::DateTime<Tz>,
    ) -> Self {
        Self {
            timestamp: MillisSinceEpoch(datetime.timestamp_millis()),
            tz_offset: datetime.offset().local_minus_utc() / 60,
        }
    }
}

content_hash! {
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub struct Signature {
        pub name: String,
        pub email: String,
        pub timestamp: Timestamp,
    }
}

content_hash! {
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub struct Commit {
        pub parents: Vec<CommitId>,
        pub predecessors: Vec<CommitId>,
        pub root_tree: TreeId,
        pub change_id: ChangeId,
        pub description: String,
        pub author: Signature,
        pub committer: Signature,
    }
}

content_hash! {
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub struct ConflictTerm {
        // TODO: Store e.g. CommitId here too? Labels (theirs/ours/base)? Would those still be
        //       useful e.g. after rebasing this conflict?
        pub value: TreeValue,
    }
}

content_hash! {
    #[derive(Default, Debug, PartialEq, Eq, Clone)]
    pub struct Conflict {
        // A conflict is represented by a list of positive and negative states that need to be applied.
        // In a simple 3-way merge of B and C with merge base A, the conflict will be { add: [B, C],
        // remove: [A] }. Also note that a conflict of the form { add: [A], remove: [] } is the
        // same as non-conflict A.
        pub removes: Vec<ConflictTerm>,
        pub adds: Vec<ConflictTerm>,
    }
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error(
        "Invalid hash length for object of type {object_type} (expected {expected} bytes, got \
         {actual} bytes): {hash}"
    )]
    InvalidHashLength {
        expected: usize,
        actual: usize,
        object_type: String,
        hash: String,
    },
    #[error("Invalid hash for object of type {object_type} with hash {hash}: {source}")]
    InvalidHash {
        object_type: String,
        hash: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Invalid UTF-8 for object {hash} of type {object_type}: {source}")]
    InvalidUtf8 {
        object_type: String,
        hash: String,
        source: std::string::FromUtf8Error,
    },
    #[error("Object {hash} of type {object_type} not found: {source}")]
    ObjectNotFound {
        object_type: String,
        hash: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Error when reading object {hash} of type {object_type}: {source}")]
    ReadObject {
        object_type: String,
        hash: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Could not write object of type {object_type}: {source}")]
    WriteObject {
        object_type: &'static str,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Error: {0}")]
    Other(String),
}

pub type BackendResult<T> = Result<T, BackendError>;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum TreeValue {
    File { id: FileId, executable: bool },
    Symlink(SymlinkId),
    Tree(TreeId),
    GitSubmodule(CommitId),
    Conflict(ConflictId),
}

impl ContentHash for TreeValue {
    fn hash(&self, state: &mut impl digest::Update) {
        use TreeValue::*;
        match self {
            File { id, executable } => {
                state.update(&0u32.to_le_bytes());
                id.hash(state);
                executable.hash(state);
            }
            Symlink(id) => {
                state.update(&1u32.to_le_bytes());
                id.hash(state);
            }
            Tree(id) => {
                state.update(&2u32.to_le_bytes());
                id.hash(state);
            }
            GitSubmodule(id) => {
                state.update(&3u32.to_le_bytes());
                id.hash(state);
            }
            Conflict(id) => {
                state.update(&4u32.to_le_bytes());
                id.hash(state);
            }
        }
    }
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

content_hash! {
    #[derive(Default, PartialEq, Eq, Debug, Clone)]
    pub struct Tree {
        entries: BTreeMap<RepoPathComponent, TreeValue>,
    }
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

/// Calculates common prefix length of two bytes. The length to be returned is
/// a number of hexadecimal digits.
pub fn common_hex_len(bytes_a: &[u8], bytes_b: &[u8]) -> usize {
    iter_half_bytes(bytes_a)
        .zip(iter_half_bytes(bytes_b))
        .take_while(|(a, b)| a == b)
        .count()
}

fn iter_half_bytes(bytes: &[u8]) -> impl ExactSizeIterator<Item = u8> + '_ {
    (0..bytes.len() * 2).map(|i| {
        let v = bytes[i / 2];
        if i & 1 == 0 {
            v >> 4
        } else {
            v & 0xf
        }
    })
}

pub fn make_root_commit(root_change_id: ChangeId, empty_tree_id: TreeId) -> Commit {
    let timestamp = Timestamp {
        timestamp: MillisSinceEpoch(0),
        tz_offset: 0,
    };
    let signature = Signature {
        name: String::new(),
        email: String::new(),
        timestamp,
    };
    Commit {
        parents: vec![],
        predecessors: vec![],
        root_tree: empty_tree_id,
        change_id: root_change_id,
        description: String::new(),
        author: signature.clone(),
        committer: signature,
    }
}

pub trait Backend: Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;

    /// A unique name that identifies this backend. Written to
    /// `.jj/repo/store/backend` when the repo is created.
    fn name(&self) -> &str;

    /// The length of commit IDs in bytes.
    fn commit_id_length(&self) -> usize;

    /// The length of change IDs in bytes.
    fn change_id_length(&self) -> usize;

    fn read_file(&self, path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>>;

    fn write_file(&self, path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId>;

    fn read_symlink(&self, path: &RepoPath, id: &SymlinkId) -> BackendResult<String>;

    fn write_symlink(&self, path: &RepoPath, target: &str) -> BackendResult<SymlinkId>;

    fn root_commit_id(&self) -> &CommitId;

    fn root_change_id(&self) -> &ChangeId;

    fn empty_tree_id(&self) -> &TreeId;

    fn read_tree(&self, path: &RepoPath, id: &TreeId) -> BackendResult<Tree>;

    fn write_tree(&self, path: &RepoPath, contents: &Tree) -> BackendResult<TreeId>;

    fn read_conflict(&self, path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict>;

    fn write_conflict(&self, path: &RepoPath, contents: &Conflict) -> BackendResult<ConflictId>;

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit>;

    fn write_commit(&self, contents: &Commit) -> BackendResult<CommitId>;
}
