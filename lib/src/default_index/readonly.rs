// Copyright 2023 The Jujutsu Authors
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
use std::cmp::Ordering;
use std::fmt::{Debug, Formatter};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use byteorder::{LittleEndian, ReadBytesExt};
use smallvec::SmallVec;

use super::composite::CompositeIndex;
use super::mutable::DefaultMutableIndex;
use super::store::IndexLoadError;
use super::{IndexEntry, IndexPosition, IndexSegment, SmallIndexPositionsVec};
use crate::backend::{ChangeId, CommitId, ObjectId};
use crate::index::{HexPrefix, Index, MutableIndex, PrefixResolution, ReadonlyIndex};
use crate::revset::{ResolvedExpression, Revset, RevsetEvaluationError};
use crate::store::Store;

struct CommitGraphEntry<'a> {
    data: &'a [u8],
    commit_id_length: usize,
    change_id_length: usize,
}

// TODO: Add pointers to ancestors further back, like a skip list. Clear the
// lowest set bit to determine which generation number the pointers point to.
impl CommitGraphEntry<'_> {
    fn size(commit_id_length: usize, change_id_length: usize) -> usize {
        20 + commit_id_length + change_id_length
    }

    fn generation_number(&self) -> u32 {
        (&self.data[4..]).read_u32::<LittleEndian>().unwrap()
    }

    fn num_parents(&self) -> u32 {
        (&self.data[8..]).read_u32::<LittleEndian>().unwrap()
    }

    fn parent1_pos(&self) -> IndexPosition {
        IndexPosition((&self.data[12..]).read_u32::<LittleEndian>().unwrap())
    }

    fn parent2_overflow_pos(&self) -> u32 {
        (&self.data[16..]).read_u32::<LittleEndian>().unwrap()
    }

    // TODO: Consider storing the change ids in a separate table. That table could
    // be sorted by change id and have the end index into a list as value. That list
    // would be the concatenation of all index positions associated with the change.
    // Possible advantages: avoids duplicating change ids; smaller main graph leads
    // to better cache locality when walking it; ability to quickly find all
    // commits associated with a change id.
    fn change_id(&self) -> ChangeId {
        ChangeId::new(self.data[20..][..self.change_id_length].to_vec())
    }

    fn commit_id(&self) -> CommitId {
        CommitId::from_bytes(&self.data[20 + self.change_id_length..][..self.commit_id_length])
    }
}

struct CommitLookupEntry<'a> {
    data: &'a [u8],
    commit_id_length: usize,
}

impl CommitLookupEntry<'_> {
    fn size(commit_id_length: usize) -> usize {
        commit_id_length + 4
    }

    fn commit_id(&self) -> CommitId {
        CommitId::from_bytes(self.commit_id_bytes())
    }

    // might be better to add borrowed version of CommitId
    fn commit_id_bytes(&self) -> &[u8] {
        &self.data[0..self.commit_id_length]
    }

    fn pos(&self) -> IndexPosition {
        IndexPosition(
            (&self.data[self.commit_id_length..][..4])
                .read_u32::<LittleEndian>()
                .unwrap(),
        )
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
pub(super) struct ReadonlyIndexSegment {
    parent_file: Option<Arc<ReadonlyIndexSegment>>,
    pub(super) num_parent_commits: u32,
    pub(super) name: String,
    pub(super) commit_id_length: usize,
    pub(super) change_id_length: usize,
    commit_graph_entry_size: usize,
    commit_lookup_entry_size: usize,
    // Number of commits not counting the parent file
    pub(super) num_local_commits: u32,
    graph: Vec<u8>,
    lookup: Vec<u8>,
    overflow_parent: Vec<u8>,
}

impl Debug for ReadonlyIndexSegment {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("ReadonlyIndexSegment")
            .field("name", &self.name)
            .field("parent_file", &self.parent_file)
            .finish()
    }
}

