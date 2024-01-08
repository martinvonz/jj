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
use std::io;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;
use thiserror::Error;

use super::composite::{AsCompositeIndex, ChangeIdIndexImpl, CompositeIndex, IndexSegment};
use super::entry::{IndexPosition, LocalPosition, SmallIndexPositionsVec};
use super::mutable::DefaultMutableIndex;
use crate::backend::{ChangeId, CommitId};
use crate::index::{ChangeIdIndex, Index, MutableIndex, ReadonlyIndex};
use crate::object_id::{HexPrefix, ObjectId, PrefixResolution};
use crate::revset::{ResolvedExpression, Revset, RevsetEvaluationError};
use crate::store::Store;

/// Error while loading index segment file.
#[derive(Debug, Error)]
#[error("Failed to load commit index file '{name}': {error}")]
pub struct ReadonlyIndexLoadError {
    /// Index file name.
    pub name: String,
    /// Underlying error.
    #[source]
    pub error: io::Error,
}

impl ReadonlyIndexLoadError {
    fn invalid_data(
        name: impl Into<String>,
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::from_io_err(name, io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn from_io_err(name: impl Into<String>, error: io::Error) -> Self {
        ReadonlyIndexLoadError {
            name: name.into(),
            error,
        }
    }

    /// Returns true if the underlying error suggests data corruption.
    pub(super) fn is_corrupt_or_not_found(&self) -> bool {
        // If the parent file name field is corrupt, the file wouldn't be found.
        // And there's no need to distinguish it from an empty file.
        matches!(
            self.error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
        )
    }
}

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
        u32::from_le_bytes(self.data[4..8].try_into().unwrap())
    }

    fn num_parents(&self) -> u32 {
        u32::from_le_bytes(self.data[8..12].try_into().unwrap())
    }

    fn parent1_pos(&self) -> IndexPosition {
        IndexPosition(u32::from_le_bytes(self.data[12..16].try_into().unwrap()))
    }

    fn parent2_overflow_pos(&self) -> u32 {
        u32::from_le_bytes(self.data[16..20].try_into().unwrap())
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
        let pos = u32::from_le_bytes(self.data[self.commit_id_length..][..4].try_into().unwrap());
        IndexPosition(pos)
    }
}

/// Commit index segment backed by immutable file.
///
/// File format:
/// ```text
/// u32: parent segment file name length (0 means root)
/// <length number of bytes>: parent segment file name
///
/// u32: number of local entries
/// u32: number of overflow parent entries
/// for each entry, in some topological order with parents first:
///   u32: flags (unused)
///   u32: generation number
///   u32: number of parents
///   u32: global index position for parent 1
///   u32: position in the overflow table of parent 2
///   <change id length number of bytes>: change id
///   <commit id length number of bytes>: commit id
/// for each entry, sorted by commit id:
///   <commit id length number of bytes>: commit id
///   u32: global index position
/// for each overflow parent:
///   u32: global index position
/// ```
///
/// Note that u32 fields are 4-byte aligned so long as the parent file name
/// (which is hexadecimal hash) and commit/change ids aren't of exotic length.
// TODO: add a version number
// TODO: replace the table by a trie so we don't have to repeat the full commit
//       ids
// TODO: add a fanout table like git's commit graph has?
pub(super) struct ReadonlyIndexSegment {
    parent_file: Option<Arc<ReadonlyIndexSegment>>,
    num_parent_commits: u32,
    name: String,
    commit_id_length: usize,
    change_id_length: usize,
    commit_graph_entry_size: usize,
    commit_lookup_entry_size: usize,
    // Number of commits not counting the parent file
    num_local_commits: u32,
    data: Vec<u8>,
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
    /// Loads both parent segments and local entries from the given file `name`.
    pub(super) fn load(
        dir: &Path,
        name: String,
        commit_id_length: usize,
        change_id_length: usize,
    ) -> Result<Arc<ReadonlyIndexSegment>, ReadonlyIndexLoadError> {
        let mut file = File::open(dir.join(&name))
            .map_err(|err| ReadonlyIndexLoadError::from_io_err(&name, err))?;
        Self::load_from(&mut file, dir, name, commit_id_length, change_id_length)
    }

