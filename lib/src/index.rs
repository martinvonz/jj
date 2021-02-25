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

extern crate byteorder;

use std::cmp::{max, min, Ordering};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io;
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use blake2::{Blake2b, Digest};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use tempfile::NamedTempFile;

use crate::commit::Commit;
use crate::dag_walk;
use crate::op_store::OperationId;
use crate::operation::Operation;
use crate::repo::ReadonlyRepo;
use crate::store::CommitId;
use crate::store_wrapper::StoreWrapper;
use std::fmt::{Debug, Formatter};
use std::ops::Bound;

#[derive(Clone)]
pub enum IndexRef<'a> {
    Readonly(Arc<ReadonlyIndex>),
    Mutable(&'a MutableIndex),
}

impl From<Arc<ReadonlyIndex>> for IndexRef<'_> {
    fn from(index: Arc<ReadonlyIndex>) -> Self {
        IndexRef::Readonly(index)
    }
}

impl<'a> From<&'a MutableIndex> for IndexRef<'a> {
    fn from(index: &'a MutableIndex) -> Self {
        IndexRef::Mutable(index)
    }
}

impl<'a> IndexRef<'a> {
    pub fn num_commits(&self) -> u32 {
        match self {
            IndexRef::Readonly(index) => index.num_commits(),
            IndexRef::Mutable(index) => index.num_commits(),
        }
    }

    pub fn stats(&self) -> IndexStats {
        match self {
            IndexRef::Readonly(index) => index.stats(),
            IndexRef::Mutable(index) => index.stats(),
        }
    }

    pub fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32> {
        match self {
            IndexRef::Readonly(index) => index.commit_id_to_pos(commit_id),
            IndexRef::Mutable(index) => index.commit_id_to_pos(commit_id),
        }
    }

    pub fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution {
        match self {
            IndexRef::Readonly(index) => index.resolve_prefix(prefix),
            IndexRef::Mutable(index) => index.resolve_prefix(prefix),
        }
    }

    pub fn entry_by_id(&self, commit_id: &CommitId) -> Option<IndexEntry> {
        match self {
            IndexRef::Readonly(index) => index.entry_by_id(commit_id),
            IndexRef::Mutable(index) => index.entry_by_id(commit_id),
        }
    }

    pub fn has_id(&self, commit_id: &CommitId) -> bool {
        match self {
            IndexRef::Readonly(index) => index.has_id(commit_id),
            IndexRef::Mutable(index) => index.has_id(commit_id),
        }
    }

    pub fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool {
        match self {
            IndexRef::Readonly(index) => index.is_ancestor(ancestor_id, descendant_id),
            IndexRef::Mutable(index) => index.is_ancestor(ancestor_id, descendant_id),
        }
    }

    pub fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId> {
        match self {
            IndexRef::Readonly(index) => index.common_ancestors(set1, set2),
            IndexRef::Mutable(index) => index.common_ancestors(set1, set2),
        }
    }

    pub fn walk_revs(&self, wanted: &[CommitId], unwanted: &[CommitId]) -> RevWalk {
        match self {
            IndexRef::Readonly(index) => index.walk_revs(wanted, unwanted),
            IndexRef::Mutable(index) => index.walk_revs(wanted, unwanted),
        }
    }

    pub fn heads<'candidates>(
        &self,
        candidates: impl IntoIterator<Item = &'candidates CommitId>,
    ) -> Vec<CommitId> {
        match self {
            IndexRef::Readonly(index) => index.heads(candidates),
            IndexRef::Mutable(index) => index.heads(candidates),
        }
    }
}

struct CommitGraphEntry<'a> {
    data: &'a [u8],
    hash_length: usize,
}

// TODO: Add pointers to ancestors further back, like a skip list. Clear the
// lowest set bit to determine which generation number the pointers point to.
impl CommitGraphEntry<'_> {
    fn size(hash_length: usize) -> usize {
        32 + hash_length
    }

    fn is_pruned(&self) -> bool {
        (&self.data[0..]).read_u32::<LittleEndian>().unwrap() != 0
    }

    fn generation_number(&self) -> u32 {
        (&self.data[4..]).read_u32::<LittleEndian>().unwrap()
    }

    fn num_parents(&self) -> u32 {
        (&self.data[8..]).read_u32::<LittleEndian>().unwrap()
    }

    fn parent1_pos(&self) -> u32 {
        (&self.data[12..]).read_u32::<LittleEndian>().unwrap()
    }

    fn parent2_overflow_pos(&self) -> u32 {
        (&self.data[16..]).read_u32::<LittleEndian>().unwrap()
    }

    fn num_predecessors(&self) -> u32 {
        (&self.data[20..]).read_u32::<LittleEndian>().unwrap()
    }

    fn predecessor1_pos(&self) -> u32 {
        (&self.data[24..]).read_u32::<LittleEndian>().unwrap()
    }

    fn predecessor2_overflow_pos(&self) -> u32 {
        (&self.data[28..]).read_u32::<LittleEndian>().unwrap()
    }

    fn commit_id(&self) -> CommitId {
        CommitId(self.data[32..32 + self.hash_length].to_vec())
    }
}

struct CommitLookupEntry<'a> {
    data: &'a [u8],
    hash_length: usize,
}

impl CommitLookupEntry<'_> {
    fn size(hash_length: usize) -> usize {
        hash_length + 4
    }

    fn commit_id(&self) -> CommitId {
        CommitId(self.data[0..self.hash_length].to_vec())
    }

    fn pos(&self) -> u32 {
        (&self.data[self.hash_length..self.hash_length + 4])
            .read_u32::<LittleEndian>()
            .unwrap()
    }
}

// File format:
// u32: number of entries
// u32: number of parent overflow entries
// for each entry, in some topological order with parents first:
//   u32: generation number
//   u32: number of parents
//   u32: position in this table for parent 1
//   u32: position in the overflow table of parent 2
//   <hash length number of bytes>: commit id
// for each entry, sorted by commit id:
//   <hash length number of bytes>: commit id
//    u32: position in the entry table above
// TODO: add a version number
// TODO: replace the table by a trie so we don't have to repeat the full commit
//       ids
// TODO: add a fanout table like git's commit graph has?
pub struct ReadonlyIndex {
    dir: PathBuf,
    parent_file: Option<Arc<ReadonlyIndex>>,
    num_parent_commits: u32,
    name: String,
    hash_length: usize,
    commit_graph_entry_size: usize,
    commit_lookup_entry_size: usize,
    // Number of commits not counting the parent file
    num_local_commits: u32,
    graph: Vec<u8>,
    lookup: Vec<u8>,
    overflow_parent: Vec<u8>,
    overflow_predecessor: Vec<u8>,
}

impl Debug for ReadonlyIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("ReadonlyIndex")
            .field("name", &self.name)
            .field("parent_file", &self.parent_file)
            .finish()
    }
}