impl ReadonlyIndexSegment {
    pub(super) fn load_from(
        file: &mut dyn Read,
        dir: PathBuf,
        name: String,
        commit_id_length: usize,
        change_id_length: usize,
    ) -> Result<Arc<ReadonlyIndexSegment>, IndexLoadError> {
        let parent_filename_len = file.read_u32::<LittleEndian>()?;
        let num_parent_commits;
        let maybe_parent_file;
        if parent_filename_len > 0 {
            let mut parent_filename_bytes = vec![0; parent_filename_len as usize];
            file.read_exact(&mut parent_filename_bytes)?;
            let parent_filename = String::from_utf8(parent_filename_bytes).unwrap();
            let parent_file_path = dir.join(&parent_filename);
            let mut index_file = File::open(parent_file_path).unwrap();
            let parent_file = ReadonlyIndexSegment::load_from(
                &mut index_file,
                dir,
                parent_filename,
                commit_id_length,
                change_id_length,
            )?;
            num_parent_commits = parent_file.num_parent_commits + parent_file.num_local_commits;
            maybe_parent_file = Some(parent_file);
        } else {
            num_parent_commits = 0;
            maybe_parent_file = None;
        };
        let num_commits = file.read_u32::<LittleEndian>()?;
        let num_parent_overflow_entries = file.read_u32::<LittleEndian>()?;
        let mut data = vec![];
        file.read_to_end(&mut data)?;
        let commit_graph_entry_size = CommitGraphEntry::size(commit_id_length, change_id_length);
        let graph_size = (num_commits as usize) * commit_graph_entry_size;
        let commit_lookup_entry_size = CommitLookupEntry::size(commit_id_length);
        let lookup_size = (num_commits as usize) * commit_lookup_entry_size;
        let parent_overflow_size = (num_parent_overflow_entries as usize) * 4;
        let expected_size = graph_size + lookup_size + parent_overflow_size;
        if data.len() != expected_size {
            return Err(IndexLoadError::IndexCorrupt(name));
        }
        let overflow_parent = data.split_off(graph_size + lookup_size);
        let lookup = data.split_off(graph_size);
        let graph = data;
        Ok(Arc::new(ReadonlyIndexSegment {
            parent_file: maybe_parent_file,
            num_parent_commits,
            name,
            commit_id_length,
            change_id_length,
            commit_graph_entry_size,
            commit_lookup_entry_size,
            num_local_commits: num_commits,
            graph,
            lookup,
            overflow_parent,
        }))
    }

