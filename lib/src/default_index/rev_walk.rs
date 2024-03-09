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

use std::cmp::{max, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::iter::{Fuse, FusedIterator};
use std::ops::Range;

use smallvec::SmallVec;

use super::composite::CompositeIndex;
use super::entry::{IndexPosition, SmallIndexPositionsVec};

/// Like `Iterator`, but doesn't borrow the `index` internally.
pub(super) trait RevWalk<I: ?Sized> {
    type Item;

    /// Advances the iteration and returns the next item.
    ///
    /// The caller must provide the same `index` instance.
    ///
    /// Returns `None` when the iteration is finished. Once `None` is returned,
    /// this will never resume. In other words, a `RevWalk` is fused.
    fn next(&mut self, index: &I) -> Option<Self::Item>;

    // The following methods are provided for convenience. They are not supposed
    // to be reimplemented.

    /// Wraps in adapter that will filter items by the given `predicate`.
    fn filter<P>(self, predicate: P) -> FilterRevWalk<Self, P>
    where
        Self: Sized,
        P: FnMut(&I, &Self::Item) -> bool,
    {
        FilterRevWalk {
            walk: self,
            predicate,
        }
    }

    /// Wraps in adapter that can peek one more item without consuming.
    fn peekable(self) -> PeekableRevWalk<I, Self>
    where
        Self: Sized,
    {
        PeekableRevWalk {
            walk: self,
            peeked: None,
        }
    }

    /// Reattaches the underlying `index`.
    fn attach(self, index: &I) -> RevWalkBorrowedIndexIter<'_, I, Self>
    where
        Self: Sized,
    {
        RevWalkBorrowedIndexIter { index, walk: self }
    }
}

impl<I: ?Sized, W: RevWalk<I> + ?Sized> RevWalk<I> for Box<W> {
    type Item = W::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        <W as RevWalk<I>>::next(self, index)
    }
}

/// Adapter that turns `Iterator` into `RevWalk` by dropping index argument.
///
/// As the name suggests, the source object is usually a slice or `Vec`.
#[derive(Clone, Debug)]
pub(super) struct EagerRevWalk<T> {
    iter: Fuse<T>,
}

impl<T: Iterator> EagerRevWalk<T> {
    pub fn new(iter: T) -> Self {
        EagerRevWalk { iter: iter.fuse() }
    }
}

impl<I: ?Sized, T: Iterator> RevWalk<I> for EagerRevWalk<T> {
    type Item = T::Item;

    fn next(&mut self, _index: &I) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
#[must_use]
pub(super) struct FilterRevWalk<W, P> {
    walk: W,
    predicate: P,
}

impl<I, W, P> RevWalk<I> for FilterRevWalk<W, P>
where
    I: ?Sized,
    W: RevWalk<I>,
    P: FnMut(&I, &W::Item) -> bool,
{
    type Item = W::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        while let Some(item) = self.walk.next(index) {
            if (self.predicate)(index, &item) {
                return Some(item);
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
#[must_use]
pub(super) struct PeekableRevWalk<I: ?Sized, W: RevWalk<I>> {
    walk: W,
    // Since RevWalk is fused, we don't need a nested Option<Option<_>>.
    peeked: Option<W::Item>,
}

impl<I: ?Sized, W: RevWalk<I>> PeekableRevWalk<I, W> {
    pub fn peek(&mut self, index: &I) -> Option<&W::Item> {
        if self.peeked.is_none() {
            self.peeked = self.walk.next(index);
        }
        self.peeked.as_ref()
    }

    pub fn next_if(
        &mut self,
        index: &I,
        predicate: impl FnOnce(&W::Item) -> bool,
    ) -> Option<W::Item> {
        match self.next(index) {
            Some(item) if predicate(&item) => Some(item),
            other => {
                assert!(self.peeked.is_none());
                self.peeked = other;
                None
            }
        }
    }
}

impl<I: ?Sized, W: RevWalk<I>> RevWalk<I> for PeekableRevWalk<I, W> {
    type Item = W::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        self.peeked.take().or_else(|| self.walk.next(index))
    }
}

/// Adapter that turns `RevWalk` into `Iterator` by attaching borrowed `index`.
#[derive(Clone, Debug)]
#[must_use]
pub(super) struct RevWalkBorrowedIndexIter<'a, I: ?Sized, W> {
    index: &'a I,
    walk: W,
}

impl<I: ?Sized, W> RevWalkBorrowedIndexIter<'_, I, W> {
    /// Turns into `'static`-lifetime walk object by detaching the index.
    pub fn detach(self) -> W {
        self.walk
    }
}

impl<I: ?Sized, W: RevWalk<I>> Iterator for RevWalkBorrowedIndexIter<'_, I, W> {
    type Item = W::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.walk.next(self.index)
    }
}

impl<I: ?Sized, W: RevWalk<I>> FusedIterator for RevWalkBorrowedIndexIter<'_, I, W> {}

/// Adapter that turns `RevWalk` into `Iterator` by attaching owned `index`.
#[derive(Clone, Debug)]
#[must_use]
pub(super) struct RevWalkOwnedIndexIter<I, W> {
    index: I,
    walk: W,
}

impl<I, W: RevWalk<I>> Iterator for RevWalkOwnedIndexIter<I, W> {
    type Item = W::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.walk.next(&self.index)
    }
}

impl<I, W: RevWalk<I>> FusedIterator for RevWalkOwnedIndexIter<I, W> {}

pub(super) trait RevWalkIndex {
    type Position: Copy + Ord;
    type AdjacentPositions: IntoIterator<Item = Self::Position>;

    fn adjacent_positions(&self, pos: Self::Position) -> Self::AdjacentPositions;
}

impl RevWalkIndex for CompositeIndex {
    type Position = IndexPosition;
    type AdjacentPositions = SmallIndexPositionsVec;

