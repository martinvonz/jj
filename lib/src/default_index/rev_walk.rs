// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

#![allow(missing_docs)]

use std::cmp::{max, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::iter::FusedIterator;
use std::ops::Range;

use smallvec::SmallVec;

use super::composite::CompositeIndex;
use super::entry::{IndexEntry, IndexPosition, SmallIndexPositionsVec};

trait RevWalkIndex<'a> {
    type Position: Copy + Ord;
    type AdjacentPositions: IntoIterator<Item = Self::Position>;

    fn entry_by_pos(&self, pos: Self::Position) -> IndexEntry<'a>;
    fn adjacent_positions(&self, entry: &IndexEntry<'_>) -> Self::AdjacentPositions;
}

impl<'a> RevWalkIndex<'a> for CompositeIndex<'a> {
    type Position = IndexPosition;
    type AdjacentPositions = SmallIndexPositionsVec;

    fn entry_by_pos(&self, pos: Self::Position) -> IndexEntry<'a> {
        CompositeIndex::entry_by_pos(self, pos)
    }

    fn adjacent_positions(&self, entry: &IndexEntry<'_>) -> Self::AdjacentPositions {
        entry.parent_positions()
    }
}

#[derive(Clone)]
struct RevWalkDescendantsIndex<'a> {
    index: CompositeIndex<'a>,
    children_map: HashMap<IndexPosition, DescendantIndexPositionsVec>,
}

// See SmallIndexPositionsVec for the array size.
type DescendantIndexPositionsVec = SmallVec<[Reverse<IndexPosition>; 4]>;

impl<'a> RevWalkDescendantsIndex<'a> {
    fn build<'b>(
        index: CompositeIndex<'a>,
        entries: impl IntoIterator<Item = IndexEntry<'b>>,
    ) -> Self {
        // For dense set, it's probably cheaper to use `Vec` instead of `HashMap`.
        let mut children_map: HashMap<IndexPosition, DescendantIndexPositionsVec> = HashMap::new();
        for entry in entries {
            children_map.entry(entry.position()).or_default(); // mark head node
            for parent_pos in entry.parent_positions() {
                let parent = children_map.entry(parent_pos).or_default();
                parent.push(Reverse(entry.position()));
            }
        }

        RevWalkDescendantsIndex {
            index,
            children_map,
        }
    }

    fn contains_pos(&self, pos: IndexPosition) -> bool {
        self.children_map.contains_key(&pos)
    }
}

impl<'a> RevWalkIndex<'a> for RevWalkDescendantsIndex<'a> {
    type Position = Reverse<IndexPosition>;
    type AdjacentPositions = DescendantIndexPositionsVec;

    fn entry_by_pos(&self, pos: Self::Position) -> IndexEntry<'a> {
        self.index.entry_by_pos(pos.0)
    }

    fn adjacent_positions(&self, entry: &IndexEntry<'_>) -> Self::AdjacentPositions {
        self.children_map[&entry.position()].clone()
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

    fn map_wanted<U>(self, f: impl FnOnce(T) -> U) -> RevWalkWorkItem<P, U> {
        RevWalkWorkItem {
            pos: self.pos,
            state: match self.state {
                RevWalkWorkItemState::Wanted(t) => RevWalkWorkItemState::Wanted(f(t)),
                RevWalkWorkItemState::Unwanted => RevWalkWorkItemState::Unwanted,
            },
        }
    }
}

#[derive(Clone)]
struct RevWalkQueue<P, T> {
    items: BinaryHeap<RevWalkWorkItem<P, T>>,
    unwanted_count: usize,
}

impl<P: Ord, T: Ord> RevWalkQueue<P, T> {
    fn new() -> Self {
        Self {
            items: BinaryHeap::new(),
            unwanted_count: 0,
        }
    }

    fn map_wanted<U: Ord>(self, mut f: impl FnMut(T) -> U) -> RevWalkQueue<P, U> {
        RevWalkQueue {
            items: self
                .items
                .into_iter()
                .map(|x| x.map_wanted(&mut f))
                .collect(),
            unwanted_count: self.unwanted_count,
        }
    }

    fn push_wanted(&mut self, pos: P, t: T) {
        let state = RevWalkWorkItemState::Wanted(t);
        self.items.push(RevWalkWorkItem { pos, state });
    }

