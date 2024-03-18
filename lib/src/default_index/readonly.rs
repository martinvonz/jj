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

use smallvec::smallvec;
use thiserror::Error;

use super::composite::{AsCompositeIndex, ChangeIdIndexImpl, CompositeIndex, IndexSegment};
use super::entry::{IndexPosition, LocalPosition, SmallIndexPositionsVec, SmallLocalPositionsVec};
use super::mutable::DefaultMutableIndex;
use crate::backend::{ChangeId, CommitId};
use crate::index::{AllHeadsForGcUnsupported, ChangeIdIndex, Index, MutableIndex, ReadonlyIndex};
use crate::object_id::{HexPrefix, ObjectId, PrefixResolution};
use crate::revset::{ResolvedExpression, Revset, RevsetEvaluationError};
use crate::store::Store;

/// Error while loading index segment file.
#[derive(Debug, Error)]
pub enum ReadonlyIndexLoadError {
    #[error("Unexpected index version")]
    UnexpectedVersion {
        found_version: u32,
        expected_version: u32,
    },
    #[error("Failed to load commit index file '{name}'")]
    Other {
        /// Index file name.
        name: String,
        /// Underlying error.
        #[source]
        error: io::Error,
    },
}

impl ReadonlyIndexLoadError {
    fn invalid_data(
        name: impl Into<String>,
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::from_io_err(name, io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn from_io_err(name: impl Into<String>, error: io::Error) -> Self {
        ReadonlyIndexLoadError::Other {
            name: name.into(),
            error,
        }
    }

    /// Returns true if the underlying error suggests data corruption.
    pub(super) fn is_corrupt_or_not_found(&self) -> bool {
        match self {
            ReadonlyIndexLoadError::UnexpectedVersion { .. } => true,
            ReadonlyIndexLoadError::Other { name: _, error } => {
                // If the parent file name field is corrupt, the file wouldn't be found.
                // And there's no need to distinguish it from an empty file.
                matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::InvalidData
                        | io::ErrorKind::UnexpectedEof
                )
            }
        }
    }
}

/// Current format version of the index segment file.
pub(crate) const INDEX_SEGMENT_FILE_FORMAT_VERSION: u32 = 6;

/// If set, the value is stored in the overflow table.
pub(crate) const OVERFLOW_FLAG: u32 = 0x8000_0000;

/// Global index position of parent entry, or overflow pointer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ParentIndexPosition(u32);

impl ParentIndexPosition {
    fn as_inlined(self) -> Option<IndexPosition> {
        (self.0 & OVERFLOW_FLAG == 0).then_some(IndexPosition(self.0))
    }

    fn as_overflow(self) -> Option<u32> {
        (self.0 & OVERFLOW_FLAG != 0).then_some(!self.0)
    }
}

/// Local position of entry pointed by change id, or overflow pointer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ChangeLocalPosition(u32);

impl ChangeLocalPosition {
    fn as_inlined(self) -> Option<LocalPosition> {
        (self.0 & OVERFLOW_FLAG == 0).then_some(LocalPosition(self.0))
    }

    fn as_overflow(self) -> Option<u32> {
        (self.0 & OVERFLOW_FLAG != 0).then_some(!self.0)
    }
}

struct CommitGraphEntry<'a> {
    data: &'a [u8],
}

// TODO: Add pointers to ancestors further back, like a skip list. Clear the
// lowest set bit to determine which generation number the pointers point to.
impl CommitGraphEntry<'_> {
    fn size(commit_id_length: usize) -> usize {
        16 + commit_id_length
    }

    fn generation_number(&self) -> u32 {
        u32::from_le_bytes(self.data[0..4].try_into().unwrap())
    }

    fn parent1_pos_or_overflow_pos(&self) -> ParentIndexPosition {
        ParentIndexPosition(u32::from_le_bytes(self.data[4..8].try_into().unwrap()))
    }

    fn parent2_pos_or_overflow_len(&self) -> ParentIndexPosition {
        ParentIndexPosition(u32::from_le_bytes(self.data[8..12].try_into().unwrap()))
    }

    fn change_id_lookup_pos(&self) -> u32 {
        u32::from_le_bytes(self.data[12..16].try_into().unwrap())
    }

    fn commit_id(&self) -> CommitId {
        CommitId::from_bytes(self.commit_id_bytes())
    }

    // might be better to add borrowed version of CommitId
    fn commit_id_bytes(&self) -> &[u8] {
        &self.data[16..]
    }
}