    fn adjacent_positions(&self, pos: Self::Position) -> Self::AdjacentPositions {
        self.entry_by_pos(pos).parent_positions()
    }
}

#[derive(Clone)]
pub(super) struct RevWalkDescendantsIndex {
    children_map: HashMap<IndexPosition, DescendantIndexPositionsVec>,
}

// See SmallIndexPositionsVec for the array size.
type DescendantIndexPositionsVec = SmallVec<[Reverse<IndexPosition>; 4]>;

impl RevWalkDescendantsIndex {
    fn build(index: &CompositeIndex, positions: impl IntoIterator<Item = IndexPosition>) -> Self {
        // For dense set, it's probably cheaper to use `Vec` instead of `HashMap`.
        let mut children_map: HashMap<IndexPosition, DescendantIndexPositionsVec> = HashMap::new();
        for pos in positions {
            children_map.entry(pos).or_default(); // mark head node
            for parent_pos in index.entry_by_pos(pos).parent_positions() {
                let parent = children_map.entry(parent_pos).or_default();
                parent.push(Reverse(pos));
            }
        }

        RevWalkDescendantsIndex { children_map }
    }

    fn contains_pos(&self, pos: IndexPosition) -> bool {
        self.children_map.contains_key(&pos)
    }
}

impl RevWalkIndex for RevWalkDescendantsIndex {
    type Position = Reverse<IndexPosition>;
    type AdjacentPositions = DescendantIndexPositionsVec;

    fn adjacent_positions(&self, pos: Self::Position) -> Self::AdjacentPositions {
        self.children_map[&pos.0].clone()
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct RevWalkWorkItem<P, T> {
    pos: P,
    state: RevWalkWorkItemState<T>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RevWalkWorkItemState<T> {
    // Order matters: Unwanted should appear earlier in the max-heap.
    Wanted(T),
    Unwanted,
}

impl<P, T> RevWalkWorkItem<P, T> {
    fn is_wanted(&self) -> bool {
        matches!(self.state, RevWalkWorkItemState::Wanted(_))
    }
}

#[derive(Clone)]
struct RevWalkQueue<P, T> {
    items: BinaryHeap<RevWalkWorkItem<P, T>>,
    min_pos: P,
    unwanted_count: usize,
}

impl<P: Ord, T: Ord> RevWalkQueue<P, T> {
    fn with_min_pos(min_pos: P) -> Self {
        Self {
            items: BinaryHeap::new(),
            min_pos,
            unwanted_count: 0,
        }
    }

    fn push_wanted(&mut self, pos: P, t: T) {
        if pos < self.min_pos {
            return;
        }
        let state = RevWalkWorkItemState::Wanted(t);
        self.items.push(RevWalkWorkItem { pos, state });
    }

    fn push_unwanted(&mut self, pos: P) {
        if pos < self.min_pos {
            return;
        }
        let state = RevWalkWorkItemState::Unwanted;
        self.items.push(RevWalkWorkItem { pos, state });
        self.unwanted_count += 1;
    }

    fn extend_wanted(&mut self, positions: impl IntoIterator<Item = P>, t: T)
    where
        T: Clone,
    {
        // positions typically contains one item, and single BinaryHeap::push()
        // appears to be slightly faster than .extend() as of rustc 1.73.0.
        for pos in positions {
            self.push_wanted(pos, t.clone());
        }
    }

    fn extend_unwanted(&mut self, positions: impl IntoIterator<Item = P>) {
        for pos in positions {
            self.push_unwanted(pos);
        }
    }

    fn pop(&mut self) -> Option<RevWalkWorkItem<P, T>> {
        if let Some(x) = self.items.pop() {
            self.unwanted_count -= !x.is_wanted() as usize;
            Some(x)
        } else {
            None
        }
    }

    fn pop_eq(&mut self, pos: &P) -> Option<RevWalkWorkItem<P, T>> {
        if let Some(x) = self.items.peek() {
            (x.pos == *pos).then(|| self.pop().unwrap())
        } else {
            None
        }
    }

    fn skip_while_eq(&mut self, pos: &P) {
        while self.pop_eq(pos).is_some() {
            continue;
        }
    }
}

#[derive(Clone)]
#[must_use]
pub(super) struct RevWalkBuilder<'a> {
    index: &'a CompositeIndex,
    wanted: Vec<IndexPosition>,
    unwanted: Vec<IndexPosition>,
}

impl<'a> RevWalkBuilder<'a> {
    pub fn new(index: &'a CompositeIndex) -> Self {
        RevWalkBuilder {
            index,
            wanted: Vec::new(),
            unwanted: Vec::new(),
        }
    }

    /// Adds head positions to be included.
    pub fn wanted_heads(mut self, positions: impl IntoIterator<Item = IndexPosition>) -> Self {
        self.wanted.extend(positions);
        self
    }

    /// Adds root positions to be excluded. The roots precede the heads.
    pub fn unwanted_roots(mut self, positions: impl IntoIterator<Item = IndexPosition>) -> Self {
        self.unwanted.extend(positions);
        self
    }

    /// Walks ancestors.
    pub fn ancestors(self) -> RevWalkAncestors<'a> {
        self.ancestors_with_min_pos(IndexPosition::MIN)
    }

    fn ancestors_with_min_pos(self, min_pos: IndexPosition) -> RevWalkAncestors<'a> {
        let index = self.index;
        let mut queue = RevWalkQueue::with_min_pos(min_pos);
        queue.extend_wanted(self.wanted, ());
        queue.extend_unwanted(self.unwanted);
        RevWalkBorrowedIndexIter {
            index,
            walk: RevWalkImpl { queue },
        }
    }