    fn push_unwanted(&mut self, pos: P) {
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
pub struct RevWalk<'a>(RevWalkImpl<'a, CompositeIndex<'a>>);

impl<'a> RevWalk<'a> {
    pub(super) fn new(index: CompositeIndex<'a>) -> Self {
        let queue = RevWalkQueue::new();
        RevWalk(RevWalkImpl { index, queue })
    }

    pub(super) fn extend_wanted(&mut self, positions: impl IntoIterator<Item = IndexPosition>) {
        self.0.queue.extend_wanted(positions, ());
    }

    pub(super) fn extend_unwanted(&mut self, positions: impl IntoIterator<Item = IndexPosition>) {
        self.0.queue.extend_unwanted(positions);
    }

    /// Filters entries by generation (or depth from the current wanted set.)
    ///
    /// The generation of the current wanted entries starts from 0.
    pub fn filter_by_generation(self, generation_range: Range<u32>) -> RevWalkGenerationRange<'a> {
        RevWalkGenerationRange(RevWalkGenerationRangeImpl::new(
            self.0.index,
            self.0.queue,
            generation_range,
        ))
    }

    /// Walks ancestors until all of the reachable roots in `root_positions` get
    /// visited.
    ///
    /// Use this if you are only interested in descendants of the given roots.
    /// The caller still needs to filter out unwanted entries.
    pub fn take_until_roots(
        self,
        root_positions: &[IndexPosition],
    ) -> impl Iterator<Item = IndexEntry<'a>> + Clone + 'a {
        // We can also make it stop visiting based on the generation number. Maybe
        // it will perform better for unbalanced branchy history.
        // https://github.com/martinvonz/jj/pull/1492#discussion_r1160678325
        let bottom_position = *root_positions.iter().min().unwrap_or(&IndexPosition::MAX);
        self.take_while(move |entry| entry.position() >= bottom_position)
    }

    /// Fully consumes the ancestors and walks back from `root_positions`.
    ///
    /// The returned iterator yields entries in order of ascending index
    /// position.
    pub fn descendants(self, root_positions: &[IndexPosition]) -> RevWalkDescendants<'a> {
        RevWalkDescendants {
            candidate_entries: self.take_until_roots(root_positions).collect(),
            root_positions: root_positions.iter().copied().collect(),
            reachable_positions: HashSet::new(),
        }
    }

    /// Fully consumes the ancestors and walks back from `root_positions` within
    /// `generation_range`.
    ///
    /// The returned iterator yields entries in order of ascending index
    /// position.
    pub fn descendants_filtered_by_generation(
        self,
        root_positions: &[IndexPosition],
        generation_range: Range<u32>,
    ) -> RevWalkDescendantsGenerationRange<'a> {
        let index = self.0.index;
        let entries = self.take_until_roots(root_positions);
        let descendants_index = RevWalkDescendantsIndex::build(index, entries);
        let mut queue = RevWalkQueue::new();
        for &pos in root_positions {
            // Do not add unreachable roots which shouldn't be visited
            if descendants_index.contains_pos(pos) {
                queue.push_wanted(Reverse(pos), ());
            }
        }
        RevWalkDescendantsGenerationRange(RevWalkGenerationRangeImpl::new(
            descendants_index,
            queue,
            generation_range,
        ))
    }
}

impl<'a> Iterator for RevWalk<'a> {
    type Item = IndexEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

#[derive(Clone)]
struct RevWalkImpl<'a, I: RevWalkIndex<'a>> {
    index: I,
    queue: RevWalkQueue<I::Position, ()>,
}

impl<'a, I: RevWalkIndex<'a>> RevWalkImpl<'a, I> {
    fn next(&mut self) -> Option<IndexEntry<'a>> {
        while let Some(item) = self.queue.pop() {
            self.queue.skip_while_eq(&item.pos);
            if item.is_wanted() {
                let entry = self.index.entry_by_pos(item.pos);
                self.queue
                    .extend_wanted(self.index.adjacent_positions(&entry), ());
                return Some(entry);
            } else if self.queue.items.len() == self.queue.unwanted_count {
                // No more wanted entries to walk
                debug_assert!(!self.queue.items.iter().any(|x| x.is_wanted()));
                return None;
            } else {
                let entry = self.index.entry_by_pos(item.pos);
                self.queue
                    .extend_unwanted(self.index.adjacent_positions(&entry));
            }
        }

        debug_assert_eq!(
            self.queue.items.iter().filter(|x| !x.is_wanted()).count(),
            self.queue.unwanted_count
        );
        None
    }
}