/// Commit index segment backed by immutable file.
///
/// File format:
/// ```text
/// u32: file format version
/// u32: parent segment file name length (0 means root)
/// <length number of bytes>: parent segment file name
///
/// u32: number of local commit entries
/// u32: number of local change ids
/// u32: number of overflow parent entries
/// u32: number of overflow change id positions
/// for each entry, in some topological order with parents first:
///   u32: generation number
///   if number of parents <= 2:
///     u32: (< 0x8000_0000) global index position for parent 1
///          (==0xffff_ffff) no parent 1
///     u32: (< 0x8000_0000) global index position for parent 2
///          (==0xffff_ffff) no parent 2
///   else:
///     u32: (>=0x8000_0000) position in the overflow table, bit-negated
///     u32: (>=0x8000_0000) number of parents (in the overflow table), bit-negated
///   u32: change id position in the sorted change ids table
///   <commit id length number of bytes>: commit id
/// for each entry, sorted by commit id:
///   u32: local position in the graph entries table
/// for each entry, sorted by change id:
///   <change id length number of bytes>: change id
/// for each entry, sorted by change id:
///   if number of associated commits == 1:
///     u32: (< 0x8000_0000) local position in the graph entries table
///   else:
///     u32: (>=0x8000_0000) position in the overflow table, bit-negated
/// for each overflow parent:
///   u32: global index position
/// for each overflow change id entry:
///   u32: local position in the graph entries table
/// ```
///
/// Note that u32 fields are 4-byte aligned so long as the parent file name
/// (which is hexadecimal hash) and commit/change ids aren't of exotic length.
// TODO: replace the table by a trie so we don't have to repeat the full commit
//       ids
// TODO: add a fanout table like git's commit graph has?
pub(super) struct ReadonlyIndexSegment {
    parent_file: Option<Arc<ReadonlyIndexSegment>>,
    num_parent_commits: u32,
    name: String,
    commit_id_length: usize,
    change_id_length: usize,
    // Number of commits not counting the parent file
    num_local_commits: u32,
    num_local_change_ids: u32,
    num_change_overflow_entries: u32,
    // Base data offsets in bytes:
    commit_lookup_base: usize,
    change_id_table_base: usize,
    change_pos_table_base: usize,
    parent_overflow_base: usize,
    change_overflow_base: usize,
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
        let format_version = read_u32(file)?;
        if format_version != INDEX_SEGMENT_FILE_FORMAT_VERSION {
            return Err(ReadonlyIndexLoadError::UnexpectedVersion {
                found_version: format_version,
                expected_version: INDEX_SEGMENT_FILE_FORMAT_VERSION,
            });
        }
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
        let num_local_change_ids = read_u32(file)?;
        let num_parent_overflow_entries = read_u32(file)?;
        let num_change_overflow_entries = read_u32(file)?;
        let mut data = vec![];
        file.read_to_end(&mut data).map_err(from_io_err)?;

        let commit_graph_entry_size = CommitGraphEntry::size(commit_id_length);
        let graph_size = (num_local_commits as usize) * commit_graph_entry_size;
        let commit_lookup_size = (num_local_commits as usize) * 4;
        let change_id_table_size = (num_local_change_ids as usize) * change_id_length;
        let change_pos_table_size = (num_local_change_ids as usize) * 4;
        let parent_overflow_size = (num_parent_overflow_entries as usize) * 4;
        let change_overflow_size = (num_change_overflow_entries as usize) * 4;