    /// Walks ancestors within the `generation_range`.
    ///
    /// A generation number counts from the heads.
    pub fn ancestors_filtered_by_generation(
        self,
        generation_range: Range<u32>,
    ) -> RevWalkAncestorsGenerationRange<'a> {
        let index = self.index;
        let mut queue = RevWalkQueue::with_min_pos(IndexPosition::MIN);
        let item_range = RevWalkItemGenerationRange::from_filter_range(generation_range.clone());
        queue.extend_wanted(self.wanted, Reverse(item_range));
        queue.extend_unwanted(self.unwanted);
        RevWalkBorrowedIndexIter {
            index,
            walk: RevWalkGenerationRangeImpl {
                queue,
                generation_end: generation_range.end,
            },
        }
    }

    /// Walks ancestors until all of the reachable roots in `root_positions` get
    /// visited.
    ///
    /// Use this if you are only interested in descendants of the given roots.
    /// The caller still needs to filter out unwanted entries.
    pub fn ancestors_until_roots(
        self,
        root_positions: impl IntoIterator<Item = IndexPosition>,
    ) -> RevWalkAncestors<'a> {
        // We can also make it stop visiting based on the generation number. Maybe
        // it will perform better for unbalanced branchy history.
        // https://github.com/martinvonz/jj/pull/1492#discussion_r1160678325
        let min_pos = root_positions
            .into_iter()
            .min()
            .unwrap_or(IndexPosition::MAX);
        self.ancestors_with_min_pos(min_pos)
    }

    /// Fully consumes ancestors and walks back from the `root_positions`.
    ///
    /// The returned iterator yields entries in order of ascending index
    /// position.
    pub fn descendants(
        self,
        root_positions: impl IntoIterator<Item = IndexPosition>,
    ) -> RevWalkDescendants<'a> {
        let index = self.index;
        let root_positions = HashSet::from_iter(root_positions);
        let candidate_positions = self
            .ancestors_until_roots(root_positions.iter().copied())
            .collect();
        RevWalkBorrowedIndexIter {
            index,
            walk: RevWalkDescendantsImpl {
                candidate_positions,
                root_positions,
                reachable_positions: HashSet::new(),
            },
        }
    }

    /// Fully consumes ancestors and walks back from the `root_positions` within
    /// the `generation_range`.
    ///
    /// A generation number counts from the roots.
    ///
    /// The returned iterator yields entries in order of ascending index
    /// position.
    pub fn descendants_filtered_by_generation(
        self,
        root_positions: impl IntoIterator<Item = IndexPosition>,
        generation_range: Range<u32>,
    ) -> RevWalkDescendantsGenerationRange {
        let index = self.index;
        let root_positions = Vec::from_iter(root_positions);
        let positions = self.ancestors_until_roots(root_positions.iter().copied());
        let descendants_index = RevWalkDescendantsIndex::build(index, positions);

        let mut queue = RevWalkQueue::with_min_pos(Reverse(IndexPosition::MAX));
        let item_range = RevWalkItemGenerationRange::from_filter_range(generation_range.clone());
        for pos in root_positions {
            // Do not add unreachable roots which shouldn't be visited
            if descendants_index.contains_pos(pos) {
                queue.push_wanted(Reverse(pos), Reverse(item_range));
            }
        }
        RevWalkOwnedIndexIter {
            index: descendants_index,
            walk: RevWalkGenerationRangeImpl {
                queue,
                generation_end: generation_range.end,
            },
        }
    }
}

pub(super) type RevWalkAncestors<'a> =
    RevWalkBorrowedIndexIter<'a, CompositeIndex, RevWalkImpl<IndexPosition>>;

#[derive(Clone)]
#[must_use]
pub(super) struct RevWalkImpl<P> {
    queue: RevWalkQueue<P, ()>,
}

impl<I: RevWalkIndex + ?Sized> RevWalk<I> for RevWalkImpl<I::Position> {
    type Item = I::Position;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        while let Some(item) = self.queue.pop() {
            self.queue.skip_while_eq(&item.pos);
            if item.is_wanted() {
                self.queue
                    .extend_wanted(index.adjacent_positions(item.pos), ());
                return Some(item.pos);
            } else if self.queue.items.len() == self.queue.unwanted_count {
                // No more wanted entries to walk
                debug_assert!(!self.queue.items.iter().any(|x| x.is_wanted()));
                return None;
            } else {
                self.queue
                    .extend_unwanted(index.adjacent_positions(item.pos));
            }
        }

        debug_assert_eq!(
            self.queue.items.iter().filter(|x| !x.is_wanted()).count(),
            self.queue.unwanted_count
        );
        None
    }
}

pub(super) type RevWalkAncestorsGenerationRange<'a> =
    RevWalkBorrowedIndexIter<'a, CompositeIndex, RevWalkGenerationRangeImpl<IndexPosition>>;
pub(super) type RevWalkDescendantsGenerationRange = RevWalkOwnedIndexIter<
    RevWalkDescendantsIndex,
    RevWalkGenerationRangeImpl<Reverse<IndexPosition>>,
>;

#[derive(Clone)]
#[must_use]
pub(super) struct RevWalkGenerationRangeImpl<P> {
    // Sort item generations in ascending order
    queue: RevWalkQueue<P, Reverse<RevWalkItemGenerationRange>>,
    generation_end: u32,
}

impl<P: Ord> RevWalkGenerationRangeImpl<P> {
    fn enqueue_wanted_adjacents<I>(&mut self, index: &I, pos: P, gen: RevWalkItemGenerationRange)
    where
        I: RevWalkIndex<Position = P> + ?Sized,
    {
        // `gen.start` is incremented from 0, which should never overflow
        if gen.start + 1 >= self.generation_end {
            return;
        }
        let succ_gen = RevWalkItemGenerationRange {
            start: gen.start + 1,
            end: gen.end.saturating_add(1),
        };
        self.queue
            .extend_wanted(index.adjacent_positions(pos), Reverse(succ_gen));
    }
}