#[derive(Clone)]
pub struct RevWalkGenerationRange<'a>(RevWalkGenerationRangeImpl<'a, CompositeIndex<'a>>);

impl<'a> Iterator for RevWalkGenerationRange<'a> {
    type Item = IndexEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

#[derive(Clone)]
pub struct RevWalkDescendantsGenerationRange<'a>(
    RevWalkGenerationRangeImpl<'a, RevWalkDescendantsIndex<'a>>,
);

impl<'a> Iterator for RevWalkDescendantsGenerationRange<'a> {
    type Item = IndexEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

#[derive(Clone)]
struct RevWalkGenerationRangeImpl<'a, I: RevWalkIndex<'a>> {
    index: I,
    // Sort item generations in ascending order
    queue: RevWalkQueue<I::Position, Reverse<RevWalkItemGenerationRange>>,
    generation_end: u32,
}

impl<'a, I: RevWalkIndex<'a>> RevWalkGenerationRangeImpl<'a, I> {
    fn new(index: I, queue: RevWalkQueue<I::Position, ()>, generation_range: Range<u32>) -> Self {
        // Translate filter range to item ranges so that overlapped ranges can be
        // merged later.
        //
        // Example: `generation_range = 1..4`
        //     (original)                       (translated)
        //     0 1 2 3 4                        0 1 2 3 4
        //       *=====o  generation_range              +  generation_end
        //     + :     :  item's generation     o=====* :  item's range
        let item_range = RevWalkItemGenerationRange {
            start: 0,
            end: u32::saturating_sub(generation_range.end, generation_range.start),
        };
        RevWalkGenerationRangeImpl {
            index,
            queue: queue.map_wanted(|()| Reverse(item_range)),
            generation_end: generation_range.end,
        }
    }

    fn enqueue_wanted_adjacents(
        &mut self,
        entry: &IndexEntry<'_>,
        gen: RevWalkItemGenerationRange,
    ) {
        // `gen.start` is incremented from 0, which should never overflow
        if gen.start + 1 >= self.generation_end {
            return;
        }
        let succ_gen = RevWalkItemGenerationRange {
            start: gen.start + 1,
            end: gen.end.saturating_add(1),
        };
        self.queue
            .extend_wanted(self.index.adjacent_positions(entry), Reverse(succ_gen));
    }

    fn next(&mut self) -> Option<IndexEntry<'a>> {
        while let Some(item) = self.queue.pop() {
            if let RevWalkWorkItemState::Wanted(Reverse(mut pending_gen)) = item.state {
                let entry = self.index.entry_by_pos(item.pos);
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
                            self.enqueue_wanted_adjacents(&entry, pending_gen);
                            gen
                        };
                    } else {
                        unreachable!("no more unwanted items of the same entry");
                    }
                }
                self.enqueue_wanted_adjacents(&entry, pending_gen);
                if some_in_range {
                    return Some(entry);
                }
            } else if self.queue.items.len() == self.queue.unwanted_count {
                // No more wanted entries to walk
                debug_assert!(!self.queue.items.iter().any(|x| x.is_wanted()));
                return None;
            } else {
                let entry = self.index.entry_by_pos(item.pos);
                self.queue.skip_while_eq(&item.pos);
                self.queue
                    .extend_unwanted(self.index.adjacent_positions(&entry));
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
#[derive(Clone)]
pub struct RevWalkDescendants<'a> {
    candidate_entries: Vec<IndexEntry<'a>>,
    root_positions: HashSet<IndexPosition>,
    reachable_positions: HashSet<IndexPosition>,
}

impl RevWalkDescendants<'_> {
    /// Builds a set of index positions reachable from the roots.
    ///
    /// This is equivalent to `.map(|entry| entry.position()).collect()` on
    /// the new iterator, but returns the internal buffer instead.
    pub fn collect_positions_set(mut self) -> HashSet<IndexPosition> {
        self.by_ref().for_each(drop);
        self.reachable_positions
    }
}

impl<'a> Iterator for RevWalkDescendants<'a> {
    type Item = IndexEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(candidate) = self.candidate_entries.pop() {
            if self.root_positions.contains(&candidate.position())
                || candidate
                    .parent_positions()
                    .iter()
                    .any(|parent_pos| self.reachable_positions.contains(parent_pos))
            {
                self.reachable_positions.insert(candidate.position());
                return Some(candidate);
            }
        }
        None
    }
}

impl FusedIterator for RevWalkDescendants<'_> {}