// Returns the ancestors of heads with parents and predecessors come before the
// commit itself
fn topo_order_earlier_first(
    store: &StoreWrapper,
    heads: Vec<CommitId>,
    parent_file: Option<Arc<ReadonlyIndex>>,
) -> Vec<Commit> {
    // First create a list of all commits in topological order with
    // children/successors first (reverse of what we want)
    let mut work = vec![];
    for head in &heads {
        work.push(store.get_commit(head).unwrap());
    }
    let mut commits = vec![];
    let mut visited = HashSet::new();
    let mut in_parent_file = HashSet::new();
    let parent_file_source = parent_file.as_ref().map(|file| file.as_ref());
    while !work.is_empty() {
        let commit = work.pop().unwrap();
        if parent_file_source.map_or(false, |index| index.has_id(commit.id())) {
            in_parent_file.insert(commit.id().clone());
            continue;
        } else if !visited.insert(commit.id().clone()) {
            continue;
        }

        work.extend(commit.parents());
        work.extend(commit.predecessors());
        commits.push(commit);
    }
    drop(visited);

    // Now create the topological order with earlier commits first. If we run into
    // any commits whose parents/predecessors have not all been indexed, put
    // them in the map of waiting commit (keyed by the commit they're waiting
    // for). Note that the order in the graph doesn't really have to be
    // topological, but it seems like a useful property to have.

    // Commits waiting for their parents/predecessors to be added
    let mut waiting = HashMap::new();

    let mut result = vec![];
    let mut visited = in_parent_file;
    while !commits.is_empty() {
        let commit = commits.pop().unwrap();
        let mut waiting_for_earlier_commit = false;
        for earlier in commit.parents().iter().chain(commit.predecessors().iter()) {
            if !visited.contains(earlier.id()) {
                waiting
                    .entry(earlier.id().clone())
                    .or_insert_with(Vec::new)
                    .push(commit.clone());
                waiting_for_earlier_commit = true;
                break;
            }
        }
        if !waiting_for_earlier_commit {
            visited.insert(commit.id().clone());
            if let Some(dependents) = waiting.remove(commit.id()) {
                commits.extend(dependents);
            }
            result.push(commit);
        }
    }
    assert!(waiting.is_empty());
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexPrefix(String);

impl HexPrefix {
    pub fn new(prefix: String) -> HexPrefix {
        assert!(
            prefix
                .matches(|c: char| !c.is_ascii_hexdigit() || c.is_ascii_uppercase())
                .next()
                .is_none(),
            "invalid hex prefix: {}",
            &prefix
        );
        HexPrefix(prefix)
    }

    pub fn bytes_prefixes(&self) -> (CommitId, CommitId) {
        if self.0.len() % 2 == 0 {
            let bytes = hex::decode(&self.0).unwrap();
            (CommitId(bytes.clone()), CommitId(bytes))
        } else {
            let min_bytes = hex::decode(&(self.0.clone() + "0")).unwrap();
            let prefix = min_bytes[0..min_bytes.len() - 1].to_vec();
            (CommitId(prefix), CommitId(min_bytes))
        }
    }

    pub fn matches(&self, id: &CommitId) -> bool {
        hex::encode(&id.0).starts_with(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixResolution {
    NoMatch,
    SingleMatch(CommitId),
    AmbiguousMatch,
}

impl PrefixResolution {
    fn plus(&self, other: &PrefixResolution) -> PrefixResolution {
        match (self, other) {
            (PrefixResolution::NoMatch, other) => other.clone(),
            (local, PrefixResolution::NoMatch) => local.clone(),
            (PrefixResolution::AmbiguousMatch, _) => PrefixResolution::AmbiguousMatch,
            (_, PrefixResolution::AmbiguousMatch) => PrefixResolution::AmbiguousMatch,
            (PrefixResolution::SingleMatch(_), PrefixResolution::SingleMatch(_)) => {
                PrefixResolution::AmbiguousMatch
            }
        }
    }
}

#[derive(Debug)]
struct MutableGraphEntry {
    commit_id: CommitId,
    is_pruned: bool,
    generation_number: u32,
    parent_positions: Vec<u32>,
    predecessor_positions: Vec<u32>,
}

pub struct MutableIndex {
    dir: PathBuf,
    parent_file: Option<Arc<ReadonlyIndex>>,
    num_parent_commits: u32,
    hash_length: usize,
    graph: Vec<MutableGraphEntry>,
    lookup: BTreeMap<CommitId, u32>,
}

impl MutableIndex {
    fn full(dir: PathBuf, hash_length: usize) -> Self {
        Self {
            dir,
            parent_file: None,
            num_parent_commits: 0,
            hash_length,
            graph: vec![],
            lookup: BTreeMap::new(),
        }
    }

    pub fn incremental(parent_file: Arc<ReadonlyIndex>) -> Self {
        let num_parent_commits = parent_file.num_parent_commits + parent_file.num_local_commits;
        let hash_length = parent_file.hash_length;
        Self {
            dir: parent_file.dir.clone(),
            parent_file: Some(parent_file),
            num_parent_commits,
            hash_length,
            graph: vec![],
            lookup: BTreeMap::new(),
        }
    }

    pub fn as_index_ref(&self) -> IndexRef {
        IndexRef::Mutable(self)
    }

    pub fn add_commit(&mut self, commit: &Commit) {
        self.add_commit_data(
            commit.id().clone(),
            commit.is_pruned(),
            commit.parent_ids(),
            commit.predecessor_ids(),
        );
    }

    fn add_commit_data(
        &mut self,
        id: CommitId,
        is_pruned: bool,
        parent_ids: Vec<CommitId>,
        predecessor_ids: Vec<CommitId>,
    ) {
        if self.has_id(&id) {
            return;
        }
        let mut entry = MutableGraphEntry {
            commit_id: id,
            is_pruned,
            generation_number: 0,
            parent_positions: vec![],
            predecessor_positions: vec![],
        };
        for parent_id in parent_ids {
            let parent_entry = self
                .entry_by_id(&parent_id)
                .expect("parent commit is not indexed");
            entry.generation_number = max(
                entry.generation_number,
                parent_entry.generation_number() + 1,
            );
            entry.parent_positions.push(parent_entry.pos);
        }
        for predecessor_id in predecessor_ids {
            let predecessor_entry = self
                .entry_by_id(&predecessor_id)
                .expect("predecessor commit is not indexed");
            entry.predecessor_positions.push(predecessor_entry.pos);
        }
        self.lookup.insert(
            entry.commit_id.clone(),
            self.graph.len() as u32 + self.num_parent_commits,
        );
        self.graph.push(entry);
    }

    fn serialize(self) -> Vec<u8> {
        assert_eq!(self.graph.len(), self.lookup.len());

        let num_commits = self.graph.len() as u32;

        let mut buf = vec![];

        if let Some(parent_file) = &self.parent_file {
            buf.write_u32::<LittleEndian>(parent_file.name.len() as u32)
                .unwrap();
            buf.write_all(&parent_file.name.as_bytes()).unwrap();
        } else {
            buf.write_u32::<LittleEndian>(0).unwrap();
        }

        buf.write_u32::<LittleEndian>(num_commits).unwrap();
        // We'll write the actual value later
        let parent_overflow_offset = buf.len();
        buf.write_u32::<LittleEndian>(0 as u32).unwrap();
        // We'll write the actual value later
        let predecessor_overflow_offset = buf.len();
        buf.write_u32::<LittleEndian>(0 as u32).unwrap();

        let mut parent_overflow = vec![];
        let mut predecessor_overflow = vec![];
        for entry in self.graph {
            let flags = if entry.is_pruned { 1 } else { 0 };
            buf.write_u32::<LittleEndian>(flags).unwrap();

            buf.write_u32::<LittleEndian>(entry.generation_number)
                .unwrap();

            buf.write_u32::<LittleEndian>(entry.parent_positions.len() as u32)
                .unwrap();
            let mut parent1_pos = 0;
            let parent_overflow_pos = parent_overflow.len() as u32;
            for (i, parent_pos) in entry.parent_positions.iter().enumerate() {
                if i == 0 {
                    parent1_pos = *parent_pos;
                } else {
                    parent_overflow.push(*parent_pos);
                }
            }
            buf.write_u32::<LittleEndian>(parent1_pos).unwrap();
            buf.write_u32::<LittleEndian>(parent_overflow_pos).unwrap();

            buf.write_u32::<LittleEndian>(entry.predecessor_positions.len() as u32)
                .unwrap();
            let mut predecessor1_pos = 0;
            let predecessor_overflow_pos = predecessor_overflow.len() as u32;
            for (i, predecessor_pos) in entry.predecessor_positions.iter().enumerate() {
                if i == 0 {
                    predecessor1_pos = *predecessor_pos;
                } else {
                    predecessor_overflow.push(*predecessor_pos);
                }
            }
            buf.write_u32::<LittleEndian>(predecessor1_pos).unwrap();
            buf.write_u32::<LittleEndian>(predecessor_overflow_pos)
                .unwrap();

            assert_eq!(entry.commit_id.0.len(), self.hash_length);
            buf.write_all(entry.commit_id.0.as_slice()).unwrap();
        }

        for (commit_id, pos) in self.lookup {
            buf.write_all(commit_id.0.as_slice()).unwrap();
            buf.write_u32::<LittleEndian>(pos).unwrap();
        }

        buf[parent_overflow_offset..parent_overflow_offset + 4]
            .as_mut()
            .write_u32::<LittleEndian>(parent_overflow.len() as u32)
            .unwrap();
        for parent_pos in parent_overflow {
            buf.write_u32::<LittleEndian>(parent_pos).unwrap();
        }
        buf[predecessor_overflow_offset..predecessor_overflow_offset + 4]
            .as_mut()
            .write_u32::<LittleEndian>(predecessor_overflow.len() as u32)
            .unwrap();
        for predecessor_pos in predecessor_overflow {
            buf.write_u32::<LittleEndian>(predecessor_pos).unwrap();
        }

        buf
    }

    /// If the MutableIndex has more commits than its parent ReadonlyIndex,
    /// return MutableIndex with the commits from both. This is done
    /// recursively, so the stack of index files has O(log n) files.
    fn maybe_squash_with_ancestors(self) -> MutableIndex {
        if self.parent_file.is_none() {
            return self;
        }

        let mut num_new_commits = self.segment_num_commits();
        let mut parent_file = self.parent_file.as_ref().unwrap().clone();
        let mut squashed;
        loop {
            // TODO: We should probably also squash if the parent file has less than N
            // commits, regardless of how many (few) are in `self`.
            if num_new_commits < parent_file.segment_num_commits() {
                squashed = MutableIndex::incremental(parent_file);
                break;
            }
            if parent_file.parent_file.is_none() {
                squashed = MutableIndex::full(self.dir.clone(), self.hash_length);
                break;
            }
            num_new_commits += parent_file.segment_num_commits();
            parent_file = parent_file.parent_file.as_ref().unwrap().clone();
        }

        // TODO: This can be made more efficient by walking the parent files in order
        // and not looking up via `self`.
        for pos in squashed.num_parent_commits..self.num_commits() {
            let entry = self.entry_by_pos(pos);
            let parent_ids: Vec<_> = entry
                .parent_positions()
                .iter()
                .map(|pos| self.entry_by_pos(*pos).commit_id())
                .collect();
            let predecessor_ids: Vec<_> = entry
                .predecessor_positions()
                .iter()
                .map(|pos| self.entry_by_pos(*pos).commit_id())
                .collect();
            squashed.add_commit_data(
                entry.commit_id(),
                entry.is_pruned(),
                parent_ids,
                predecessor_ids,
            );
        }
        squashed
    }

    pub fn save(self) -> io::Result<Arc<ReadonlyIndex>> {
        if self.segment_num_commits() == 0 && self.parent_file.is_some() {
            return Ok(self.parent_file.unwrap());
        }

        let hash_length = self.hash_length;
        let dir = self.dir.clone();

        let buf = self.maybe_squash_with_ancestors().serialize();
        let mut hasher = Blake2b::new();
        hasher.update(&buf);
        let index_file_id_hex = hex::encode(&hasher.finalize());
        let index_file_path = dir.join(&index_file_id_hex);

        let mut temp_file = NamedTempFile::new_in(&dir)?;
        let file = temp_file.as_file_mut();
        file.write_all(&buf).unwrap();
        temp_file.persist(&index_file_path)?;

        let mut cursor = Cursor::new(&buf);
        ReadonlyIndex::load_from(&mut cursor, dir, index_file_id_hex, hash_length)
    }

    pub fn num_commits(&self) -> u32 {
        CompositeIndex(self).num_commits()
    }

    pub fn stats(&self) -> IndexStats {
        CompositeIndex(self).stats()
    }

    pub fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32> {
        CompositeIndex(self).commit_id_to_pos(commit_id)
    }

    pub fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution {
        CompositeIndex(self).resolve_prefix(prefix)
    }

    pub fn entry_by_id(&self, commit_id: &CommitId) -> Option<IndexEntry> {
        CompositeIndex(self).entry_by_id(commit_id)
    }

    pub fn entry_by_pos(&self, pos: u32) -> IndexEntry {
        CompositeIndex(self).entry_by_pos(pos)
    }

    pub fn has_id(&self, commit_id: &CommitId) -> bool {
        CompositeIndex(self).has_id(commit_id)
    }

    pub fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool {
        CompositeIndex(self).is_ancestor(ancestor_id, descendant_id)
    }

    pub fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId> {
        CompositeIndex(self).common_ancestors(set1, set2)
    }

    pub fn walk_revs(&self, wanted: &[CommitId], unwanted: &[CommitId]) -> RevWalk {
        CompositeIndex(self).walk_revs(wanted, unwanted)
    }

    pub fn heads<'candidates>(
        &self,
        candidates: impl IntoIterator<Item = &'candidates CommitId>,
    ) -> Vec<CommitId> {
        CompositeIndex(self).heads(candidates)
    }
}

trait IndexSegment {
    fn segment_num_parent_commits(&self) -> u32;

    fn segment_num_commits(&self) -> u32;

    fn segment_parent_file(&self) -> &Option<Arc<ReadonlyIndex>>;

    fn segment_name(&self) -> Option<String>;

    fn segment_commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32>;

    fn segment_resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution;

    fn segment_is_pruned(&self, local_pos: u32) -> bool;

    fn segment_generation_number(&self, local_pos: u32) -> u32;

    fn segment_commit_id(&self, local_pos: u32) -> CommitId;

    fn segment_num_parents(&self, local_pos: u32) -> u32;

    fn segment_parent_positions(&self, local_pos: u32) -> Vec<u32>;

    fn segment_num_predecessors(&self, local_pos: u32) -> u32;

    fn segment_predecessor_positions(&self, local_pos: u32) -> Vec<u32>;

    fn segment_entry_by_pos(&self, pos: u32, local_pos: u32) -> IndexEntry;
}

#[derive(Clone)]
struct CompositeIndex<'a>(&'a dyn IndexSegment);

impl<'a> CompositeIndex<'a> {
    pub fn num_commits(&self) -> u32 {
        self.0.segment_num_parent_commits() + self.0.segment_num_commits()
    }

    pub fn stats(&self) -> IndexStats {
        let num_commits = self.num_commits();
        let mut num_merges = 0;
        let mut max_generation_number = 0;
        let mut is_head = vec![true; num_commits as usize];
        for pos in 0..num_commits {
            let entry = self.entry_by_pos(pos);
            max_generation_number = max(max_generation_number, entry.generation_number());
            if entry.num_parents() > 1 {
                num_merges += 1;
            }
            for parent_pos in entry.parent_positions() {
                is_head[parent_pos as usize] = false;
            }
        }
        let num_heads = is_head.iter().filter(|is_head| **is_head).count() as u32;

        let mut levels = vec![IndexLevelStats {
            num_commits: self.0.segment_num_commits(),
            name: self.0.segment_name(),
        }];
        let mut parent_file = self.0.segment_parent_file().clone();
        while parent_file.is_some() {
            let file = parent_file.as_ref().unwrap();
            levels.push(IndexLevelStats {
                num_commits: file.segment_num_commits(),
                name: file.segment_name(),
            });
            parent_file = file.segment_parent_file().clone();
        }
        levels.reverse();

        IndexStats {
            num_commits,
            num_merges,
            max_generation_number,
            num_heads,
            levels,
        }
    }

    fn entry_by_pos(&self, pos: u32) -> IndexEntry<'a> {
        let num_parent_commits = self.0.segment_num_parent_commits();
        if pos >= num_parent_commits {
            self.0.segment_entry_by_pos(pos, pos - num_parent_commits)
        } else {
            let parent_file: &ReadonlyIndex =
                self.0.segment_parent_file().as_ref().unwrap().as_ref();
            // The parent ReadonlyIndex outlives the child
            let parent_file: &'a ReadonlyIndex = unsafe { std::mem::transmute(parent_file) };

            CompositeIndex(parent_file).entry_by_pos(pos)
        }
    }

    pub fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32> {
        let local_match = self.0.segment_commit_id_to_pos(commit_id);
        local_match.or_else(|| {
            self.0
                .segment_parent_file()
                .as_ref()
                .and_then(|file| IndexRef::Readonly(file.clone()).commit_id_to_pos(commit_id))
        })
    }

    pub fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution {
        let local_match = self.0.segment_resolve_prefix(prefix);
        if local_match == PrefixResolution::AmbiguousMatch {
            // return early to avoid checking the parent file(s)
            return local_match;
        }
        let parent_match = self
            .0
            .segment_parent_file()
            .as_ref()
            .map_or(PrefixResolution::NoMatch, |file| {
                file.resolve_prefix(prefix)
            });
        local_match.plus(&parent_match)
    }

    pub fn entry_by_id(&self, commit_id: &CommitId) -> Option<IndexEntry<'a>> {
        self.commit_id_to_pos(commit_id)
            .map(&|pos| self.entry_by_pos(pos))
    }

    pub fn has_id(&self, commit_id: &CommitId) -> bool {
        self.commit_id_to_pos(commit_id).is_some()
    }

    pub fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool {
        let ancestor_pos = self.commit_id_to_pos(ancestor_id).unwrap();
        let descendant_pos = self.commit_id_to_pos(descendant_id).unwrap();
        self.is_ancestor_pos(ancestor_pos, descendant_pos)
    }

    fn is_ancestor_pos(&self, ancestor_pos: u32, descendant_pos: u32) -> bool {
        let ancestor_generation = self.entry_by_pos(ancestor_pos).generation_number();
        let mut work = vec![descendant_pos];
        let mut visited = HashSet::new();
        while !work.is_empty() {
            let descendant_pos = work.pop().unwrap();
            let descendant_entry = self.entry_by_pos(descendant_pos);
            if descendant_pos == ancestor_pos {
                return true;
            }
            if !visited.insert(descendant_entry.pos) {
                continue;
            }
            if descendant_entry.generation_number() <= ancestor_generation {
                continue;
            }
            work.extend(descendant_entry.parent_positions());
        }
        false
    }

    pub fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId> {
        let pos1: Vec<_> = set1
            .iter()
            .map(|id| self.commit_id_to_pos(id).unwrap())
            .collect();
        let pos2: Vec<_> = set2
            .iter()
            .map(|id| self.commit_id_to_pos(id).unwrap())
            .collect();
        self.common_ancestors_pos(&pos1, &pos2)
            .iter()
            .map(|pos| self.entry_by_pos(*pos).commit_id())
            .collect()
    }

    fn common_ancestors_pos(&self, set1: &[u32], set2: &[u32]) -> BTreeSet<u32> {
        let mut items1: BTreeSet<_> = set1
            .iter()
            .map(|pos| IndexEntryByGeneration(self.entry_by_pos(*pos)))
            .collect();
        let mut items2: BTreeSet<_> = set2
            .iter()
            .map(|pos| IndexEntryByGeneration(self.entry_by_pos(*pos)))
            .collect();

        let mut result = BTreeSet::new();
        while !(items1.is_empty() || items2.is_empty()) {
            let entry1 = items1.last().unwrap();
            let entry2 = items2.last().unwrap();
            match entry1.cmp(&entry2) {
                Ordering::Greater => {
                    let entry1 = items1.pop_last().unwrap();
                    for pos in entry1.0.parent_positions() {
                        items1.insert(IndexEntryByGeneration(self.entry_by_pos(pos)));
                    }
                }
                Ordering::Less => {
                    let entry2 = items2.pop_last().unwrap();
                    for pos in entry2.0.parent_positions() {
                        items2.insert(IndexEntryByGeneration(self.entry_by_pos(pos)));
                    }
                }
                Ordering::Equal => {
                    result.insert(entry1.0.pos);
                    items1.pop_last();
                    items2.pop_last();
                }
            }
        }
        self.heads_pos(result)
    }

    pub fn walk_revs(&self, wanted: &[CommitId], unwanted: &[CommitId]) -> RevWalk<'a> {
        let mut rev_walk = RevWalk::new(self.clone());
        for pos in wanted.iter().map(|id| self.commit_id_to_pos(id).unwrap()) {
            rev_walk.add_wanted(pos);
        }
        for pos in unwanted.iter().map(|id| self.commit_id_to_pos(id).unwrap()) {
            rev_walk.add_unwanted(pos);
        }
        rev_walk
    }

    pub fn heads<'candidates>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'candidates CommitId>,
    ) -> Vec<CommitId> {
        let candidate_positions: BTreeSet<_> = candidate_ids
            .into_iter()
            .map(|id| self.commit_id_to_pos(id).unwrap())
            .collect();

        self.heads_pos(candidate_positions)
            .iter()
            .map(|pos| self.entry_by_pos(*pos).commit_id())
            .collect()
    }

    fn heads_pos(&self, mut candidate_positions: BTreeSet<u32>) -> BTreeSet<u32> {
        // Add all parents of the candidates to the work queue. The parents and their
        // ancestors are not heads.
        // Also find the smallest generation number among the candidates.
        let mut work = BinaryHeap::new();
        let mut min_generation = std::u32::MAX;
        for pos in &candidate_positions {
            let entry = self.entry_by_pos(*pos);
            min_generation = min(min_generation, entry.generation_number());
            for parent_pos in entry.parent_positions() {
                work.push(IndexEntryByGeneration(self.entry_by_pos(parent_pos)));
            }
        }

        // Walk ancestors of the parents of the candidates. Remove visited commits from
        // set of candidates. Stop walking when we have gone past the minimum
        // candidate generation.
        let mut visited = HashSet::new();
        while !work.is_empty() {
            let item = work.pop().unwrap().0;
            if !visited.insert(item.pos) {
                continue;
            }
            if item.generation_number() < min_generation {
                break;
            }
            candidate_positions.remove(&item.pos);
            for parent_pos in item.parent_positions() {
                work.push(IndexEntryByGeneration(self.entry_by_pos(parent_pos)));
            }
        }
        candidate_positions
    }
}

