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

use std::cmp::{max, min, Ordering};
use std::collections::{BTreeSet, BinaryHeap, HashSet};
use std::iter;
use std::sync::Arc;

use itertools::Itertools;

use super::entry::{
    IndexEntry, IndexPosition, IndexPositionByGeneration, LocalPosition, SmallIndexPositionsVec,
};
use super::readonly::ReadonlyIndexSegment;
use super::rev_walk::RevWalk;
use crate::backend::{ChangeId, CommitId, ObjectId};
use crate::index::{HexPrefix, Index, PrefixResolution};
use crate::revset::{ResolvedExpression, Revset, RevsetEvaluationError};
use crate::store::Store;
use crate::{default_revset_engine, hex_util};

pub(super) trait IndexSegment: Send + Sync {
    fn num_parent_commits(&self) -> u32;

    fn num_local_commits(&self) -> u32;

    fn parent_file(&self) -> Option<&Arc<ReadonlyIndexSegment>>;

    fn name(&self) -> Option<String>;

    fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<IndexPosition>;

    /// Suppose the given `commit_id` exists, returns the previous and next
    /// commit ids in lexicographical order.
    fn resolve_neighbor_commit_ids(
        &self,
        commit_id: &CommitId,
    ) -> (Option<CommitId>, Option<CommitId>);

    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId>;

    fn generation_number(&self, local_pos: LocalPosition) -> u32;

    fn commit_id(&self, local_pos: LocalPosition) -> CommitId;

    fn change_id(&self, local_pos: LocalPosition) -> ChangeId;

    fn num_parents(&self, local_pos: LocalPosition) -> u32;

    fn parent_positions(&self, local_pos: LocalPosition) -> SmallIndexPositionsVec;
}

/// Abstraction over owned and borrowed types that can be cheaply converted to
/// a `CompositeIndex` reference.
pub trait AsCompositeIndex {
    /// Returns reference wrapper that provides global access to this index.
    fn as_composite(&self) -> CompositeIndex<'_>;
}

impl<T: AsCompositeIndex + ?Sized> AsCompositeIndex for &T {
    fn as_composite(&self) -> CompositeIndex<'_> {
        <T as AsCompositeIndex>::as_composite(self)
    }
}

impl<T: AsCompositeIndex + ?Sized> AsCompositeIndex for &mut T {
    fn as_composite(&self) -> CompositeIndex<'_> {
        <T as AsCompositeIndex>::as_composite(self)
    }
}

/// Reference wrapper that provides global access to nested index segments.
#[derive(Clone, Copy)]
pub struct CompositeIndex<'a>(&'a dyn IndexSegment);

impl<'a> CompositeIndex<'a> {
    pub(super) fn new(segment: &'a dyn IndexSegment) -> Self {
        CompositeIndex(segment)
    }