    /// Loads both parent segments and local entries from the given `file`.
    pub(super) fn load_from(
        file: &mut dyn Read,
        dir: &Path,
        name: String,
        commit_id_length: usize,
        change_id_length: usize,
    ) -> Result<Arc<ReadonlyIndexSegment>, ReadonlyIndexLoadError> {
        let from_io_err = |err| ReadonlyIndexLoadError::from_io_err(&name, err);
        let read_u32 = |file: &mut dyn Read| {
            let mut buf = [0; 4];
            file.read_exact(&mut buf).map_err(from_io_err)?;
            Ok(u32::from_le_bytes(buf))
        };
        let parent_filename_len = read_u32(file)?;
        let maybe_parent_file = if parent_filename_len > 0 {
            let mut parent_filename_bytes = vec![0; parent_filename_len as usize];
            file.read_exact(&mut parent_filename_bytes)
                .map_err(from_io_err)?;
            let parent_filename = String::from_utf8(parent_filename_bytes).map_err(|_| {
                ReadonlyIndexLoadError::invalid_data(&name, "parent file name is not valid UTF-8")
            })?;
            let parent_file = ReadonlyIndexSegment::load(
                dir,
                parent_filename,
                commit_id_length,
                change_id_length,
            )?;
            Some(parent_file)
        } else {
            None
        };
        Self::load_with_parent_file(
            file,
            name,
            maybe_parent_file,
            commit_id_length,
            change_id_length,
        )
    }

    /// Loads local entries from the given `file`, returns new segment linked to
    /// the given `parent_file`.
    pub(super) fn load_with_parent_file(
        file: &mut dyn Read,
        name: String,
        parent_file: Option<Arc<ReadonlyIndexSegment>>,
        commit_id_length: usize,
        change_id_length: usize,
    ) -> Result<Arc<ReadonlyIndexSegment>, ReadonlyIndexLoadError> {
        let from_io_err = |err| ReadonlyIndexLoadError::from_io_err(&name, err);
        let read_u32 = |file: &mut dyn Read| {
            let mut buf = [0; 4];
            file.read_exact(&mut buf).map_err(from_io_err)?;
            Ok(u32::from_le_bytes(buf))
        };
        let num_parent_commits = parent_file
            .as_ref()
            .map_or(0, |segment| segment.as_composite().num_commits());
        let num_local_commits = read_u32(file)?;
        let num_parent_overflow_entries = read_u32(file)?;
        let mut data = vec![];
        file.read_to_end(&mut data).map_err(from_io_err)?;
        let commit_graph_entry_size = CommitGraphEntry::size(commit_id_length, change_id_length);
        let graph_size = (num_local_commits as usize) * commit_graph_entry_size;
        let commit_lookup_entry_size = CommitLookupEntry::size(commit_id_length);
        let commit_lookup_size = (num_local_commits as usize) * commit_lookup_entry_size;
        let parent_overflow_size = (num_parent_overflow_entries as usize) * 4;
        let expected_size = graph_size + commit_lookup_size + parent_overflow_size;
        if data.len() != expected_size {
            return Err(ReadonlyIndexLoadError::invalid_data(
                name,
                "unexpected data length",
            ));
        }
        Ok(Arc::new(ReadonlyIndexSegment {
            parent_file,
            num_parent_commits,
            name,
            commit_id_length,
            change_id_length,
            commit_graph_entry_size,
            commit_lookup_entry_size,
            num_local_commits,
            data,
        }))
    }