    pub(super) fn as_composite(&self) -> CompositeIndex {
        CompositeIndex::new(self)
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    fn graph_entry(&self, local_pos: u32) -> CommitGraphEntry {
        let offset = (local_pos as usize) * self.commit_graph_entry_size;
        CommitGraphEntry {
            data: &self.graph[offset..][..self.commit_graph_entry_size],
            commit_id_length: self.commit_id_length,
            change_id_length: self.change_id_length,
        }
    }

    fn lookup_entry(&self, lookup_pos: u32) -> CommitLookupEntry {
        let offset = (lookup_pos as usize) * self.commit_lookup_entry_size;
        CommitLookupEntry {
            data: &self.lookup[offset..][..self.commit_lookup_entry_size],
            commit_id_length: self.commit_id_length,
        }
    }

    fn overflow_parent(&self, overflow_pos: u32) -> IndexPosition {
        let offset = (overflow_pos as usize) * 4;
        IndexPosition(
            (&self.overflow_parent[offset..][..4])
                .read_u32::<LittleEndian>()
                .unwrap(),
        )
    }

    fn commit_id_byte_prefix_to_lookup_pos(&self, prefix: &CommitId) -> Option<u32> {
        if self.num_local_commits == 0 {
            // Avoid overflow when subtracting 1 below
            return None;
        }
        let mut low = 0;
        let mut high = self.num_local_commits - 1;

        // binary search for the commit id
        loop {
            let mid = (low + high) / 2;
            if high == low {
                return Some(mid);
            }
            let entry = self.lookup_entry(mid);
            if entry.commit_id_bytes() < prefix.as_bytes() {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
    }
}

impl IndexSegment for ReadonlyIndexSegment {
    fn segment_num_parent_commits(&self) -> u32 {
        self.num_parent_commits
    }

    fn segment_num_commits(&self) -> u32 {
        self.num_local_commits
    }

    fn segment_parent_file(&self) -> Option<&Arc<ReadonlyIndexSegment>> {
        self.parent_file.as_ref()
    }

    fn segment_name(&self) -> Option<String> {
        Some(self.name.clone())
    }

    fn segment_commit_id_to_pos(&self, commit_id: &CommitId) -> Option<IndexPosition> {
        let lookup_pos = self.commit_id_byte_prefix_to_lookup_pos(commit_id)?;
        let entry = self.lookup_entry(lookup_pos);
        (&entry.commit_id() == commit_id).then(|| entry.pos())
    }

    fn segment_commit_id_to_neighbor_positions(
        &self,
        commit_id: &CommitId,
    ) -> (Option<IndexPosition>, Option<IndexPosition>) {
        if let Some(lookup_pos) = self.commit_id_byte_prefix_to_lookup_pos(commit_id) {
            let entry_commit_id = self.lookup_entry(lookup_pos).commit_id();
            let (prev_lookup_pos, next_lookup_pos) = match entry_commit_id.cmp(commit_id) {
                Ordering::Less => {
                    assert_eq!(lookup_pos + 1, self.num_local_commits);
                    (Some(lookup_pos), None)
                }
                Ordering::Equal => {
                    let succ = ((lookup_pos + 1)..self.num_local_commits).next();
                    (lookup_pos.checked_sub(1), succ)
                }
                Ordering::Greater => (lookup_pos.checked_sub(1), Some(lookup_pos)),
            };
            let prev_pos = prev_lookup_pos.map(|p| self.lookup_entry(p).pos());
            let next_pos = next_lookup_pos.map(|p| self.lookup_entry(p).pos());
            (prev_pos, next_pos)
        } else {
            (None, None)
        }
    }

    fn segment_resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        let min_bytes_prefix = CommitId::from_bytes(prefix.min_prefix_bytes());
        let lookup_pos = self
            .commit_id_byte_prefix_to_lookup_pos(&min_bytes_prefix)
            .unwrap_or(self.num_local_commits);
        let mut matches = (lookup_pos..self.num_local_commits)
            .map(|pos| self.lookup_entry(pos).commit_id())
            .take_while(|id| prefix.matches(id))
            .fuse();
        match (matches.next(), matches.next()) {
            (Some(id), None) => PrefixResolution::SingleMatch(id),
            (Some(_), Some(_)) => PrefixResolution::AmbiguousMatch,
            (None, _) => PrefixResolution::NoMatch,
        }
    }

    fn segment_generation_number(&self, local_pos: u32) -> u32 {
        self.graph_entry(local_pos).generation_number()
    }

    fn segment_commit_id(&self, local_pos: u32) -> CommitId {
        self.graph_entry(local_pos).commit_id()
    }

    fn segment_change_id(&self, local_pos: u32) -> ChangeId {
        self.graph_entry(local_pos).change_id()
    }

    fn segment_num_parents(&self, local_pos: u32) -> u32 {
        self.graph_entry(local_pos).num_parents()
    }

    fn segment_parent_positions(&self, local_pos: u32) -> SmallIndexPositionsVec {
        let graph_entry = self.graph_entry(local_pos);
        let mut parent_entries = SmallVec::with_capacity(graph_entry.num_parents() as usize);
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

    fn segment_entry_by_pos(&self, pos: IndexPosition, local_pos: u32) -> IndexEntry {
        IndexEntry {
            source: self,
            local_pos,
            pos,
        }
    }
}

/// Commit index backend which stores data on local disk.
#[derive(Debug)]
pub struct DefaultReadonlyIndex(pub(super) Arc<ReadonlyIndexSegment>);

impl DefaultReadonlyIndex {
    pub fn as_composite(&self) -> CompositeIndex {
        self.0.as_composite()
    }
}

impl Index for DefaultReadonlyIndex {
    fn shortest_unique_commit_id_prefix_len(&self, commit_id: &CommitId) -> usize {
        self.as_composite()
            .shortest_unique_commit_id_prefix_len(commit_id)
    }

    fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        self.as_composite().resolve_prefix(prefix)
    }

    fn has_id(&self, commit_id: &CommitId) -> bool {
        self.as_composite().has_id(commit_id)
    }

    fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool {
        self.as_composite().is_ancestor(ancestor_id, descendant_id)
    }

    fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId> {
        self.as_composite().common_ancestors(set1, set2)
    }

    fn heads(&self, candidates: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId> {
        self.as_composite().heads(candidates)
    }

    fn topo_order(&self, input: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId> {
        self.as_composite().topo_order(input)
    }

    fn evaluate_revset<'index>(
        &'index self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset<'index> + 'index>, RevsetEvaluationError> {
        self.as_composite().evaluate_revset(expression, store)
    }
}

impl ReadonlyIndex for DefaultReadonlyIndex {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_index(&self) -> &dyn Index {
        self
    }

    fn start_modification(&self) -> Box<dyn MutableIndex> {
        Box::new(DefaultMutableIndex::incremental(self.0.clone()))
    }
}