pub struct IndexLevelStats {
    pub num_commits: u32,
    pub name: Option<String>,
}

pub struct IndexStats {
    pub num_commits: u32,
    pub num_merges: u32,
    pub max_generation_number: u32,
    pub num_heads: u32,
    pub levels: Vec<IndexLevelStats>,
}

#[derive(Eq, PartialEq)]
struct IndexEntryByGeneration<'a>(IndexEntry<'a>);

impl Ord for IndexEntryByGeneration<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .generation_number()
            .cmp(&other.0.generation_number())
            .then(self.0.pos.cmp(&other.0.pos))
    }
}

impl PartialOrd for IndexEntryByGeneration<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct RevWalkWorkItem<'a> {
    entry: IndexEntryByGeneration<'a>,
    wanted: bool,
}

pub struct RevWalk<'a> {
    index: CompositeIndex<'a>,
    items: BinaryHeap<RevWalkWorkItem<'a>>,
    wanted_boundary_set: HashSet<u32>,
    unwanted_boundary_set: HashSet<u32>,
}

impl<'a> RevWalk<'a> {
    fn new(index: CompositeIndex<'a>) -> Self {
        Self {
            index,
            items: BinaryHeap::new(),
            wanted_boundary_set: HashSet::new(),
            unwanted_boundary_set: HashSet::new(),
        }
    }