impl<I: RevWalkIndex + ?Sized> RevWalk<I> for RevWalkGenerationRangeImpl<I::Position> {
    type Item = I::Position;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        while let Some(item) = self.queue.pop() {
            if let RevWalkWorkItemState::Wanted(Reverse(mut pending_gen)) = item.state {
                let mut some_in_range = pending_gen.contains_end(self.generation_end);
                while let Some(x) = self.queue.pop_eq(&item.pos) {
                    // Merge overlapped ranges to reduce number of the queued items.
                    // For queries like `:(heads-)`, `gen.end` is close to `u32::MAX`, so
                    // ranges can be merged into one. If this is still slow, maybe we can add
                    // special case for upper/lower bounded ranges.
                    if let RevWalkWorkItemState::Wanted(Reverse(gen)) = x.state {
                        some_in_range |= gen.contains_end(self.generation_end);
                        pending_gen = if let Some(merged) = pending_gen.try_merge_end(gen) {
                            merged
                        } else {
                            self.enqueue_wanted_adjacents(index, item.pos, pending_gen);
                            gen
                        };
                    } else {
                        unreachable!("no more unwanted items of the same entry");
                    }
                }
                self.enqueue_wanted_adjacents(index, item.pos, pending_gen);
                if some_in_range {
                    return Some(item.pos);
                }
            } else if self.queue.items.len() == self.queue.unwanted_count {
                // No more wanted entries to walk
                debug_assert!(!self.queue.items.iter().any(|x| x.is_wanted()));
                return None;
            } else {
                self.queue.skip_while_eq(&item.pos);
                self.queue
                    .extend_unwanted(index.adjacent_positions(item.pos));
            }
        }

        debug_assert_eq!(
            self.queue.items.iter().filter(|x| !x.is_wanted()).count(),
            self.queue.unwanted_count
        );
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RevWalkItemGenerationRange {
    start: u32,
    end: u32,
}

impl RevWalkItemGenerationRange {
    /// Translates filter range to item range so that overlapped ranges can be
    /// merged later.
    ///
    /// Example: `generation_range = 1..4`
    /// ```text
    ///     (original)                       (translated)
    ///     0 1 2 3 4                        0 1 2 3 4
    ///       *=====o  generation_range              +  generation_end
    ///     + :     :  item's generation     o=====* :  item's range
    /// ```
    fn from_filter_range(range: Range<u32>) -> Self {
        RevWalkItemGenerationRange {
            start: 0,
            end: u32::saturating_sub(range.end, range.start),
        }
    }

    /// Suppose sorted ranges `self, other`, merges them if overlapped.
    #[must_use]
    fn try_merge_end(self, other: Self) -> Option<Self> {
        (other.start <= self.end).then(|| RevWalkItemGenerationRange {
            start: self.start,
            end: max(self.end, other.end),
        })
    }

    #[must_use]
    fn contains_end(self, end: u32) -> bool {
        self.start < end && end <= self.end
    }
}

/// Walks descendants from the roots, in order of ascending index position.
pub(super) type RevWalkDescendants<'a> =
    RevWalkBorrowedIndexIter<'a, CompositeIndex, RevWalkDescendantsImpl>;

#[derive(Clone)]
#[must_use]
pub(super) struct RevWalkDescendantsImpl {
    candidate_positions: Vec<IndexPosition>,
    root_positions: HashSet<IndexPosition>,
    reachable_positions: HashSet<IndexPosition>,
}

impl RevWalkDescendants<'_> {
    /// Builds a set of index positions reachable from the roots.
    ///
    /// This is equivalent to `.collect()` on the new iterator, but returns the
    /// internal buffer instead.
    pub fn collect_positions_set(mut self) -> HashSet<IndexPosition> {
        self.by_ref().for_each(drop);
        self.walk.reachable_positions
    }
}

impl RevWalk<CompositeIndex> for RevWalkDescendantsImpl {
    type Item = IndexPosition;

    fn next(&mut self, index: &CompositeIndex) -> Option<Self::Item> {
        while let Some(candidate_pos) = self.candidate_positions.pop() {
            if self.root_positions.contains(&candidate_pos)
                || index
                    .entry_by_pos(candidate_pos)
                    .parent_positions()
                    .iter()
                    .any(|parent_pos| self.reachable_positions.contains(parent_pos))
            {
                self.reachable_positions.insert(candidate_pos);
                return Some(candidate_pos);
            }
        }
        None
    }
}

/// Computes ancestors set lazily.
///
/// This is similar to `RevWalk` functionality-wise, but implemented with the
/// different design goals:
///
/// * optimized for dense ancestors set
/// * optimized for testing set membership
/// * no iterator API (which could be implemented on top)
#[derive(Clone, Debug)]
pub(super) struct AncestorsBitSet {
    bitset: Vec<u64>,
    last_visited_bitset_pos: u32,
}

impl AncestorsBitSet {
    /// Creates bit set of the specified capacity.
    pub fn with_capacity(len: u32) -> Self {
        let bitset_len = usize::try_from(u32::div_ceil(len, u64::BITS)).unwrap();
        AncestorsBitSet {
            bitset: vec![0; bitset_len], // request zeroed page
            last_visited_bitset_pos: 0,
        }
    }

    /// Adds head `pos` to the set.
    ///
    /// Panics if the `pos` exceeds the capacity.
    pub fn add_head(&mut self, pos: IndexPosition) {
        let bitset_pos = pos.0 / u64::BITS;
        let bit = 1_u64 << (pos.0 % u64::BITS);
        self.bitset[usize::try_from(bitset_pos).unwrap()] |= bit;
        self.last_visited_bitset_pos = max(self.last_visited_bitset_pos, bitset_pos + 1);
    }