        let graph_base = 0;
        let commit_lookup_base = graph_base + graph_size;
        let change_id_table_base = commit_lookup_base + commit_lookup_size;
        let change_pos_table_base = change_id_table_base + change_id_table_size;
        let parent_overflow_base = change_pos_table_base + change_pos_table_size;
        let change_overflow_base = parent_overflow_base + parent_overflow_size;
        let expected_size = change_overflow_base + change_overflow_size;

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
            num_local_commits,
            num_local_change_ids,
            num_change_overflow_entries,
            commit_lookup_base,
            change_id_table_base,
            change_pos_table_base,
            parent_overflow_base,
            change_overflow_base,
            data,
        }))
    }

    pub(super) fn as_composite(&self) -> &CompositeIndex {
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
        let table = &self.data[..self.commit_lookup_base];
        let entry_size = CommitGraphEntry::size(self.commit_id_length);
        let offset = (local_pos.0 as usize) * entry_size;
        CommitGraphEntry {
            data: &table[offset..][..entry_size],
        }
    }

    fn commit_lookup_pos(&self, lookup_pos: u32) -> LocalPosition {
        let table = &self.data[self.commit_lookup_base..self.change_id_table_base];
        let offset = (lookup_pos as usize) * 4;
        LocalPosition(u32::from_le_bytes(table[offset..][..4].try_into().unwrap()))
    }

    fn change_lookup_id(&self, lookup_pos: u32) -> ChangeId {
        ChangeId::from_bytes(self.change_lookup_id_bytes(lookup_pos))
    }

    // might be better to add borrowed version of ChangeId
    fn change_lookup_id_bytes(&self, lookup_pos: u32) -> &[u8] {
        let table = &self.data[self.change_id_table_base..self.change_pos_table_base];
        let offset = (lookup_pos as usize) * self.change_id_length;
        &table[offset..][..self.change_id_length]
    }

    fn change_lookup_pos(&self, lookup_pos: u32) -> ChangeLocalPosition {
        let table = &self.data[self.change_pos_table_base..self.parent_overflow_base];
        let offset = (lookup_pos as usize) * 4;
        ChangeLocalPosition(u32::from_le_bytes(table[offset..][..4].try_into().unwrap()))
    }

    fn overflow_parents(&self, overflow_pos: u32, num_parents: u32) -> SmallIndexPositionsVec {
        let table = &self.data[self.parent_overflow_base..self.change_overflow_base];
        let offset = (overflow_pos as usize) * 4;
        let size = (num_parents as usize) * 4;
        table[offset..][..size]
            .chunks_exact(4)
            .map(|chunk| IndexPosition(u32::from_le_bytes(chunk.try_into().unwrap())))
            .collect()
    }

    /// Scans graph entry positions stored in the overflow change ids table.
    fn overflow_changes_from(&self, overflow_pos: u32) -> impl Iterator<Item = LocalPosition> + '_ {
        let table = &self.data[self.change_overflow_base..];
        let offset = (overflow_pos as usize) * 4;
        table[offset..]
            .chunks_exact(4)
            .map(|chunk| LocalPosition(u32::from_le_bytes(chunk.try_into().unwrap())))
    }

    /// Binary searches commit id by `prefix`. Returns the lookup position.
    fn commit_id_byte_prefix_to_lookup_pos(&self, prefix: &[u8]) -> PositionLookupResult {
        binary_search_pos_by(self.num_local_commits, |pos| {
            let local_pos = self.commit_lookup_pos(pos);
            let entry = self.graph_entry(local_pos);
            entry.commit_id_bytes().cmp(prefix)
        })
    }

    /// Binary searches change id by `prefix`. Returns the lookup position.
    fn change_id_byte_prefix_to_lookup_pos(&self, prefix: &[u8]) -> PositionLookupResult {
        binary_search_pos_by(self.num_local_change_ids, |pos| {
            let change_id_bytes = self.change_lookup_id_bytes(pos);
            change_id_bytes.cmp(prefix)
        })
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

    fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<LocalPosition> {
        self.commit_id_byte_prefix_to_lookup_pos(commit_id.as_bytes())
            .ok()
            .map(|pos| self.commit_lookup_pos(pos))
    }

    fn resolve_neighbor_commit_ids(
        &self,
        commit_id: &CommitId,
    ) -> (Option<CommitId>, Option<CommitId>) {
        self.commit_id_byte_prefix_to_lookup_pos(commit_id.as_bytes())
            .map_neighbors(|pos| {
                let local_pos = self.commit_lookup_pos(pos);
                let entry = self.graph_entry(local_pos);
                entry.commit_id()
            })
    }

    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        self.commit_id_byte_prefix_to_lookup_pos(prefix.min_prefix_bytes())
            .prefix_matches(prefix, |pos| {
                let local_pos = self.commit_lookup_pos(pos);
                let entry = self.graph_entry(local_pos);
                entry.commit_id()
            })
            .map(|(id, _)| id)
    }

    fn resolve_neighbor_change_ids(
        &self,
        change_id: &ChangeId,
    ) -> (Option<ChangeId>, Option<ChangeId>) {
        self.change_id_byte_prefix_to_lookup_pos(change_id.as_bytes())
            .map_neighbors(|pos| self.change_lookup_id(pos))
    }

    fn resolve_change_id_prefix(
        &self,
        prefix: &HexPrefix,
    ) -> PrefixResolution<(ChangeId, SmallLocalPositionsVec)> {
        self.change_id_byte_prefix_to_lookup_pos(prefix.min_prefix_bytes())
            .prefix_matches(prefix, |pos| self.change_lookup_id(pos))
            .map(|(id, lookup_pos)| {
                let change_pos = self.change_lookup_pos(lookup_pos);
                if let Some(local_pos) = change_pos.as_inlined() {
                    (id, smallvec![local_pos])
                } else {
                    let overflow_pos = change_pos.as_overflow().unwrap();
                    // Collect commits having the same change id. For cache
                    // locality, it might be better to look for the next few
                    // change id positions to determine the size.
                    let positions: SmallLocalPositionsVec = self
                        .overflow_changes_from(overflow_pos)
                        .take_while(|&local_pos| {
                            let entry = self.graph_entry(local_pos);
                            entry.change_id_lookup_pos() == lookup_pos
                        })
                        .collect();
                    debug_assert_eq!(
                        overflow_pos + u32::try_from(positions.len()).unwrap(),
                        (lookup_pos + 1..self.num_local_change_ids)
                            .find_map(|lookup_pos| self.change_lookup_pos(lookup_pos).as_overflow())
                            .unwrap_or(self.num_change_overflow_entries),
                        "all overflow positions to the next change id should be collected"
                    );
                    (id, positions)
                }
            })
    }

    fn generation_number(&self, local_pos: LocalPosition) -> u32 {
        self.graph_entry(local_pos).generation_number()
    }

    fn commit_id(&self, local_pos: LocalPosition) -> CommitId {
        self.graph_entry(local_pos).commit_id()
    }

    fn change_id(&self, local_pos: LocalPosition) -> ChangeId {
        let entry = self.graph_entry(local_pos);
        self.change_lookup_id(entry.change_id_lookup_pos())
    }

    fn num_parents(&self, local_pos: LocalPosition) -> u32 {
        let graph_entry = self.graph_entry(local_pos);
        let pos1_or_overflow_pos = graph_entry.parent1_pos_or_overflow_pos();
        let pos2_or_overflow_len = graph_entry.parent2_pos_or_overflow_len();
        let inlined_len1 = pos1_or_overflow_pos.as_inlined().is_some() as u32;
        let inlined_len2 = pos2_or_overflow_len.as_inlined().is_some() as u32;
        let overflow_len = pos2_or_overflow_len.as_overflow().unwrap_or(0);
        inlined_len1 + inlined_len2 + overflow_len
    }

    fn parent_positions(&self, local_pos: LocalPosition) -> SmallIndexPositionsVec {
        let graph_entry = self.graph_entry(local_pos);
        let pos1_or_overflow_pos = graph_entry.parent1_pos_or_overflow_pos();
        let pos2_or_overflow_len = graph_entry.parent2_pos_or_overflow_len();
        if let Some(pos1) = pos1_or_overflow_pos.as_inlined() {
            if let Some(pos2) = pos2_or_overflow_len.as_inlined() {
                smallvec![pos1, pos2]
            } else {
                smallvec![pos1]
            }
        } else {
            let overflow_pos = pos1_or_overflow_pos.as_overflow().unwrap();
            let num_parents = pos2_or_overflow_len.as_overflow().unwrap();
            self.overflow_parents(overflow_pos, num_parents)
        }
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
    fn as_composite(&self) -> &CompositeIndex {
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

    fn all_heads_for_gc(
        &self,
    ) -> Result<Box<dyn Iterator<Item = CommitId> + '_>, AllHeadsForGcUnsupported> {
        Ok(Box::new(self.as_composite().all_heads()))
    }

    fn heads(&self, candidates: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId> {
        self.as_composite().heads(candidates)
    }

    fn evaluate_revset<'index>(
        &'index self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError> {
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

/// Binary search result in a sorted lookup table.
#[derive(Clone, Copy, Debug)]
struct PositionLookupResult {
    /// `Ok` means the element is found at the position. `Err` contains the
    /// position where the element could be inserted.
    result: Result<u32, u32>,
    size: u32,
}

impl PositionLookupResult {
    /// Returns position of the element if exactly matched.
    fn ok(self) -> Option<u32> {
        self.result.ok()
    }

    /// Returns `(previous, next)` positions of the matching element or
    /// boundary.
    fn neighbors(self) -> (Option<u32>, Option<u32>) {
        match self.result {
            Ok(pos) => (pos.checked_sub(1), (pos + 1..self.size).next()),
            Err(pos) => (pos.checked_sub(1), (pos..self.size).next()),
        }
    }

    /// Looks up `(previous, next)` elements by the given function.
    fn map_neighbors<T>(self, mut lookup: impl FnMut(u32) -> T) -> (Option<T>, Option<T>) {
        let (prev_pos, next_pos) = self.neighbors();
        (prev_pos.map(&mut lookup), next_pos.map(&mut lookup))
    }

    /// Looks up matching elements from the current position, returns one if
    /// the given `prefix` unambiguously matches.
    fn prefix_matches<T: ObjectId>(
        self,
        prefix: &HexPrefix,
        lookup: impl FnMut(u32) -> T,
    ) -> PrefixResolution<(T, u32)> {
        let lookup_pos = self.result.unwrap_or_else(|pos| pos);
        let mut matches = (lookup_pos..self.size)
            .map(lookup)
            .take_while(|id| prefix.matches(id))
            .fuse();
        match (matches.next(), matches.next()) {
            (Some(id), None) => PrefixResolution::SingleMatch((id, lookup_pos)),
            (Some(_), Some(_)) => PrefixResolution::AmbiguousMatch,
            (None, _) => PrefixResolution::NoMatch,
        }
    }
}

/// Binary searches u32 position with the given comparison function.
fn binary_search_pos_by(size: u32, mut f: impl FnMut(u32) -> Ordering) -> PositionLookupResult {
    let mut low = 0;
    let mut high = size;
    while low < high {
        let mid = (low + high) / 2;
        let cmp = f(mid);
        // According to Rust std lib, this produces cmov instructions.
        // https://github.com/rust-lang/rust/blob/1.76.0/library/core/src/slice/mod.rs#L2845-L2855
        low = if cmp == Ordering::Less { mid + 1 } else { low };
        high = if cmp == Ordering::Greater { mid } else { high };
        if cmp == Ordering::Equal {
            let result = Ok(mid);
            return PositionLookupResult { result, size };
        }
    }
    let result = Err(low);
    PositionLookupResult { result, size }
}