    fn add_wanted(&mut self, pos: u32) {
        if !self.wanted_boundary_set.insert(pos) {
            return;
        }
        self.items.push(RevWalkWorkItem {
            entry: IndexEntryByGeneration(self.index.entry_by_pos(pos)),
            wanted: true,
        });
    }

    fn add_unwanted(&mut self, pos: u32) {
        if !self.unwanted_boundary_set.insert(pos) {
            return;
        }
        self.items.push(RevWalkWorkItem {
            entry: IndexEntryByGeneration(self.index.entry_by_pos(pos)),
            wanted: false,
        });
    }
}

impl<'a> Iterator for RevWalk<'a> {
    type Item = CommitId;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.wanted_boundary_set.is_empty() {
            let item = self.items.pop().unwrap();
            if item.wanted {
                self.wanted_boundary_set.remove(&item.entry.0.pos);
                if self.unwanted_boundary_set.contains(&item.entry.0.pos) {
                    continue;
                }
                for parent_pos in item.entry.0.parent_positions() {
                    self.add_wanted(parent_pos);
                }
                return Some(item.entry.0.commit_id());
            } else {
                self.unwanted_boundary_set.remove(&item.entry.0.pos);
                for parent_pos in item.entry.0.parent_positions() {
                    self.add_unwanted(parent_pos);
                }
            }
        }
        None
    }
}

impl IndexSegment for ReadonlyIndex {
    fn segment_num_parent_commits(&self) -> u32 {
        self.num_parent_commits
    }

    fn segment_num_commits(&self) -> u32 {
        self.num_local_commits
    }

    fn segment_parent_file(&self) -> &Option<Arc<ReadonlyIndex>> {
        &self.parent_file
    }