    /// Returns `true` if the given `pos` is ancestors of the heads.
    ///
    /// Panics if the `pos` exceeds the capacity or has not been visited yet.
    pub fn contains(&self, pos: IndexPosition) -> bool {
        let bitset_pos = pos.0 / u64::BITS;
        let bit = 1_u64 << (pos.0 % u64::BITS);
        assert!(bitset_pos >= self.last_visited_bitset_pos);
        self.bitset[usize::try_from(bitset_pos).unwrap()] & bit != 0
    }

    /// Updates set by visiting ancestors until the given `to_visit_pos`.
    pub fn visit_until(&mut self, index: &CompositeIndex, to_visit_pos: IndexPosition) {
        let to_visit_bitset_pos = to_visit_pos.0 / u64::BITS;
        if to_visit_bitset_pos >= self.last_visited_bitset_pos {
            return;
        }
        for visiting_bitset_pos in (to_visit_bitset_pos..self.last_visited_bitset_pos).rev() {
            let mut unvisited_bits = self.bitset[usize::try_from(visiting_bitset_pos).unwrap()];
            while unvisited_bits != 0 {
                let bit_pos = u64::BITS - unvisited_bits.leading_zeros() - 1; // from MSB
                unvisited_bits ^= 1_u64 << bit_pos;
                let current_pos = IndexPosition(visiting_bitset_pos * u64::BITS + bit_pos);
                for parent_pos in index.entry_by_pos(current_pos).parent_positions() {
                    assert!(parent_pos < current_pos);
                    let parent_bitset_pos = parent_pos.0 / u64::BITS;
                    let bit = 1_u64 << (parent_pos.0 % u64::BITS);
                    self.bitset[usize::try_from(parent_bitset_pos).unwrap()] |= bit;
                    if visiting_bitset_pos == parent_bitset_pos {
                        unvisited_bits |= bit;
                    }
                }
            }
        }
        self.last_visited_bitset_pos = to_visit_bitset_pos;
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools as _;

    use super::super::composite::AsCompositeIndex as _;
    use super::super::mutable::DefaultMutableIndex;
    use super::*;
    use crate::backend::{ChangeId, CommitId};

    /// Generator of unique 16-byte CommitId excluding root id
    fn commit_id_generator() -> impl FnMut() -> CommitId {
        let mut iter = (1_u128..).map(|n| CommitId::new(n.to_le_bytes().into()));
        move || iter.next().unwrap()
    }

    /// Generator of unique 16-byte ChangeId excluding root id
    fn change_id_generator() -> impl FnMut() -> ChangeId {
        let mut iter = (1_u128..).map(|n| ChangeId::new(n.to_le_bytes().into()));
        move || iter.next().unwrap()
    }

    fn to_positions_vec(index: &CompositeIndex, commit_ids: &[CommitId]) -> Vec<IndexPosition> {
        commit_ids
            .iter()
            .map(|id| index.commit_id_to_pos(id).unwrap())
            .collect()
    }

    #[test]
    fn test_peekable_rev_walk() {
        let source = EagerRevWalk::new(vec![0, 1, 2, 3].into_iter());
        let mut peekable = source.peekable();
        assert_eq!(peekable.peek(&()), Some(&0));
        assert_eq!(peekable.peek(&()), Some(&0));
        assert_eq!(peekable.next(&()), Some(0));
        assert_eq!(peekable.peeked, None);
        assert_eq!(peekable.next_if(&(), |&v| v == 2), None);
        assert_eq!(peekable.next(&()), Some(1));
        assert_eq!(peekable.next_if(&(), |&v| v == 2), Some(2));
        assert_eq!(peekable.peeked, None);
        assert_eq!(peekable.peek(&()), Some(&3));
        assert_eq!(peekable.next_if(&(), |&v| v == 3), Some(3));
        assert_eq!(peekable.peeked, None);
        assert_eq!(peekable.next(&()), None);
        assert_eq!(peekable.next(&()), None);

        let source = EagerRevWalk::new((vec![] as Vec<i32>).into_iter());
        let mut peekable = source.peekable();
        assert_eq!(peekable.peek(&()), None);
        assert_eq!(peekable.next(&()), None);
    }

    #[test]
    fn test_filter_rev_walk() {
        let source = EagerRevWalk::new(vec![0, 1, 2, 3, 4].into_iter());
        let mut filtered = source.filter(|_, &v| v & 1 == 0);
        assert_eq!(filtered.next(&()), Some(0));
        assert_eq!(filtered.next(&()), Some(2));
        assert_eq!(filtered.next(&()), Some(4));
        assert_eq!(filtered.next(&()), None);
        assert_eq!(filtered.next(&()), None);
    }

    #[test]
    fn test_walk_ancestors() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
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
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_2.clone()]);

        let walk_commit_ids = |wanted: &[CommitId], unwanted: &[CommitId]| {
            let index = index.as_composite();
            RevWalkBuilder::new(index)
                .wanted_heads(to_positions_vec(index, wanted))
                .unwanted_roots(to_positions_vec(index, unwanted))
                .ancestors()
                .map(|pos| index.entry_by_pos(pos).commit_id())
                .collect_vec()
        };

        // No wanted commits
        assert!(walk_commit_ids(&[], &[]).is_empty());
        // Simple linear walk to roo
        assert_eq!(
            walk_commit_ids(&[id_4.clone()], &[]),
            vec![id_4.clone(), id_1.clone(), id_0.clone()]
        );
        // Commits that are both wanted and unwanted are not walked
        assert_eq!(walk_commit_ids(&[id_0.clone()], &[id_0.clone()]), vec![]);
        // Commits that are listed twice are only walked once
        assert_eq!(
            walk_commit_ids(&[id_0.clone(), id_0.clone()], &[]),
            vec![id_0.clone()]
        );
        // If a commit and its ancestor are both wanted, the ancestor still gets walked
        // only once
        assert_eq!(
            walk_commit_ids(&[id_0.clone(), id_1.clone()], &[]),
            vec![id_1.clone(), id_0.clone()]
        );
        // Ancestors of both wanted and unwanted commits are not walked
        assert_eq!(
            walk_commit_ids(&[id_2.clone()], &[id_1.clone()]),
            vec![id_2.clone()]
        );
        // Same as above, but the opposite order, to make sure that order in index
        // doesn't matter
        assert_eq!(
            walk_commit_ids(&[id_1.clone()], &[id_2.clone()]),
            vec![id_1.clone()]
        );
        // Two wanted nodes
        assert_eq!(
            walk_commit_ids(&[id_1.clone(), id_2.clone()], &[]),
            vec![id_2.clone(), id_1.clone(), id_0.clone()]
        );
        // Order of output doesn't depend on order of input
        assert_eq!(
            walk_commit_ids(&[id_2.clone(), id_1.clone()], &[]),
            vec![id_2.clone(), id_1.clone(), id_0]
        );
        // Two wanted nodes that share an unwanted ancestor
        assert_eq!(
            walk_commit_ids(&[id_5.clone(), id_3.clone()], &[id_2]),
            vec![id_5, id_4, id_3, id_1]
        );
    }

    #[test]
    fn test_walk_ancestors_until_roots() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        //   7
        // 6 |
        // 5 |
        // 4 |
        // | 3
        // | 2
        // |/
        // 1
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        let id_7 = CommitId::from_hex("777777");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone()]);
        index.add_commit_data(id_6.clone(), new_change_id(), &[id_5.clone()]);
        index.add_commit_data(id_7.clone(), new_change_id(), &[id_3.clone()]);

        let index = index.as_composite();
        let make_iter = |heads: &[CommitId], roots: &[CommitId]| {
            RevWalkBuilder::new(index)
                .wanted_heads(to_positions_vec(index, heads))
                .ancestors_until_roots(to_positions_vec(index, roots))
        };
        let to_commit_id = |pos| index.entry_by_pos(pos).commit_id();

        let mut iter = make_iter(&[id_6.clone(), id_7.clone()], &[id_3.clone()]);
        assert_eq!(iter.walk.queue.items.len(), 2);
        assert_eq!(iter.next().map(to_commit_id), Some(id_7.clone()));
        assert_eq!(iter.next().map(to_commit_id), Some(id_6.clone()));
        assert_eq!(iter.next().map(to_commit_id), Some(id_5.clone()));
        assert_eq!(iter.walk.queue.items.len(), 2);
        assert_eq!(iter.next().map(to_commit_id), Some(id_4.clone()));
        assert_eq!(iter.walk.queue.items.len(), 1); // id_1 shouldn't be queued
        assert_eq!(iter.next().map(to_commit_id), Some(id_3.clone()));
        assert_eq!(iter.walk.queue.items.len(), 0); // id_2 shouldn't be queued
        assert!(iter.next().is_none());

        let iter = make_iter(&[id_6.clone(), id_7.clone(), id_2.clone()], &[id_3.clone()]);
        assert_eq!(iter.walk.queue.items.len(), 2); // id_2 shouldn't be queued

        let iter = make_iter(&[id_6.clone(), id_7.clone()], &[]);
        assert!(iter.walk.queue.items.is_empty()); // no ids should be queued
    }

    #[test]
    fn test_walk_ancestors_filtered_by_generation() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 8 6
        // | |
        // 7 5
        // |/|
        // 4 |
        // | 3
        // 2 |
        // |/
        // 1
        // |
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        let id_7 = CommitId::from_hex("777777");
        let id_8 = CommitId::from_hex("888888");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_3.clone()]);
        index.add_commit_data(id_6.clone(), new_change_id(), &[id_5.clone()]);
        index.add_commit_data(id_7.clone(), new_change_id(), &[id_4.clone()]);
        index.add_commit_data(id_8.clone(), new_change_id(), &[id_7.clone()]);

        let walk_commit_ids = |wanted: &[CommitId], unwanted: &[CommitId], range: Range<u32>| {
            let index = index.as_composite();
            RevWalkBuilder::new(index)
                .wanted_heads(to_positions_vec(index, wanted))
                .unwanted_roots(to_positions_vec(index, unwanted))
                .ancestors_filtered_by_generation(range)
                .map(|pos| index.entry_by_pos(pos).commit_id())
                .collect_vec()
        };

        // Empty generation bounds
        assert_eq!(walk_commit_ids(&[&id_8].map(Clone::clone), &[], 0..0), []);
        assert_eq!(
            walk_commit_ids(&[&id_8].map(Clone::clone), &[], Range { start: 2, end: 1 }),
            []
        );

        // Simple generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_2].map(Clone::clone), &[], 0..3),
            [&id_2, &id_1, &id_0].map(Clone::clone)
        );

        // Ancestors may be walked with different generations
        assert_eq!(
            walk_commit_ids(&[&id_6].map(Clone::clone), &[], 2..4),
            [&id_4, &id_3, &id_2, &id_1].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_5].map(Clone::clone), &[], 2..3),
            [&id_2, &id_1].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_5, &id_7].map(Clone::clone), &[], 2..3),
            [&id_2, &id_1].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_7, &id_8].map(Clone::clone), &[], 0..2),
            [&id_8, &id_7, &id_4].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_6, &id_7].map(Clone::clone), &[], 0..3),
            [&id_7, &id_6, &id_5, &id_4, &id_3, &id_2].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_6, &id_7].map(Clone::clone), &[], 2..3),
            [&id_4, &id_3, &id_2].map(Clone::clone)
        );

        // Ancestors of both wanted and unwanted commits are not walked
        assert_eq!(
            walk_commit_ids(&[&id_5].map(Clone::clone), &[&id_2].map(Clone::clone), 1..5),
            [&id_4, &id_3].map(Clone::clone)
        );
    }

    #[test]
    #[allow(clippy::redundant_clone)] // allow id_n.clone()
    fn test_walk_ancestors_filtered_by_generation_range_merging() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // Long linear history with some short branches
        let ids = (0..11)
            .map(|n| CommitId::try_from_hex(&format!("{n:06x}")).unwrap())
            .collect_vec();
        index.add_commit_data(ids[0].clone(), new_change_id(), &[]);
        for i in 1..ids.len() {
            index.add_commit_data(ids[i].clone(), new_change_id(), &[ids[i - 1].clone()]);
        }
        let id_branch5_0 = CommitId::from_hex("050000");
        let id_branch5_1 = CommitId::from_hex("050001");
        index.add_commit_data(id_branch5_0.clone(), new_change_id(), &[ids[5].clone()]);
        index.add_commit_data(
            id_branch5_1.clone(),
            new_change_id(),
            &[id_branch5_0.clone()],
        );

        let walk_commit_ids = |wanted: &[CommitId], range: Range<u32>| {
            let index = index.as_composite();
            RevWalkBuilder::new(index)
                .wanted_heads(to_positions_vec(index, wanted))
                .ancestors_filtered_by_generation(range)
                .map(|pos| index.entry_by_pos(pos).commit_id())
                .collect_vec()
        };

        // Multiple non-overlapping generation ranges to track:
        // 9->6: 3..5, 6: 0..2
        assert_eq!(
            walk_commit_ids(&[&ids[9], &ids[6]].map(Clone::clone), 4..6),
            [&ids[5], &ids[4], &ids[2], &ids[1]].map(Clone::clone)
        );

        // Multiple non-overlapping generation ranges to track, and merged later:
        // 10->7: 3..5, 7: 0..2
        // 10->6: 4..6, 7->6, 1..3, 6: 0..2
        assert_eq!(
            walk_commit_ids(&[&ids[10], &ids[7], &ids[6]].map(Clone::clone), 5..7),
            [&ids[5], &ids[4], &ids[2], &ids[1], &ids[0]].map(Clone::clone)
        );

        // Merge range with sub-range (1..4 + 2..3 should be 1..4, not 1..3):
        // 8,7,6->5::1..4, B5_1->5::2..3
        assert_eq!(
            walk_commit_ids(
                &[&ids[8], &ids[7], &ids[6], &id_branch5_1].map(Clone::clone),
                5..6
            ),
            [&ids[3], &ids[2], &ids[1]].map(Clone::clone)
        );
    }

    #[test]
    fn test_walk_descendants_filtered_by_generation() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 8 6
        // | |
        // 7 5
        // |/|
        // 4 |
        // | 3
        // 2 |
        // |/
        // 1
        // |
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        let id_7 = CommitId::from_hex("777777");
        let id_8 = CommitId::from_hex("888888");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_3.clone()]);
        index.add_commit_data(id_6.clone(), new_change_id(), &[id_5.clone()]);
        index.add_commit_data(id_7.clone(), new_change_id(), &[id_4.clone()]);
        index.add_commit_data(id_8.clone(), new_change_id(), &[id_7.clone()]);

        let visible_heads = [&id_6, &id_8].map(Clone::clone);
        let walk_commit_ids = |roots: &[CommitId], heads: &[CommitId], range: Range<u32>| {
            let index = index.as_composite();
            RevWalkBuilder::new(index)
                .wanted_heads(to_positions_vec(index, heads))
                .descendants_filtered_by_generation(to_positions_vec(index, roots), range)
                .map(|Reverse(pos)| index.entry_by_pos(pos).commit_id())
                .collect_vec()
        };

        // Empty generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_0].map(Clone::clone), &visible_heads, 0..0),
            []
        );
        assert_eq!(
            walk_commit_ids(
                &[&id_8].map(Clone::clone),
                &visible_heads,
                Range { start: 2, end: 1 }
            ),
            []
        );

        // Full generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_0].map(Clone::clone), &visible_heads, 0..u32::MAX),
            [&id_0, &id_1, &id_2, &id_3, &id_4, &id_5, &id_6, &id_7, &id_8].map(Clone::clone)
        );

        // Simple generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_3].map(Clone::clone), &visible_heads, 0..3),
            [&id_3, &id_5, &id_6].map(Clone::clone)
        );

        // Descendants may be walked with different generations
        assert_eq!(
            walk_commit_ids(&[&id_0].map(Clone::clone), &visible_heads, 2..4),
            [&id_2, &id_3, &id_4, &id_5].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_1].map(Clone::clone), &visible_heads, 2..3),
            [&id_4, &id_5].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_3].map(Clone::clone), &visible_heads, 2..3),
            [&id_5, &id_6, &id_7].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_4].map(Clone::clone), &visible_heads, 0..2),
            [&id_2, &id_4, &id_5, &id_7].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_3].map(Clone::clone), &visible_heads, 0..3),
            [&id_2, &id_3, &id_4, &id_5, &id_6, &id_7].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_3].map(Clone::clone), &visible_heads, 2..3),
            [&id_5, &id_6, &id_7].map(Clone::clone)
        );

        // Roots set contains entries unreachable from heads
        assert_eq!(
            walk_commit_ids(
                &[&id_2, &id_3].map(Clone::clone),
                &[&id_8].map(Clone::clone),
                0..3
            ),
            [&id_2, &id_4, &id_7].map(Clone::clone)
        );
    }

    #[test]
    fn test_ancestors_bit_set() {
        let mut new_commit_id = commit_id_generator();
        let mut new_change_id = change_id_generator();
        let mut mutable_index = DefaultMutableIndex::full(16, 16);

        // F      F = 256
        // |\     E = 193,194,195,..,254
        // E | D  D = 192,255
        // | |/   C = 66,68,70,..,190
        // B C    B = 65,67,69,..,189,191
        // |/     A = 0,1,2,..,64
        // A
        let id_a0 = new_commit_id();
        mutable_index.add_commit_data(id_a0.clone(), new_change_id(), &[]);
        let id_a64 = (1..=64).fold(id_a0.clone(), |parent_id, i| {
            assert_eq!(mutable_index.as_composite().num_commits(), i);
            let id = new_commit_id();
            mutable_index.add_commit_data(id.clone(), new_change_id(), &[parent_id]);
            id
        });
        let (id_b189, id_c190) = (65..=190).step_by(2).fold(
            (id_a64.clone(), id_a64.clone()),
            |(parent_id_b, parent_id_c), i| {
                assert_eq!(mutable_index.as_composite().num_commits(), i);
                let id_b = new_commit_id();
                let id_c = new_commit_id();
                mutable_index.add_commit_data(id_b.clone(), new_change_id(), &[parent_id_b]);
                mutable_index.add_commit_data(id_c.clone(), new_change_id(), &[parent_id_c]);
                (id_b, id_c)
            },
        );
        let id_b191 = new_commit_id();
        mutable_index.add_commit_data(id_b191.clone(), new_change_id(), &[id_b189]);
        let id_d192 = new_commit_id();
        mutable_index.add_commit_data(id_d192.clone(), new_change_id(), &[id_c190.clone()]);
        let id_e254 = (193..=254).fold(id_b191.clone(), |parent_id, i| {
            assert_eq!(mutable_index.as_composite().num_commits(), i);
            let id = new_commit_id();
            mutable_index.add_commit_data(id.clone(), new_change_id(), &[parent_id]);
            id
        });
        let id_d255 = new_commit_id();
        mutable_index.add_commit_data(id_d255.clone(), new_change_id(), &[id_d192.clone()]);
        let id_f256 = new_commit_id();
        mutable_index.add_commit_data(
            id_f256.clone(),
            new_change_id(),
            &[id_c190.clone(), id_e254.clone()],
        );
        assert_eq!(mutable_index.as_composite().num_commits(), 257);

        let index = mutable_index.as_composite();
        let to_pos = |id: &CommitId| index.commit_id_to_pos(id).unwrap();
        let new_ancestors_set = |heads: &[&CommitId]| {
            let mut set = AncestorsBitSet::with_capacity(index.num_commits());
            for &id in heads {
                set.add_head(to_pos(id));
            }
            set
        };

        // Nothing reachable
        let set = new_ancestors_set(&[]);
        assert_eq!(set.last_visited_bitset_pos, 0);
        for pos in (0..=256).map(IndexPosition) {
            assert!(!set.contains(pos), "{pos:?} should be unreachable");
        }

        // All reachable
        let mut set = new_ancestors_set(&[&id_f256, &id_d255]);
        assert_eq!(set.last_visited_bitset_pos, 5);
        set.visit_until(index, to_pos(&id_f256));
        assert_eq!(set.last_visited_bitset_pos, 4);
        assert!(set.contains(to_pos(&id_f256)));
        set.visit_until(index, to_pos(&id_d192));
        assert_eq!(set.last_visited_bitset_pos, 3);
        assert!(set.contains(to_pos(&id_e254)));
        assert!(set.contains(to_pos(&id_d255)));
        assert!(set.contains(to_pos(&id_d192)));
        set.visit_until(index, to_pos(&id_a0));
        assert_eq!(set.last_visited_bitset_pos, 0);
        set.visit_until(index, to_pos(&id_f256)); // should be noop
        assert_eq!(set.last_visited_bitset_pos, 0);
        for pos in (0..=256).map(IndexPosition) {
            assert!(set.contains(pos), "{pos:?} should be reachable");
        }

        // A, B, C, E, F are reachable
        let mut set = new_ancestors_set(&[&id_f256]);
        assert_eq!(set.last_visited_bitset_pos, 5);
        set.visit_until(index, to_pos(&id_f256));
        assert_eq!(set.last_visited_bitset_pos, 4);
        assert!(set.contains(to_pos(&id_f256)));
        set.visit_until(index, to_pos(&id_d192));
        assert_eq!(set.last_visited_bitset_pos, 3);
        assert!(!set.contains(to_pos(&id_d255)));
        assert!(!set.contains(to_pos(&id_d192)));
        set.visit_until(index, to_pos(&id_c190));
        assert_eq!(set.last_visited_bitset_pos, 2);
        assert!(set.contains(to_pos(&id_c190)));
        set.visit_until(index, to_pos(&id_a64));
        assert_eq!(set.last_visited_bitset_pos, 1);
        assert!(set.contains(to_pos(&id_b191)));
        assert!(set.contains(to_pos(&id_a64)));
        set.visit_until(index, to_pos(&id_a0));
        assert_eq!(set.last_visited_bitset_pos, 0);
        assert!(set.contains(to_pos(&id_a0)));

        // A, C, D are reachable
        let mut set = new_ancestors_set(&[&id_d255]);
        assert_eq!(set.last_visited_bitset_pos, 4);
        assert!(!set.contains(to_pos(&id_f256)));
        set.visit_until(index, to_pos(&id_e254));
        assert_eq!(set.last_visited_bitset_pos, 3);
        assert!(!set.contains(to_pos(&id_e254)));
        set.visit_until(index, to_pos(&id_d255));
        assert_eq!(set.last_visited_bitset_pos, 3);
        assert!(set.contains(to_pos(&id_d255)));
        set.visit_until(index, to_pos(&id_b191));
        assert_eq!(set.last_visited_bitset_pos, 2);
        assert!(!set.contains(to_pos(&id_b191)));
        set.visit_until(index, to_pos(&id_c190));
        assert_eq!(set.last_visited_bitset_pos, 2);
        assert!(set.contains(to_pos(&id_c190)));
        set.visit_until(index, to_pos(&id_a0));
        assert_eq!(set.last_visited_bitset_pos, 0);
        assert!(set.contains(to_pos(&id_a64)));
        assert!(set.contains(to_pos(&id_a0)));
    }
}