    pub(super) fn as_composite(&self) -> CompositeIndex {
        CompositeIndex::new(self)
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn commit_id_length(&self) -> usize {
        self.commit_id_length
    }

    pub(super) fn change_id_length(&self) -> usize {
        self.change_id_length
    }

    fn graph_entry(&self, local_pos: LocalPosition) -> CommitGraphEntry {
        assert!(local_pos.0 < self.num_local_commits);
        let offset = (local_pos.0 as usize) * self.commit_graph_entry_size;
        CommitGraphEntry {
            data: &self.data[offset..][..self.commit_graph_entry_size],
            commit_id_length: self.commit_id_length,
            change_id_length: self.change_id_length,
        }
    }

    fn commit_lookup_entry(&self, lookup_pos: u32) -> CommitLookupEntry {
        assert!(lookup_pos < self.num_local_commits);
        let offset = (lookup_pos as usize) * self.commit_lookup_entry_size
            + (self.num_local_commits as usize) * self.commit_graph_entry_size;
        CommitLookupEntry {
            data: &self.data[offset..][..self.commit_lookup_entry_size],
            commit_id_length: self.commit_id_length,
        }
    }

    fn overflow_parent(&self, overflow_pos: u32) -> IndexPosition {
        let offset = (overflow_pos as usize) * 4
            + (self.num_local_commits as usize) * self.commit_graph_entry_size
            + (self.num_local_commits as usize) * self.commit_lookup_entry_size;
        let pos = u32::from_le_bytes(self.data[offset..][..4].try_into().unwrap());
        IndexPosition(pos)
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
            let entry = self.commit_lookup_entry(mid);
            if entry.commit_id_bytes() < prefix.as_bytes() {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
    }
}

impl IndexSegment for ReadonlyIndexSegment {
    fn num_parent_commits(&self) -> u32 {
        self.num_parent_commits
    }

    fn num_local_commits(&self) -> u32 {
        self.num_local_commits
    }

    fn parent_file(&self) -> Option<&Arc<ReadonlyIndexSegment>> {
        self.parent_file.as_ref()
    }

    fn name(&self) -> Option<String> {
        Some(self.name.clone())
    }

    fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<IndexPosition> {
        let lookup_pos = self.commit_id_byte_prefix_to_lookup_pos(commit_id)?;
        let entry = self.commit_lookup_entry(lookup_pos);
        (&entry.commit_id() == commit_id).then(|| entry.pos())
    }

    fn resolve_neighbor_commit_ids(
        &self,
        commit_id: &CommitId,
    ) -> (Option<CommitId>, Option<CommitId>) {
        if let Some(lookup_pos) = self.commit_id_byte_prefix_to_lookup_pos(commit_id) {
            let entry_commit_id = self.commit_lookup_entry(lookup_pos).commit_id();
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
            let prev_id = prev_lookup_pos.map(|p| self.commit_lookup_entry(p).commit_id());
            let next_id = next_lookup_pos.map(|p| self.commit_lookup_entry(p).commit_id());
            (prev_id, next_id)
        } else {
            (None, None)
        }
    }

    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        let min_bytes_prefix = CommitId::from_bytes(prefix.min_prefix_bytes());
        let lookup_pos = self
            .commit_id_byte_prefix_to_lookup_pos(&min_bytes_prefix)
            .unwrap_or(self.num_local_commits);
        let mut matches = (lookup_pos..self.num_local_commits)
            .map(|pos| self.commit_lookup_entry(pos).commit_id())
            .take_while(|id| prefix.matches(id))
            .fuse();
        match (matches.next(), matches.next()) {
            (Some(id), None) => PrefixResolution::SingleMatch(id),
            (Some(_), Some(_)) => PrefixResolution::AmbiguousMatch,
            (None, _) => PrefixResolution::NoMatch,
        }
    }

    fn generation_number(&self, local_pos: LocalPosition) -> u32 {
        self.graph_entry(local_pos).generation_number()
    }

    fn commit_id(&self, local_pos: LocalPosition) -> CommitId {
        self.graph_entry(local_pos).commit_id()
    }

    fn change_id(&self, local_pos: LocalPosition) -> ChangeId {
        self.graph_entry(local_pos).change_id()
    }

    fn num_parents(&self, local_pos: LocalPosition) -> u32 {
        self.graph_entry(local_pos).num_parents()
    }

    fn parent_positions(&self, local_pos: LocalPosition) -> SmallIndexPositionsVec {
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
}

/// Commit index backend which stores data on local disk.
#[derive(Clone, Debug)]
pub struct DefaultReadonlyIndex(Arc<ReadonlyIndexSegment>);

impl DefaultReadonlyIndex {
    pub(super) fn from_segment(segment: Arc<ReadonlyIndexSegment>) -> Self {
        DefaultReadonlyIndex(segment)
    }

    pub(super) fn as_segment(&self) -> &Arc<ReadonlyIndexSegment> {
        &self.0
    }
}

impl AsCompositeIndex for DefaultReadonlyIndex {
    fn as_composite(&self) -> CompositeIndex<'_> {
        self.0.as_composite()
    }
}

impl Index for DefaultReadonlyIndex {
    fn shortest_unique_commit_id_prefix_len(&self, commit_id: &CommitId) -> usize {
        self.as_composite()
            .shortest_unique_commit_id_prefix_len(commit_id)
    }

    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        self.as_composite().resolve_commit_id_prefix(prefix)
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

    // TODO: Create a persistent lookup from change id to commit ids.
    fn change_id_index(
        &self,
        heads: &mut dyn Iterator<Item = &CommitId>,
    ) -> Box<dyn ChangeIdIndex> {
        Box::new(ChangeIdIndexImpl::new(self.clone(), heads))
    }

    fn start_modification(&self) -> Box<dyn MutableIndex> {
        Box::new(DefaultMutableIndex::incremental(self.0.clone()))
    }
}