    fn segment_name(&self) -> Option<String> {
        Some(self.name.clone())
    }

    fn segment_commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32> {
        if self.num_local_commits == 0 {
            // Avoid overflow when subtracting 1 below
            return None;
        }
        let mut low = 0;
        let mut high = self.num_local_commits - 1;

        // binary search for the commit id
        loop {
            let mid = (low + high) / 2;
            let entry = self.lookup_entry(mid);
            let entry_commit_id = entry.commit_id();
            if high == low {
                return if &entry_commit_id == commit_id {
                    Some(entry.pos())
                } else {
                    None
                };
            }
            if commit_id > &entry_commit_id {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
    }

    fn segment_resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution {
        let (bytes_prefix, min_bytes_prefix) = prefix.bytes_prefixes();
        match self.commit_id_byte_prefix_to_pos(&min_bytes_prefix) {
            None => PrefixResolution::NoMatch,
            Some(lookup_pos) => {
                let mut first_match = None;
                for i in lookup_pos..self.num_local_commits as u32 {
                    let entry = self.lookup_entry(i);
                    let id = entry.commit_id();
                    if !id.0.starts_with(&bytes_prefix.0) {
                        break;
                    }
                    if prefix.matches(&id) {
                        if first_match.is_some() {
                            return PrefixResolution::AmbiguousMatch;
                        }
                        first_match = Some(id)
                    }
                }
                match first_match {
                    None => PrefixResolution::NoMatch,
                    Some(id) => PrefixResolution::SingleMatch(id),
                }
            }
        }
    }

    fn segment_is_pruned(&self, local_pos: u32) -> bool {
        let graph_entry = self.graph_entry(local_pos);
        graph_entry.is_pruned()
    }

    fn segment_generation_number(&self, local_pos: u32) -> u32 {
        self.graph_entry(local_pos).generation_number()
    }

    fn segment_commit_id(&self, local_pos: u32) -> CommitId {
        self.graph_entry(local_pos).commit_id()
    }

    fn segment_num_parents(&self, local_pos: u32) -> u32 {
        self.graph_entry(local_pos).num_parents()
    }

    fn segment_parent_positions(&self, local_pos: u32) -> Vec<u32> {
        let graph_entry = self.graph_entry(local_pos);
        let mut parent_entries = vec![];
        if graph_entry.num_parents() >= 1 {
            parent_entries.push(graph_entry.parent1_pos());
        }
        if graph_entry.num_parents() >= 2 {
            let mut parent_overflow_pos = graph_entry.parent2_overflow_pos();
            for _ in 1..graph_entry.num_parents() {
                parent_entries.push(self.overflow_parent(parent_overflow_pos));
                parent_overflow_pos += 1;
            }
        }
        parent_entries
    }

    fn segment_num_predecessors(&self, local_pos: u32) -> u32 {
        self.graph_entry(local_pos).num_predecessors()
    }

    fn segment_predecessor_positions(&self, local_pos: u32) -> Vec<u32> {
        let graph_entry = self.graph_entry(local_pos);
        let mut predecessor_entries = vec![];
        if graph_entry.num_predecessors() >= 1 {
            predecessor_entries.push(graph_entry.predecessor1_pos());
        }
        if graph_entry.num_predecessors() >= 2 {
            let mut predecessor_overflow_pos = graph_entry.predecessor2_overflow_pos();
            for _ in 1..graph_entry.num_predecessors() {
                predecessor_entries.push(self.overflow_predecessor(predecessor_overflow_pos));
                predecessor_overflow_pos += 1;
            }
        }
        predecessor_entries
    }

    fn segment_entry_by_pos(&self, pos: u32, local_pos: u32) -> IndexEntry {
        IndexEntry {
            source: self,
            local_pos,
            pos,
        }
    }
}

impl IndexSegment for MutableIndex {
    fn segment_num_parent_commits(&self) -> u32 {
        self.num_parent_commits
    }

    fn segment_num_commits(&self) -> u32 {
        self.graph.len() as u32
    }

    fn segment_parent_file(&self) -> &Option<Arc<ReadonlyIndex>> {
        &self.parent_file
    }

    fn segment_name(&self) -> Option<String> {
        None
    }

    fn segment_commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32> {
        self.lookup.get(commit_id).cloned()
    }

    fn segment_resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution {
        let (bytes_prefix, min_bytes_prefix) = prefix.bytes_prefixes();
        let mut potential_range = self
            .lookup
            .range((Bound::Included(&min_bytes_prefix), Bound::Unbounded));
        let mut first_match = None;
        loop {
            match potential_range.next() {
                None => {
                    break;
                }
                Some((id, _pos)) => {
                    if !id.0.starts_with(&bytes_prefix.0) {
                        break;
                    }
                    if prefix.matches(&id) {
                        if first_match.is_some() {
                            return PrefixResolution::AmbiguousMatch;
                        }
                        first_match = Some(id)
                    }
                }
            }
        }
        match first_match {
            None => PrefixResolution::NoMatch,
            Some(id) => PrefixResolution::SingleMatch(id.clone()),
        }
    }

    fn segment_is_pruned(&self, local_pos: u32) -> bool {
        self.graph[local_pos as usize].is_pruned
    }

    fn segment_generation_number(&self, local_pos: u32) -> u32 {
        self.graph[local_pos as usize].generation_number
    }

    fn segment_commit_id(&self, local_pos: u32) -> CommitId {
        self.graph[local_pos as usize].commit_id.clone()
    }

    fn segment_num_parents(&self, local_pos: u32) -> u32 {
        self.graph[local_pos as usize].parent_positions.len() as u32
    }

    fn segment_parent_positions(&self, local_pos: u32) -> Vec<u32> {
        self.graph[local_pos as usize].parent_positions.clone()
    }

    fn segment_num_predecessors(&self, local_pos: u32) -> u32 {
        self.graph[local_pos as usize].predecessor_positions.len() as u32
    }

    fn segment_predecessor_positions(&self, local_pos: u32) -> Vec<u32> {
        self.graph[local_pos as usize].predecessor_positions.clone()
    }

    fn segment_entry_by_pos(&self, pos: u32, local_pos: u32) -> IndexEntry {
        IndexEntry {
            source: self,
            local_pos,
            pos,
        }
    }
}

#[derive(Clone)]
pub struct IndexEntry<'a> {
    source: &'a dyn IndexSegment,
    pos: u32,
    // Position within the source segment
    local_pos: u32,
}

impl PartialEq for IndexEntry<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
    }
}
impl Eq for IndexEntry<'_> {}

impl IndexEntry<'_> {
    pub fn generation_number(&self) -> u32 {
        self.source.segment_generation_number(self.local_pos)
    }

    pub fn commit_id(&self) -> CommitId {
        self.source.segment_commit_id(self.local_pos)
    }

    fn is_pruned(&self) -> bool {
        self.source.segment_is_pruned(self.local_pos)
    }

    pub fn num_parents(&self) -> u32 {
        self.source.segment_num_parents(self.local_pos)
    }

    fn parent_positions(&self) -> Vec<u32> {
        self.source.segment_parent_positions(self.local_pos)
    }

    pub fn num_predecessors(&self) -> u32 {
        self.source.segment_num_predecessors(self.local_pos)
    }

    fn predecessor_positions(&self) -> Vec<u32> {
        self.source.segment_predecessor_positions(self.local_pos)
    }
}

impl ReadonlyIndex {
    pub fn init(dir: PathBuf) {
        std::fs::create_dir(dir.join("operations")).unwrap();
    }

    pub fn reinit(dir: PathBuf) {
        std::fs::remove_dir_all(dir.join("operations")).unwrap();
        ReadonlyIndex::init(dir);
    }

    pub fn load(repo: &ReadonlyRepo, dir: PathBuf, op_id: OperationId) -> Arc<ReadonlyIndex> {
        let op_id_hex = op_id.hex();
        let op_id_file = dir.join("operations").join(&op_id_hex);
        if op_id_file.exists() {
            let op_id = OperationId(hex::decode(op_id_hex).unwrap());
            ReadonlyIndex::load_at_operation(dir, repo.store().hash_length(), &op_id).unwrap()
        } else {
            let op = repo.view().as_view_ref().get_operation(&op_id).unwrap();
            ReadonlyIndex::index(repo.store(), dir, &op).unwrap()
        }
    }