    /// Iterates parent and its ancestor readonly index segments.
    pub(super) fn ancestor_files_without_local(
        &self,
    ) -> impl Iterator<Item = &'a Arc<ReadonlyIndexSegment>> {
        let parent_file = self.0.parent_file();
        iter::successors(parent_file, |file| file.parent_file())
    }

    /// Iterates self and its ancestor index segments.
    pub(super) fn ancestor_index_segments(&self) -> impl Iterator<Item = &'a dyn IndexSegment> {
        iter::once(self.0).chain(
            self.ancestor_files_without_local()
                .map(|file| file.as_ref() as &dyn IndexSegment),
        )
    }

    pub fn num_commits(&self) -> u32 {
        self.0.num_parent_commits() + self.0.num_local_commits()
    }

    pub fn stats(&self) -> IndexStats {
        let num_commits = self.num_commits();
        let mut num_merges = 0;
        let mut max_generation_number = 0;
        let mut is_head = vec![true; num_commits as usize];
        let mut change_ids = HashSet::new();
        for pos in 0..num_commits {
            let entry = self.entry_by_pos(IndexPosition(pos));
            max_generation_number = max(max_generation_number, entry.generation_number());
            if entry.num_parents() > 1 {
                num_merges += 1;
            }
            for parent_pos in entry.parent_positions() {
                is_head[parent_pos.0 as usize] = false;
            }
            change_ids.insert(entry.change_id());
        }
        let num_heads = u32::try_from(is_head.iter().filter(|is_head| **is_head).count()).unwrap();

        let mut levels = self
            .ancestor_index_segments()
            .map(|segment| IndexLevelStats {
                num_commits: segment.num_local_commits(),
                name: segment.name(),
            })
            .collect_vec();
        levels.reverse();

        IndexStats {
            num_commits,
            num_merges,
            max_generation_number,
            num_heads,
            num_changes: change_ids.len().try_into().unwrap(),
            levels,
        }
    }

    pub fn entry_by_pos(&self, pos: IndexPosition) -> IndexEntry<'a> {
        self.ancestor_index_segments()
            .find_map(|segment| {
                u32::checked_sub(pos.0, segment.num_parent_commits())
                    .map(|local_pos| IndexEntry::new(segment, pos, LocalPosition(local_pos)))
            })
            .unwrap()
    }

    pub fn commit_id_to_pos(&self, commit_id: &CommitId) -> Option<IndexPosition> {
        self.ancestor_index_segments()
            .find_map(|segment| segment.commit_id_to_pos(commit_id))
    }

    /// Suppose the given `commit_id` exists, returns the previous and next
    /// commit ids in lexicographical order.
    pub(super) fn resolve_neighbor_commit_ids(
        &self,
        commit_id: &CommitId,
    ) -> (Option<CommitId>, Option<CommitId>) {
        self.ancestor_index_segments()
            .map(|segment| segment.resolve_neighbor_commit_ids(commit_id))
            .reduce(|(acc_prev_id, acc_next_id), (prev_id, next_id)| {
                (
                    acc_prev_id.into_iter().chain(prev_id).max(),
                    acc_next_id.into_iter().chain(next_id).min(),
                )
            })
            .unwrap()
    }

    pub fn entry_by_id(&self, commit_id: &CommitId) -> Option<IndexEntry<'a>> {
        self.commit_id_to_pos(commit_id)
            .map(|pos| self.entry_by_pos(pos))
    }

    pub(super) fn is_ancestor_pos(
        &self,
        ancestor_pos: IndexPosition,
        descendant_pos: IndexPosition,
    ) -> bool {
        let ancestor_generation = self.entry_by_pos(ancestor_pos).generation_number();
        let mut work = vec![descendant_pos];
        let mut visited = HashSet::new();
        while let Some(descendant_pos) = work.pop() {
            let descendant_entry = self.entry_by_pos(descendant_pos);
            if descendant_pos == ancestor_pos {
                return true;
            }
            if !visited.insert(descendant_entry.position()) {
                continue;
            }
            if descendant_entry.generation_number() <= ancestor_generation {
                continue;
            }
            work.extend(descendant_entry.parent_positions());
        }
        false
    }

    pub(super) fn common_ancestors_pos(
        &self,
        set1: &[IndexPosition],
        set2: &[IndexPosition],
    ) -> BTreeSet<IndexPosition> {
        let mut items1: BinaryHeap<_> = set1
            .iter()
            .map(|pos| IndexPositionByGeneration::from(&self.entry_by_pos(*pos)))
            .collect();
        let mut items2: BinaryHeap<_> = set2
            .iter()
            .map(|pos| IndexPositionByGeneration::from(&self.entry_by_pos(*pos)))
            .collect();

        let mut result = BTreeSet::new();
        while let (Some(item1), Some(item2)) = (items1.peek(), items2.peek()) {
            match item1.cmp(item2) {
                Ordering::Greater => {
                    let item1 = dedup_pop(&mut items1).unwrap();
                    let entry1 = self.entry_by_pos(item1.pos);
                    for parent_entry in entry1.parents() {
                        assert!(parent_entry.position() < entry1.position());
                        items1.push(IndexPositionByGeneration::from(&parent_entry));
                    }
                }
                Ordering::Less => {
                    let item2 = dedup_pop(&mut items2).unwrap();
                    let entry2 = self.entry_by_pos(item2.pos);
                    for parent_entry in entry2.parents() {
                        assert!(parent_entry.position() < entry2.position());
                        items2.push(IndexPositionByGeneration::from(&parent_entry));
                    }
                }
                Ordering::Equal => {
                    result.insert(item1.pos);
                    dedup_pop(&mut items1).unwrap();
                    dedup_pop(&mut items2).unwrap();
                }
            }
        }
        self.heads_pos(result)
    }

    pub fn walk_revs(&self, wanted: &[IndexPosition], unwanted: &[IndexPosition]) -> RevWalk<'a> {
        let mut rev_walk = RevWalk::new(*self);
        rev_walk.extend_wanted(wanted.iter().copied());
        rev_walk.extend_unwanted(unwanted.iter().copied());
        rev_walk
    }

    pub fn heads_pos(
        &self,
        mut candidate_positions: BTreeSet<IndexPosition>,
    ) -> BTreeSet<IndexPosition> {
        // Add all parents of the candidates to the work queue. The parents and their
        // ancestors are not heads.
        // Also find the smallest generation number among the candidates.
        let mut work = BinaryHeap::new();
        let mut min_generation = u32::MAX;
        for pos in &candidate_positions {
            let entry = self.entry_by_pos(*pos);
            min_generation = min(min_generation, entry.generation_number());
            for parent_entry in entry.parents() {
                work.push(IndexPositionByGeneration::from(&parent_entry));
            }
        }

        // Walk ancestors of the parents of the candidates. Remove visited commits from
        // set of candidates. Stop walking when we have gone past the minimum
        // candidate generation.
        while let Some(item) = dedup_pop(&mut work) {
            if item.generation < min_generation {
                break;
            }
            candidate_positions.remove(&item.pos);
            let entry = self.entry_by_pos(item.pos);
            for parent_entry in entry.parents() {
                assert!(parent_entry.position() < entry.position());
                work.push(IndexPositionByGeneration::from(&parent_entry));
            }
        }
        candidate_positions
    }

    pub(super) fn evaluate_revset(
        &self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset<'a> + 'a>, RevsetEvaluationError> {
        let revset_impl = default_revset_engine::evaluate(expression, store, *self)?;
        Ok(Box::new(revset_impl))
    }
}