    fn load_from(
        file: &mut dyn Read,
        dir: PathBuf,
        name: String,
        hash_length: usize,
    ) -> io::Result<Arc<ReadonlyIndex>> {
        let parent_filename_len = file.read_u32::<LittleEndian>()?;
        let num_parent_commits;
        let maybe_parent_file;
        if parent_filename_len > 0 {
            let mut parent_filename_bytes = vec![0; parent_filename_len as usize];
            file.read_exact(&mut parent_filename_bytes)?;
            let parent_filename = String::from_utf8(parent_filename_bytes).unwrap();
            let parent_file_path = dir.join(&parent_filename);
            let mut index_file = File::open(&parent_file_path).unwrap();
            let parent_file = ReadonlyIndex::load_from(
                &mut index_file,
                dir.clone(),
                parent_filename,
                hash_length,
            )?;
            num_parent_commits = parent_file.num_parent_commits + parent_file.num_local_commits;
            maybe_parent_file = Some(parent_file);
        } else {
            num_parent_commits = 0;
            maybe_parent_file = None;
        };
        let num_commits = file.read_u32::<LittleEndian>()?;
        let num_parent_overflow_entries = file.read_u32::<LittleEndian>()?;
        let num_predecessor_overflow_entries = file.read_u32::<LittleEndian>()?;
        let mut data = vec![];
        file.read_to_end(&mut data)?;
        let commit_graph_entry_size = CommitGraphEntry::size(hash_length);
        let graph_size = (num_commits as usize) * commit_graph_entry_size;
        let commit_lookup_entry_size = CommitLookupEntry::size(hash_length);
        let lookup_size = (num_commits as usize) * commit_lookup_entry_size;
        let parent_overflow_size = (num_parent_overflow_entries as usize) * 4;
        let predecessor_overflow_size = (num_predecessor_overflow_entries as usize) * 4;
        let expected_size =
            graph_size + lookup_size + parent_overflow_size + predecessor_overflow_size;
        assert_eq!(data.len(), expected_size);
        let overflow_predecessor = data.split_off(graph_size + lookup_size + parent_overflow_size);
        let overflow_parent = data.split_off(graph_size + lookup_size);
        let lookup = data.split_off(graph_size);
        let graph = data;
        Ok(Arc::new(ReadonlyIndex {
            dir,
            parent_file: maybe_parent_file,
            num_parent_commits,
            name,
            hash_length,
            commit_graph_entry_size,
            commit_lookup_entry_size,
            num_local_commits: num_commits,
            graph,
            lookup,
            overflow_parent,
            overflow_predecessor,
        }))
    }

    fn load_at_operation(
        dir: PathBuf,
        hash_length: usize,
        op_id: &OperationId,
    ) -> io::Result<Arc<ReadonlyIndex>> {
        let op_id_file = dir.join("operations").join(op_id.hex());
        let mut buf = vec![];
        File::open(op_id_file)
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        let index_file_id_hex = String::from_utf8(buf).unwrap();
        let index_file_path = dir.join(&index_file_id_hex);
        let mut index_file = File::open(&index_file_path).unwrap();
        ReadonlyIndex::load_from(&mut index_file, dir, index_file_id_hex, hash_length)
    }

    fn index(
        store: &StoreWrapper,
        dir: PathBuf,
        operation: &Operation,
    ) -> io::Result<Arc<ReadonlyIndex>> {
        let view = operation.view();
        let operations_dir = dir.join("operations");
        let hash_length = store.hash_length();
        let mut new_heads = view.heads().clone();
        let mut parent_op_id: Option<OperationId> = None;
        for op in dag_walk::bfs(
            vec![operation.clone()],
            Box::new(|op: &Operation| op.id().clone()),
            Box::new(|op: &Operation| op.parents()),
        ) {
            if operations_dir.join(op.id().hex()).is_file() {
                if parent_op_id.is_none() {
                    parent_op_id = Some(op.id().clone())
                }
            } else {
                for head in op.view().heads() {
                    new_heads.insert(head.clone());
                }
            }
        }
        let mut data;
        let maybe_parent_file;
        match parent_op_id {
            None => {
                maybe_parent_file = None;
                data = MutableIndex::full(dir, hash_length);
            }
            Some(parent_op_id) => {
                let parent_file =
                    ReadonlyIndex::load_at_operation(dir, hash_length, &parent_op_id).unwrap();
                maybe_parent_file = Some(parent_file.clone());
                data = MutableIndex::incremental(parent_file)
            }
        }

        let mut heads: Vec<CommitId> = new_heads.into_iter().collect();
        heads.sort();
        let commits = topo_order_earlier_first(store, heads, maybe_parent_file);

        for commit in &commits {
            data.add_commit(&commit);
        }

        let index_file = data.save()?;

        index_file.associate_with_operation(operation.id())?;

        Ok(index_file)
    }

    /// Records a link from the given operation to the this index version.
    pub fn associate_with_operation(&self, op_id: &OperationId) -> io::Result<()> {
        let mut temp_file = NamedTempFile::new_in(&self.dir)?;
        let file = temp_file.as_file_mut();
        file.write_all(&self.name.as_bytes()).unwrap();
        temp_file.persist(&self.dir.join("operations").join(op_id.hex()))?;
        Ok(())
    }

    pub fn num_commits(&self) -> u32 {
        CompositeIndex(self).num_commits()
    }

    pub fn stats(&self) -> IndexStats {
        CompositeIndex(self).stats()
    }

    pub fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<u32> {
        CompositeIndex(self).commit_id_to_pos(commit_id)
    }

    pub fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution {
        CompositeIndex(self).resolve_prefix(prefix)
    }

    pub fn entry_by_id(&self, commit_id: &CommitId) -> Option<IndexEntry> {
        CompositeIndex(self).entry_by_id(commit_id)
    }

    pub fn entry_by_pos(&self, pos: u32) -> IndexEntry {
        CompositeIndex(self).entry_by_pos(pos)
    }

    pub fn has_id(&self, commit_id: &CommitId) -> bool {
        CompositeIndex(self).has_id(commit_id)
    }

    pub fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool {
        CompositeIndex(self).is_ancestor(ancestor_id, descendant_id)
    }

    pub fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId> {
        CompositeIndex(self).common_ancestors(set1, set2)
    }

    pub fn walk_revs(&self, wanted: &[CommitId], unwanted: &[CommitId]) -> RevWalk {
        CompositeIndex(self).walk_revs(wanted, unwanted)
    }

    pub fn heads<'candidates>(
        &self,
        candidates: impl IntoIterator<Item = &'candidates CommitId>,
    ) -> Vec<CommitId> {
        CompositeIndex(self).heads(candidates)
    }

    fn graph_entry(&self, local_pos: u32) -> CommitGraphEntry {
        let offset = (local_pos as usize) * self.commit_graph_entry_size;
        CommitGraphEntry {
            data: &self.graph[offset..offset + self.commit_graph_entry_size],
            hash_length: self.hash_length,
        }
    }

    fn lookup_entry(&self, lookup_pos: u32) -> CommitLookupEntry {
        let offset = (lookup_pos as usize) * self.commit_lookup_entry_size;
        CommitLookupEntry {
            data: &self.lookup[offset..offset + self.commit_lookup_entry_size],
            hash_length: self.hash_length,
        }
    }

    fn overflow_parent(&self, overflow_pos: u32) -> u32 {
        let offset = (overflow_pos as usize) * 4;
        (&self.overflow_parent[offset..offset + 4])
            .read_u32::<LittleEndian>()
            .unwrap()
    }

    fn overflow_predecessor(&self, overflow_pos: u32) -> u32 {
        let offset = (overflow_pos as usize) * 4;
        (&self.overflow_predecessor[offset..offset + 4])
            .read_u32::<LittleEndian>()
            .unwrap()
    }

    fn commit_id_byte_prefix_to_pos(&self, prefix: &CommitId) -> Option<u32> {
        if self.num_local_commits == 0 {
            // Avoid overflow when subtracting 1 below
            return None;
        }
        let mut low = 0;
        let mut high = self.num_local_commits - 1;
        let prefix_len = prefix.0.len();

        // binary search for the commit id
        loop {
            let mid = (low + high) / 2;
            let entry = self.lookup_entry(mid);
            let entry_commit_id = entry.commit_id();
            let entry_prefix = &entry_commit_id.0[0..prefix_len];
            if high == low {
                return Some(mid);
            }
            if entry_prefix < prefix.0.as_slice() {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn index_empty(on_disk: bool) {
        let temp_dir = tempfile::tempdir().unwrap();
        let index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        let index = if on_disk {
            IndexRef::Readonly(index.save().unwrap())
        } else {
            IndexRef::Mutable(&index)
        };

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 0);
        assert_eq!(stats.num_heads, 0);
        assert_eq!(stats.max_generation_number, 0);
        assert_eq!(stats.num_merges, 0);
        assert_eq!(index.num_commits(), 0);
        // Cannot find any commits
        assert!(index.entry_by_id(&CommitId::from_hex("000000")).is_none());
        assert!(index.entry_by_id(&CommitId::from_hex("aaa111")).is_none());
        assert!(index.entry_by_id(&CommitId::from_hex("ffffff")).is_none());
    }

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn index_root_commit(on_disk: bool) {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        let id_0 = CommitId::from_hex("000000");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        let index = if on_disk {
            IndexRef::Readonly(index.save().unwrap())
        } else {
            IndexRef::Mutable(&index)
        };

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 1);
        assert_eq!(stats.num_heads, 1);
        assert_eq!(stats.max_generation_number, 0);
        assert_eq!(stats.num_merges, 0);
        assert_eq!(index.num_commits(), 1);
        // Can find only the root commit
        assert_eq!(index.commit_id_to_pos(&id_0), Some(0));
        assert_eq!(index.commit_id_to_pos(&CommitId::from_hex("aaaaaa")), None);
        assert_eq!(index.commit_id_to_pos(&CommitId::from_hex("ffffff")), None);
        // Check properties of root entry
        let entry = index.entry_by_id(&id_0).unwrap();
        assert_eq!(entry.pos, 0);
        assert_eq!(entry.commit_id(), id_0);
        assert_eq!(entry.generation_number(), 0);
        assert_eq!(entry.num_parents(), 0);
        assert_eq!(entry.parent_positions(), Vec::<u32>::new());
        assert_eq!(entry.num_predecessors(), 0);
        assert_eq!(entry.predecessor_positions(), Vec::<u32>::new());
    }

    #[test]
    #[should_panic(expected = "parent commit is not indexed")]
    fn index_missing_parent_commit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        index.add_commit_data(id_1, false, vec![id_0], vec![]);
    }

    #[test_case(false, false; "full in memory")]
    #[test_case(false, true; "full on disk")]
    #[test_case(true, false; "incremental in memory")]
    #[test_case(true, true; "incremental on disk")]
    fn index_multiple_commits(incremental: bool, on_disk: bool) {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);

        // If testing incremental indexing, write the first three commits to one file
        // now and build the remainder as another segment on top.
        if incremental {
            let initial_file = index.save().unwrap();
            index = MutableIndex::incremental(initial_file);
        }

        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_3.clone(), false, vec![id_2.clone()], vec![]);
        index.add_commit_data(
            id_4.clone(),
            false,
            vec![id_1.clone()],
            vec![id_2.clone(), id_3.clone()],
        );
        index.add_commit_data(
            id_5.clone(),
            false,
            vec![id_4.clone(), id_2.clone()],
            vec![],
        );
        let index = if on_disk {
            IndexRef::Readonly(index.save().unwrap())
        } else {
            IndexRef::Mutable(&index)
        };

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 6);
        assert_eq!(stats.num_heads, 2);
        assert_eq!(stats.max_generation_number, 3);
        assert_eq!(stats.num_merges, 1);
        assert_eq!(index.num_commits(), 6);
        // Can find all the commits
        let entry_0 = index.entry_by_id(&id_0).unwrap();
        let entry_1 = index.entry_by_id(&id_1).unwrap();
        let entry_2 = index.entry_by_id(&id_2).unwrap();
        let entry_3 = index.entry_by_id(&id_3).unwrap();
        let entry_4 = index.entry_by_id(&id_4).unwrap();
        let entry_5 = index.entry_by_id(&id_5).unwrap();
        // Check properties of some entries
        assert_eq!(entry_0.pos, 0);
        assert_eq!(entry_0.commit_id(), id_0);
        assert_eq!(entry_1.pos, 1);
        assert_eq!(entry_1.commit_id(), id_1);
        assert_eq!(entry_1.generation_number(), 1);
        assert_eq!(entry_1.num_parents(), 1);
        assert_eq!(entry_1.parent_positions(), vec![0]);
        assert_eq!(entry_1.num_predecessors(), 0);
        assert_eq!(entry_1.predecessor_positions(), Vec::<u32>::new());
        assert_eq!(entry_2.pos, 2);
        assert_eq!(entry_2.commit_id(), id_2);
        assert_eq!(entry_2.generation_number(), 1);
        assert_eq!(entry_2.num_parents(), 1);
        assert_eq!(entry_2.parent_positions(), vec![0]);
        assert_eq!(entry_3.generation_number(), 2);
        assert_eq!(entry_3.parent_positions(), vec![2]);
        assert_eq!(entry_4.pos, 4);
        assert_eq!(entry_4.generation_number(), 2);
        assert_eq!(entry_4.num_parents(), 1);
        assert_eq!(entry_4.parent_positions(), vec![1]);
        assert_eq!(entry_4.num_predecessors(), 2);
        assert_eq!(entry_4.predecessor_positions(), vec![2, 3]);
        assert_eq!(entry_5.generation_number(), 3);
        assert_eq!(entry_5.num_parents(), 2);
        assert_eq!(entry_5.parent_positions(), vec![4, 2]);
        assert_eq!(entry_5.num_predecessors(), 0);
        assert_eq!(entry_5.predecessor_positions(), Vec::<u32>::new());
    }

    #[test_case(false; "in memory")]
    #[test_case(true; "on disk")]
    fn index_many_parents(on_disk: bool) {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        //     6
        //    /|\
        //   / | \
        //  / /|\ \
        // 1 2 3 4 5
        //  \ \|/ /
        //   \ | /
        //    \|/
        //     0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_3.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_4.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_5.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(
            id_6.clone(),
            false,
            vec![
                id_1.clone(),
                id_2.clone(),
                id_3.clone(),
                id_4.clone(),
                id_5.clone(),
            ],
            vec![],
        );
        let index = if on_disk {
            IndexRef::Readonly(index.save().unwrap())
        } else {
            IndexRef::Mutable(&index)
        };

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 7);
        assert_eq!(stats.num_heads, 1);
        assert_eq!(stats.max_generation_number, 2);
        assert_eq!(stats.num_merges, 1);

        // The octopus merge has the right parents
        let entry_6 = index.entry_by_id(&id_6).unwrap();
        assert_eq!(entry_6.commit_id(), id_6.clone());
        assert_eq!(entry_6.num_parents(), 5);
        assert_eq!(entry_6.parent_positions(), vec![1, 2, 3, 4, 5]);
        assert_eq!(entry_6.generation_number(), 2);
    }

    #[test]
    fn resolve_prefix() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);

        // Create some commits with different various common prefixes.
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("009999");
        let id_2 = CommitId::from_hex("055488");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![], vec![]);

        // Write the first three commits to one file and build the remainder on top.
        let initial_file = index.save().unwrap();
        index = MutableIndex::incremental(initial_file);

        let id_3 = CommitId::from_hex("055444");
        let id_4 = CommitId::from_hex("055555");
        let id_5 = CommitId::from_hex("033333");
        index.add_commit_data(id_3.clone(), false, vec![], vec![]);
        index.add_commit_data(id_4.clone(), false, vec![], vec![]);
        index.add_commit_data(id_5.clone(), false, vec![], vec![]);

        // Can find commits given the full hex number
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new(id_0.hex())),
            PrefixResolution::SingleMatch(id_0.clone())
        );
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new(id_1.hex())),
            PrefixResolution::SingleMatch(id_1.clone())
        );
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new(id_2.hex())),
            PrefixResolution::SingleMatch(id_2.clone())
        );
        // Test non-existent commits
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new("ffffff".to_string())),
            PrefixResolution::NoMatch
        );
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new("000001".to_string())),
            PrefixResolution::NoMatch
        );
        // Test ambiguous prefix
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new("0".to_string())),
            PrefixResolution::AmbiguousMatch
        );
        // Test a globally unique prefix in initial part
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new("009".to_string())),
            PrefixResolution::SingleMatch(CommitId::from_hex("009999"))
        );
        // Test a globally unique prefix in incremental part
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new("03".to_string())),
            PrefixResolution::SingleMatch(CommitId::from_hex("033333"))
        );
        // Test a locally unique but globally ambiguous prefix
        assert_eq!(
            index.resolve_prefix(&HexPrefix::new("0554".to_string())),
            PrefixResolution::AmbiguousMatch
        );
    }
    #[test]
    fn test_is_ancestor() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_3.clone(), false, vec![id_2.clone()], vec![]);
        index.add_commit_data(id_4.clone(), false, vec![id_1.clone()], vec![]);
        index.add_commit_data(
            id_5.clone(),
            false,
            vec![id_4.clone(), id_2.clone()],
            vec![],
        );

        assert!(index.is_ancestor(&id_0, &id_0));
        assert!(index.is_ancestor(&id_0, &id_1));
        assert!(index.is_ancestor(&id_2, &id_3));
        assert!(index.is_ancestor(&id_2, &id_5));
        assert!(index.is_ancestor(&id_1, &id_5));
        assert!(index.is_ancestor(&id_0, &id_5));
        assert!(!index.is_ancestor(&id_1, &id_0));
        assert!(!index.is_ancestor(&id_5, &id_3));
        assert!(!index.is_ancestor(&id_3, &id_5));
        assert!(!index.is_ancestor(&id_2, &id_4));
        assert!(!index.is_ancestor(&id_4, &id_2));
    }

    #[test]
    fn test_common_ancestors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 5
        // |\
        // 4 |
        // | |
        // 1 2 3
        // | |/
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_3.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_4.clone(), false, vec![id_1.clone()], vec![]);
        index.add_commit_data(
            id_5.clone(),
            false,
            vec![id_4.clone(), id_2.clone()],
            vec![],
        );

        assert_eq!(
            index.common_ancestors(&[id_0.clone()], &[id_0.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_5.clone()], &[id_5.clone()]),
            vec![id_5.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone()], &[id_2.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_2.clone()], &[id_1.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone()], &[id_4.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_4.clone()], &[id_1.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_3.clone()], &[id_5.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_5.clone()], &[id_3.clone()]),
            vec![id_0.clone()]
        );

        // With multiple commits in an input set
        assert_eq!(
            index.common_ancestors(&[id_0.clone(), id_1.clone()], &[id_0.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_0.clone(), id_1.clone()], &[id_1.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_1.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_4.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_5.clone()]),
            vec![id_1.clone(), id_2.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_3.clone()]),
            vec![id_0.clone()]
        );
    }

    #[test]
    fn test_common_ancestors_criss_cross() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 3 4
        // |X|
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(
            id_3.clone(),
            false,
            vec![id_1.clone(), id_2.clone()],
            vec![],
        );
        index.add_commit_data(
            id_4.clone(),
            false,
            vec![id_1.clone(), id_2.clone()],
            vec![],
        );

        let mut common_ancestors = index.common_ancestors(&[id_3.clone()], &[id_4.clone()]);
        common_ancestors.sort();
        assert_eq!(common_ancestors, vec![id_1.clone(), id_2.clone()]);
    }

    #[test]
    fn test_common_ancestors_merge_with_ancestor() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 4   5
        // |\ /|
        // 1 2 3
        //  \|/
        //   0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_3.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(
            id_4.clone(),
            false,
            vec![id_0.clone(), id_2.clone()],
            vec![],
        );
        index.add_commit_data(
            id_5.clone(),
            false,
            vec![id_0.clone(), id_2.clone()],
            vec![],
        );

        let mut common_ancestors = index.common_ancestors(&[id_4.clone()], &[id_5.clone()]);
        common_ancestors.sort();
        assert_eq!(common_ancestors, vec![id_2.clone()]);
    }

    #[test]
    fn test_walk_revs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_3.clone(), false, vec![id_2.clone()], vec![]);
        index.add_commit_data(id_4.clone(), false, vec![id_1.clone()], vec![]);
        index.add_commit_data(
            id_5.clone(),
            false,
            vec![id_4.clone(), id_2.clone()],
            vec![],
        );

        // No wanted commits
        let revs: Vec<CommitId> = index.walk_revs(&[], &[]).collect();
        assert!(revs.is_empty());
        // Simple linear walk to roo
        let revs: Vec<CommitId> = index.walk_revs(&[id_4.clone()], &[]).collect();
        assert_eq!(revs, vec![id_4.clone(), id_1.clone(), id_0.clone()]);
        // Commits that are both wanted and unwanted are not walked
        let revs: Vec<CommitId> = index.walk_revs(&[id_0.clone()], &[id_0.clone()]).collect();
        assert_eq!(revs, vec![]);
        // Commits that are listed twice are only walked once
        let revs: Vec<CommitId> = index
            .walk_revs(&[id_0.clone(), id_0.clone()], &[])
            .collect();
        assert_eq!(revs, vec![id_0.clone()]);
        // If a commit and its ancestor are both wanted, the ancestor still gets walked
        // only once
        let revs: Vec<CommitId> = index
            .walk_revs(&[id_0.clone(), id_1.clone()], &[])
            .collect();
        assert_eq!(revs, vec![id_1.clone(), id_0.clone()]);
        // Ancestors of both wanted and unwanted commits are not walked
        let revs: Vec<CommitId> = index.walk_revs(&[id_2.clone()], &[id_1.clone()]).collect();
        assert_eq!(revs, vec![id_2.clone()]);
        // Same as above, but the opposite order, to make sure that order in index
        // doesn't matter
        let revs: Vec<CommitId> = index.walk_revs(&[id_1.clone()], &[id_2.clone()]).collect();
        assert_eq!(revs, vec![id_1.clone()]);
        // Two wanted nodes
        let revs: Vec<CommitId> = index
            .walk_revs(&[id_1.clone(), id_2.clone()], &[])
            .collect();
        assert_eq!(revs, vec![id_2.clone(), id_1.clone(), id_0.clone()]);
        // Order of output doesn't depend on order of input
        let revs: Vec<CommitId> = index
            .walk_revs(&[id_2.clone(), id_1.clone()], &[])
            .collect();
        assert_eq!(revs, vec![id_2.clone(), id_1.clone(), id_0]);
        // Two wanted nodes that share an unwanted ancestor
        let revs: Vec<CommitId> = index
            .walk_revs(&[id_5.clone(), id_3.clone()], &[id_2])
            .collect();
        assert_eq!(revs, vec![id_5, id_4, id_3, id_1]);
    }

    #[test]
    fn test_heads() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = MutableIndex::full(temp_dir.path().to_owned(), 3);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), false, vec![], vec![]);
        index.add_commit_data(id_1.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_2.clone(), false, vec![id_0.clone()], vec![]);
        index.add_commit_data(id_3.clone(), false, vec![id_2.clone()], vec![]);
        index.add_commit_data(id_4.clone(), false, vec![id_1.clone()], vec![]);
        index.add_commit_data(
            id_5.clone(),
            false,
            vec![id_4.clone(), id_2.clone()],
            vec![],
        );

        // Empty input
        assert!(index.heads(&[]).is_empty());
        // Single head
        assert_eq!(index.heads(&[id_4.clone()]), vec![id_4.clone()]);
        // Single head and parent
        assert_eq!(index.heads(&[id_4.clone(), id_1]), vec![id_4.clone()]);
        // Single head and grand-parent
        assert_eq!(index.heads(&[id_4.clone(), id_0]), vec![id_4.clone()]);
        // Multiple heads
        assert_eq!(
            index.heads(&[id_4.clone(), id_3.clone()]),
            vec![id_3.clone(), id_4]
        );
        // Merge commit and ancestors
        assert_eq!(index.heads(&[id_5.clone(), id_2]), vec![id_5.clone()]);
        // Merge commit and other commit
        assert_eq!(index.heads(&[id_5.clone(), id_3.clone()]), vec![id_3, id_5]);
    }
}