impl AsCompositeIndex for CompositeIndex<'_> {
    fn as_composite(&self) -> CompositeIndex<'_> {
        *self
    }
}

impl Index for CompositeIndex<'_> {
    /// Suppose the given `commit_id` exists, returns the minimum prefix length
    /// to disambiguate it. The length to be returned is a number of hexadecimal
    /// digits.
    ///
    /// If the given `commit_id` doesn't exist, this will return the prefix
    /// length that never matches with any commit ids.
    fn shortest_unique_commit_id_prefix_len(&self, commit_id: &CommitId) -> usize {
        let (prev_id, next_id) = self.resolve_neighbor_commit_ids(commit_id);
        itertools::chain(prev_id, next_id)
            .map(|id| hex_util::common_hex_len(commit_id.as_bytes(), id.as_bytes()) + 1)
            .max()
            .unwrap_or(0)
    }

    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        self.ancestor_index_segments()
            .fold(PrefixResolution::NoMatch, |acc_match, segment| {
                if acc_match == PrefixResolution::AmbiguousMatch {
                    acc_match // avoid checking the parent file(s)
                } else {
                    let local_match = segment.resolve_commit_id_prefix(prefix);
                    acc_match.plus(&local_match)
                }
            })
    }

    fn has_id(&self, commit_id: &CommitId) -> bool {
        self.commit_id_to_pos(commit_id).is_some()
    }

    fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool {
        let ancestor_pos = self.commit_id_to_pos(ancestor_id).unwrap();
        let descendant_pos = self.commit_id_to_pos(descendant_id).unwrap();
        self.is_ancestor_pos(ancestor_pos, descendant_pos)
    }

    fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId> {
        let pos1 = set1
            .iter()
            .map(|id| self.commit_id_to_pos(id).unwrap())
            .collect_vec();
        let pos2 = set2
            .iter()
            .map(|id| self.commit_id_to_pos(id).unwrap())
            .collect_vec();
        self.common_ancestors_pos(&pos1, &pos2)
            .iter()
            .map(|pos| self.entry_by_pos(*pos).commit_id())
            .collect()
    }

    fn heads(&self, candidate_ids: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId> {
        let candidate_positions: BTreeSet<_> = candidate_ids
            .map(|id| self.commit_id_to_pos(id).unwrap())
            .collect();

        self.heads_pos(candidate_positions)
            .iter()
            .map(|pos| self.entry_by_pos(*pos).commit_id())
            .collect()
    }

    /// Parents before children
    fn topo_order(&self, input: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId> {
        let mut ids = input.cloned().collect_vec();
        ids.sort_by_cached_key(|id| self.commit_id_to_pos(id).unwrap());
        ids
    }

    fn evaluate_revset<'index>(
        &'index self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset<'index> + 'index>, RevsetEvaluationError> {
        CompositeIndex::evaluate_revset(self, expression, store)
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
    pub num_changes: u32,
    pub levels: Vec<IndexLevelStats>,
}

/// Removes the greatest items (including duplicates) from the heap, returns
/// one.
fn dedup_pop<T: Ord>(heap: &mut BinaryHeap<T>) -> Option<T> {
    let item = heap.pop()?;
    while heap.peek() == Some(&item) {
        heap.pop().unwrap();
    }
    Some(item)
}
