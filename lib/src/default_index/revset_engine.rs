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

use std::cell::RefCell;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap, HashSet};
use std::fmt;
use std::iter::Peekable;
use std::ops::Range;
use std::sync::Arc;

use itertools::Itertools;

use super::rev_walk::{RevWalk, RevWalkBuilder};
use super::revset_graph_iterator::RevsetGraphIterator;
use crate::backend::{ChangeId, CommitId, MillisSinceEpoch};
use crate::default_index::{AsCompositeIndex, CompositeIndex, IndexEntry, IndexPosition};
use crate::matchers::{EverythingMatcher, Matcher, PrefixMatcher, Visit};
use crate::repo_path::RepoPath;
use crate::revset::{
    ResolvedExpression, ResolvedPredicateExpression, Revset, RevsetEvaluationError,
    RevsetFilterPredicate, GENERATION_RANGE_FULL,
};
use crate::revset_graph::RevsetGraphEdge;
use crate::rewrite;
use crate::store::Store;

trait ToPredicateFn: fmt::Debug {
    /// Creates function that tests if the given entry is included in the set.
    ///
    /// The predicate function is evaluated in order of `RevsetIterator`.
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a>;
}

impl<T: ToPredicateFn + ?Sized> ToPredicateFn for Box<T> {
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        <T as ToPredicateFn>::to_predicate_fn(self, index)
    }
}

trait InternalRevset: fmt::Debug + ToPredicateFn {
    // All revsets currently iterate in order of descending index position
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a>;

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a>;

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a;
}

impl<T: InternalRevset + ?Sized> InternalRevset for Box<T> {
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        <T as InternalRevset>::entries(self, index)
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        <T as InternalRevset>::positions(self, index)
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        <T as InternalRevset>::into_predicate(*self)
    }
}

pub struct RevsetImpl<I> {
    inner: Box<dyn InternalRevset>,
    index: I,
}

impl<I: AsCompositeIndex> RevsetImpl<I> {
    fn new(inner: Box<dyn InternalRevset>, index: I) -> Self {
        Self { inner, index }
    }

    fn entries(&self) -> Box<dyn Iterator<Item = IndexEntry<'_>> + '_> {
        self.inner.entries(self.index.as_composite())
    }

    fn positions(&self) -> Box<dyn Iterator<Item = IndexPosition> + '_> {
        self.inner.positions(self.index.as_composite())
    }

    pub fn iter_graph_impl(&self) -> RevsetGraphIterator<'_, '_> {
        RevsetGraphIterator::new(self.index.as_composite(), self.entries())
    }
}

impl<I> fmt::Debug for RevsetImpl<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevsetImpl")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<I: AsCompositeIndex> Revset for RevsetImpl<I> {
    fn iter(&self) -> Box<dyn Iterator<Item = CommitId> + '_> {
        Box::new(self.entries().map(|index_entry| index_entry.commit_id()))
    }

    fn commit_change_ids(&self) -> Box<dyn Iterator<Item = (CommitId, ChangeId)> + '_> {
        Box::new(
            self.entries()
                .map(|index_entry| (index_entry.commit_id(), index_entry.change_id())),
        )
    }

    fn iter_graph(&self) -> Box<dyn Iterator<Item = (CommitId, Vec<RevsetGraphEdge>)> + '_> {
        Box::new(self.iter_graph_impl())
    }

    fn is_empty(&self) -> bool {
        self.positions().next().is_none()
    }

    fn count_estimate(&self) -> (usize, Option<usize>) {
        if cfg!(feature = "testing") {
            // Exercise the estimation feature in tests. (If we ever have a Revset
            // implementation in production code that returns estimates, we can probably
            // remove this and rewrite the associated tests.)
            let count = self.positions().take(10).count();
            if count < 10 {
                (count, Some(count))
            } else {
                (10, None)
            }
        } else {
            let count = self.positions().count();
            (count, Some(count))
        }
    }

    fn containing_fn(&self) -> Box<dyn Fn(&CommitId) -> bool + '_> {
        let positions = PositionsAccumulator::new(self.index.as_composite(), self.positions());
        Box::new(move |commit_id| positions.contains(commit_id))
    }
}

/// Incrementally consumes positions iterator of the revset collecting
/// positions.
struct PositionsAccumulator<'revset, 'index> {
    index: CompositeIndex<'index>,
    inner: RefCell<PositionsAccumulatorInner<'revset>>,
}

impl<'revset, 'index> PositionsAccumulator<'revset, 'index> {
    fn new(
        index: CompositeIndex<'index>,
        positions_iter: Box<dyn Iterator<Item = IndexPosition> + 'revset>,
    ) -> Self {
        let inner = RefCell::new(PositionsAccumulatorInner {
            positions_iter: Some(positions_iter),
            consumed_positions: Vec::new(),
        });
        PositionsAccumulator { index, inner }
    }

    /// Checks whether the commit is in the revset.
    fn contains(&self, commit_id: &CommitId) -> bool {
        let Some(position) = self.index.commit_id_to_pos(commit_id) else {
            return false;
        };

        let mut inner = self.inner.borrow_mut();
        if let Some(last_position) = inner.consumed_positions.last() {
            if last_position > &position {
                inner.consume_to(position);
            }
        } else {
            inner.consume_to(position);
        }

        inner
            .consumed_positions
            .binary_search_by(|p| p.cmp(&position).reverse())
            .is_ok()
    }

    #[cfg(test)]
    fn consumed_len(&self) -> usize {
        self.inner.borrow().consumed_positions.len()
    }
}

/// Helper struct for [`PositionsAccumulator`] to simplify interior mutability.
struct PositionsAccumulatorInner<'revset> {
    positions_iter: Option<Box<dyn Iterator<Item = IndexPosition> + 'revset>>,
    consumed_positions: Vec<IndexPosition>,
}

impl<'revset> PositionsAccumulatorInner<'revset> {
    /// Consumes positions iterator to a desired position but not deeper.
    fn consume_to(&mut self, desired_position: IndexPosition) {
        let Some(iter) = self.positions_iter.as_mut() else {
            return;
        };
        for position in iter {
            self.consumed_positions.push(position);
            if position <= desired_position {
                return;
            }
        }
        self.positions_iter = None;
    }
}

#[derive(Debug)]
struct EagerRevset {
    positions: Vec<IndexPosition>,
}

impl EagerRevset {
    pub const fn empty() -> Self {
        EagerRevset {
            positions: Vec::new(),
        }
    }
}

impl InternalRevset for EagerRevset {
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        let entries = self
            .positions
            .iter()
            .map(move |&pos| index.entry_by_pos(pos));
        Box::new(entries)
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        _index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        Box::new(self.positions.iter().copied())
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl ToPredicateFn for EagerRevset {
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        _index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        predicate_fn_from_positions(self.positions.iter().copied())
    }
}

struct RevWalkRevset<W> {
    walk: W,
}

impl<W> fmt::Debug for RevWalkRevset<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevWalkRevset").finish_non_exhaustive()
    }
}

impl<W> InternalRevset for RevWalkRevset<W>
where
    W: for<'index> RevWalk<CompositeIndex<'index>, Item = IndexPosition> + Clone,
{
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        let positions = self.walk.clone().attach(index);
        Box::new(positions.map(move |pos| index.entry_by_pos(pos)))
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        Box::new(self.walk.clone().attach(index))
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl<W> ToPredicateFn for RevWalkRevset<W>
where
    W: for<'index> RevWalk<CompositeIndex<'index>, Item = IndexPosition> + Clone,
{
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let positions = self.walk.clone().attach(index);
        predicate_fn_from_positions(positions)
    }
}

fn predicate_fn_from_positions<'iter>(
    iter: impl Iterator<Item = IndexPosition> + 'iter,
) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'iter> {
    let mut iter = iter.fuse().peekable();
    Box::new(move |entry| {
        while iter.next_if(|&pos| pos > entry.position()).is_some() {
            continue;
        }
        iter.next_if(|&pos| pos == entry.position()).is_some()
    })
}

#[derive(Debug)]
struct FilterRevset<S, P> {
    candidates: S,
    predicate: P,
}

impl<S, P> InternalRevset for FilterRevset<S, P>
where
    S: InternalRevset,
    P: ToPredicateFn,
{
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        let p = self.predicate.to_predicate_fn(index);
        Box::new(self.candidates.entries(index).filter(p))
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        Box::new(self.entries(index).map(|entry| entry.position()))
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl<S, P> ToPredicateFn for FilterRevset<S, P>
where
    S: ToPredicateFn,
    P: ToPredicateFn,
{
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let mut p1 = self.candidates.to_predicate_fn(index);
        let mut p2 = self.predicate.to_predicate_fn(index);
        Box::new(move |entry| p1(entry) && p2(entry))
    }
}

#[derive(Debug)]
struct NotInPredicate<S>(S);

impl<S: ToPredicateFn> ToPredicateFn for NotInPredicate<S> {
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let mut p = self.0.to_predicate_fn(index);
        Box::new(move |entry| !p(entry))
    }
}

#[derive(Debug)]
struct UnionRevset<S1, S2> {
    set1: S1,
    set2: S2,
}

impl<S1, S2> InternalRevset for UnionRevset<S1, S2>
where
    S1: InternalRevset,
    S2: InternalRevset,
{
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        Box::new(union_by(
            self.set1.entries(index),
            self.set2.entries(index),
            |entry1, entry2| entry1.position().cmp(&entry2.position()).reverse(),
        ))
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        Box::new(union_by(
            self.set1.positions(index),
            self.set2.positions(index),
            |pos1, pos2| pos1.cmp(pos2).reverse(),
        ))
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl<S1, S2> ToPredicateFn for UnionRevset<S1, S2>
where
    S1: ToPredicateFn,
    S2: ToPredicateFn,
{
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let mut p1 = self.set1.to_predicate_fn(index);
        let mut p2 = self.set2.to_predicate_fn(index);
        Box::new(move |entry| p1(entry) || p2(entry))
    }
}

/// Iterator that merges two sorted iterators.
///
/// The input items should be sorted in ascending order by the `cmp` function.
struct UnionByIterator<I1: Iterator, I2: Iterator, C> {
    iter1: Peekable<I1>,
    iter2: Peekable<I2>,
    cmp: C,
}

impl<I1, I2, C> Iterator for UnionByIterator<I1, I2, C>
where
    I1: Iterator,
    I2: Iterator<Item = I1::Item>,
    C: FnMut(&I1::Item, &I2::Item) -> Ordering,
{
    type Item = I1::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.iter1.peek(), self.iter2.peek()) {
            (None, _) => self.iter2.next(),
            (_, None) => self.iter1.next(),
            (Some(item1), Some(item2)) => match (self.cmp)(item1, item2) {
                Ordering::Less => self.iter1.next(),
                Ordering::Equal => {
                    self.iter2.next();
                    self.iter1.next()
                }
                Ordering::Greater => self.iter2.next(),
            },
        }
    }
}

fn union_by<I1, I2, C>(
    iter1: I1,
    iter2: I2,
    cmp: C,
) -> UnionByIterator<I1::IntoIter, I2::IntoIter, C>
where
    I1: IntoIterator,
    I2: IntoIterator<Item = I1::Item>,
    C: FnMut(&I1::Item, &I2::Item) -> Ordering,
{
    UnionByIterator {
        iter1: iter1.into_iter().peekable(),
        iter2: iter2.into_iter().peekable(),
        cmp,
    }
}

#[derive(Debug)]
struct IntersectionRevset<S1, S2> {
    set1: S1,
    set2: S2,
}

impl<S1, S2> InternalRevset for IntersectionRevset<S1, S2>
where
    S1: InternalRevset,
    S2: InternalRevset,
{
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        Box::new(intersection_by(
            self.set1.entries(index),
            self.set2.positions(index),
            |entry1, pos2| entry1.position().cmp(pos2).reverse(),
        ))
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        Box::new(intersection_by(
            self.set1.positions(index),
            self.set2.positions(index),
            |pos1, pos2| pos1.cmp(pos2).reverse(),
        ))
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl<S1, S2> ToPredicateFn for IntersectionRevset<S1, S2>
where
    S1: ToPredicateFn,
    S2: ToPredicateFn,
{
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let mut p1 = self.set1.to_predicate_fn(index);
        let mut p2 = self.set2.to_predicate_fn(index);
        Box::new(move |entry| p1(entry) && p2(entry))
    }
}

/// Iterator that intersects two sorted iterators.
///
/// The input items should be sorted in ascending order by the `cmp` function.
struct IntersectionByIterator<I1: Iterator, I2: Iterator, C> {
    iter1: Peekable<I1>,
    iter2: Peekable<I2>,
    cmp: C,
}

impl<I1, I2, C> Iterator for IntersectionByIterator<I1, I2, C>
where
    I1: Iterator,
    I2: Iterator,
    C: FnMut(&I1::Item, &I2::Item) -> Ordering,
{
    type Item = I1::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.iter1.peek(), self.iter2.peek()) {
                (None, _) => {
                    return None;
                }
                (_, None) => {
                    return None;
                }
                (Some(item1), Some(item2)) => match (self.cmp)(item1, item2) {
                    Ordering::Less => {
                        self.iter1.next();
                    }
                    Ordering::Equal => {
                        self.iter2.next();
                        return self.iter1.next();
                    }
                    Ordering::Greater => {
                        self.iter2.next();
                    }
                },
            }
        }
    }
}

fn intersection_by<I1, I2, C>(
    iter1: I1,
    iter2: I2,
    cmp: C,
) -> IntersectionByIterator<I1::IntoIter, I2::IntoIter, C>
where
    I1: IntoIterator,
    I2: IntoIterator,
    C: FnMut(&I1::Item, &I2::Item) -> Ordering,
{
    IntersectionByIterator {
        iter1: iter1.into_iter().peekable(),
        iter2: iter2.into_iter().peekable(),
        cmp,
    }
}

#[derive(Debug)]
struct DifferenceRevset<S1, S2> {
    // The minuend (what to subtract from)
    set1: S1,
    // The subtrahend (what to subtract)
    set2: S2,
}

impl<S1, S2> InternalRevset for DifferenceRevset<S1, S2>
where
    S1: InternalRevset,
    S2: InternalRevset,
{
    fn entries<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexEntry<'index>> + 'a> {
        Box::new(difference_by(
            self.set1.entries(index),
            self.set2.positions(index),
            |entry1, pos2| entry1.position().cmp(pos2).reverse(),
        ))
    }

    fn positions<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn Iterator<Item = IndexPosition> + 'a> {
        Box::new(difference_by(
            self.set1.positions(index),
            self.set2.positions(index),
            |pos1, pos2| pos1.cmp(pos2).reverse(),
        ))
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl<S1, S2> ToPredicateFn for DifferenceRevset<S1, S2>
where
    S1: ToPredicateFn,
    S2: ToPredicateFn,
{
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let mut p1 = self.set1.to_predicate_fn(index);
        let mut p2 = self.set2.to_predicate_fn(index);
        Box::new(move |entry| p1(entry) && !p2(entry))
    }
}

/// Iterator that subtracts `iter2` items from `iter1`.
///
/// The input items should be sorted in ascending order by the `cmp` function.
struct DifferenceByIterator<I1: Iterator, I2: Iterator, C> {
    iter1: Peekable<I1>,
    iter2: Peekable<I2>,
    cmp: C,
}

impl<I1, I2, C> Iterator for DifferenceByIterator<I1, I2, C>
where
    I1: Iterator,
    I2: Iterator,
    C: FnMut(&I1::Item, &I2::Item) -> Ordering,
{
    type Item = I1::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.iter1.peek(), self.iter2.peek()) {
                (None, _) => {
                    return None;
                }
                (_, None) => {
                    return self.iter1.next();
                }
                (Some(item1), Some(item2)) => match (self.cmp)(item1, item2) {
                    Ordering::Less => {
                        return self.iter1.next();
                    }
                    Ordering::Equal => {
                        self.iter2.next();
                        self.iter1.next();
                    }
                    Ordering::Greater => {
                        self.iter2.next();
                    }
                },
            }
        }
    }
}

fn difference_by<I1, I2, C>(
    iter1: I1,
    iter2: I2,
    cmp: C,
) -> DifferenceByIterator<I1::IntoIter, I2::IntoIter, C>
where
    I1: IntoIterator,
    I2: IntoIterator,
    C: FnMut(&I1::Item, &I2::Item) -> Ordering,
{
    DifferenceByIterator {
        iter1: iter1.into_iter().peekable(),
        iter2: iter2.into_iter().peekable(),
        cmp,
    }
}

pub fn evaluate<I: AsCompositeIndex>(
    expression: &ResolvedExpression,
    store: &Arc<Store>,
    index: I,
) -> Result<RevsetImpl<I>, RevsetEvaluationError> {
    let context = EvaluationContext {
        store: store.clone(),
        index: index.as_composite(),
    };
    let internal_revset = context.evaluate(expression)?;
    Ok(RevsetImpl::new(internal_revset, index))
}

struct EvaluationContext<'index> {
    store: Arc<Store>,
    index: CompositeIndex<'index>,
}

fn to_u32_generation_range(range: &Range<u64>) -> Result<Range<u32>, RevsetEvaluationError> {
    let start = range.start.try_into().map_err(|_| {
        RevsetEvaluationError::Other(format!(
            "Lower bound of generation ({}) is too large",
            range.start
        ))
    })?;
    let end = range.end.try_into().unwrap_or(u32::MAX);
    Ok(start..end)
}

impl<'index> EvaluationContext<'index> {
    fn evaluate(
        &self,
        expression: &ResolvedExpression,
    ) -> Result<Box<dyn InternalRevset>, RevsetEvaluationError> {
        let index = self.index;
        match expression {
            ResolvedExpression::Commits(commit_ids) => {
                Ok(Box::new(self.revset_for_commit_ids(commit_ids)))
            }
            ResolvedExpression::Ancestors { heads, generation } => {
                let head_set = self.evaluate(heads)?;
                let head_positions = head_set.positions(index);
                let builder = RevWalkBuilder::new(index).wanted_heads(head_positions);
                if generation == &GENERATION_RANGE_FULL {
                    let walk = builder.ancestors().detach();
                    Ok(Box::new(RevWalkRevset { walk }))
                } else {
                    let generation = to_u32_generation_range(generation)?;
                    let walk = builder
                        .ancestors_filtered_by_generation(generation)
                        .detach();
                    Ok(Box::new(RevWalkRevset { walk }))
                }
            }
            ResolvedExpression::Range {
                roots,
                heads,
                generation,
            } => {
                let root_set = self.evaluate(roots)?;
                let root_positions = root_set.positions(index).collect_vec();
                // Pre-filter heads so queries like 'immutable_heads()..' can
                // terminate early. immutable_heads() usually includes some
                // visible heads, which can be trivially rejected.
                let head_set = self.evaluate(heads)?;
                let head_positions = difference_by(
                    head_set.positions(index),
                    root_positions.iter().copied(),
                    |pos1, pos2| pos1.cmp(pos2).reverse(),
                );
                let builder = RevWalkBuilder::new(index)
                    .wanted_heads(head_positions)
                    .unwanted_roots(root_positions);
                if generation == &GENERATION_RANGE_FULL {
                    let walk = builder.ancestors().detach();
                    Ok(Box::new(RevWalkRevset { walk }))
                } else {
                    let generation = to_u32_generation_range(generation)?;
                    let walk = builder
                        .ancestors_filtered_by_generation(generation)
                        .detach();
                    Ok(Box::new(RevWalkRevset { walk }))
                }
            }
            ResolvedExpression::DagRange {
                roots,
                heads,
                generation_from_roots,
            } => {
                let root_set = self.evaluate(roots)?;
                let root_positions = root_set.positions(index);
                let head_set = self.evaluate(heads)?;
                let head_positions = head_set.positions(index);
                let builder = RevWalkBuilder::new(index).wanted_heads(head_positions);
                if generation_from_roots == &(1..2) {
                    let root_positions: HashSet<_> = root_positions.collect();
                    let walk = builder
                        .ancestors_until_roots(root_positions.iter().copied())
                        .detach();
                    let candidates = RevWalkRevset { walk };
                    let predicate = as_pure_predicate_fn(move |_index, entry| {
                        entry
                            .parent_positions()
                            .iter()
                            .any(|parent_pos| root_positions.contains(parent_pos))
                    });
                    // TODO: Suppose heads include all visible heads, ToPredicateFn version can be
                    // optimized to only test the predicate()
                    Ok(Box::new(FilterRevset {
                        candidates,
                        predicate,
                    }))
                } else if generation_from_roots == &GENERATION_RANGE_FULL {
                    let mut positions = builder.descendants(root_positions).collect_vec();
                    positions.reverse();
                    Ok(Box::new(EagerRevset { positions }))
                } else {
                    // For small generation range, it might be better to build a reachable map
                    // with generation bit set, which can be calculated incrementally from roots:
                    //   reachable[pos] = (reachable[parent_pos] | ...) << 1
                    let mut positions = builder
                        .descendants_filtered_by_generation(
                            root_positions,
                            to_u32_generation_range(generation_from_roots)?,
                        )
                        .map(|Reverse(pos)| pos)
                        .collect_vec();
                    positions.reverse();
                    Ok(Box::new(EagerRevset { positions }))
                }
            }
            ResolvedExpression::Heads(candidates) => {
                let candidate_set = self.evaluate(candidates)?;
                let head_positions: BTreeSet<_> =
                    index.heads_pos(candidate_set.positions(index).collect());
                let positions = head_positions.into_iter().rev().collect();
                Ok(Box::new(EagerRevset { positions }))
            }
            ResolvedExpression::Roots(candidates) => {
                let candidate_entries = self.evaluate(candidates)?.entries(index).collect_vec();
                let candidate_positions = candidate_entries
                    .iter()
                    .map(|entry| entry.position())
                    .collect_vec();
                let filled = RevWalkBuilder::new(index)
                    .wanted_heads(candidate_positions.iter().copied())
                    .descendants(candidate_positions)
                    .collect_positions_set();
                let mut positions = vec![];
                for candidate in candidate_entries {
                    if !candidate
                        .parent_positions()
                        .iter()
                        .any(|parent| filled.contains(parent))
                    {
                        positions.push(candidate.position());
                    }
                }
                Ok(Box::new(EagerRevset { positions }))
            }
            ResolvedExpression::Latest { candidates, count } => {
                let candidate_set = self.evaluate(candidates)?;
                Ok(Box::new(
                    self.take_latest_revset(candidate_set.as_ref(), *count),
                ))
            }
            ResolvedExpression::Union(expression1, expression2) => {
                let set1 = self.evaluate(expression1)?;
                let set2 = self.evaluate(expression2)?;
                Ok(Box::new(UnionRevset { set1, set2 }))
            }
            ResolvedExpression::FilterWithin {
                candidates,
                predicate,
            } => Ok(Box::new(FilterRevset {
                candidates: self.evaluate(candidates)?,
                predicate: self.evaluate_predicate(predicate)?,
            })),
            ResolvedExpression::Intersection(expression1, expression2) => {
                let set1 = self.evaluate(expression1)?;
                let set2 = self.evaluate(expression2)?;
                Ok(Box::new(IntersectionRevset { set1, set2 }))
            }
            ResolvedExpression::Difference(expression1, expression2) => {
                let set1 = self.evaluate(expression1)?;
                let set2 = self.evaluate(expression2)?;
                Ok(Box::new(DifferenceRevset { set1, set2 }))
            }
        }
    }

    fn evaluate_predicate(
        &self,
        expression: &ResolvedPredicateExpression,
    ) -> Result<Box<dyn ToPredicateFn>, RevsetEvaluationError> {
        match expression {
            ResolvedPredicateExpression::Filter(predicate) => {
                Ok(build_predicate_fn(self.store.clone(), predicate))
            }
            ResolvedPredicateExpression::Set(expression) => {
                Ok(self.evaluate(expression)?.into_predicate())
            }
            ResolvedPredicateExpression::NotIn(complement) => {
                let set = self.evaluate_predicate(complement)?;
                Ok(Box::new(NotInPredicate(set)))
            }
            ResolvedPredicateExpression::Union(expression1, expression2) => {
                let set1 = self.evaluate_predicate(expression1)?;
                let set2 = self.evaluate_predicate(expression2)?;
                Ok(Box::new(UnionRevset { set1, set2 }))
            }
        }
    }

    fn revset_for_commit_ids(&self, commit_ids: &[CommitId]) -> EagerRevset {
        let mut positions = commit_ids
            .iter()
            .map(|id| self.index.commit_id_to_pos(id).unwrap())
            .collect_vec();
        positions.sort_unstable_by_key(|&pos| Reverse(pos));
        positions.dedup();
        EagerRevset { positions }
    }

    fn take_latest_revset(&self, candidate_set: &dyn InternalRevset, count: usize) -> EagerRevset {
        if count == 0 {
            return EagerRevset::empty();
        }

        #[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
        struct Item {
            timestamp: MillisSinceEpoch,
            pos: IndexPosition, // tie-breaker
        }

        let make_rev_item = |entry: IndexEntry<'_>| {
            let commit = self.store.get_commit(&entry.commit_id()).unwrap();
            Reverse(Item {
                timestamp: commit.committer().timestamp.timestamp,
                pos: entry.position(),
            })
        };

        // Maintain min-heap containing the latest (greatest) count items. For small
        // count and large candidate set, this is probably cheaper than building vec
        // and applying selection algorithm.
        let mut candidate_iter = candidate_set.entries(self.index).map(make_rev_item).fuse();
        let mut latest_items = BinaryHeap::from_iter(candidate_iter.by_ref().take(count));
        for item in candidate_iter {
            let mut earliest = latest_items.peek_mut().unwrap();
            if earliest.0 < item.0 {
                *earliest = item;
            }
        }

        assert!(latest_items.len() <= count);
        let mut positions = latest_items
            .into_iter()
            .map(|item| item.0.pos)
            .collect_vec();
        positions.sort_unstable_by_key(|&pos| Reverse(pos));
        EagerRevset { positions }
    }
}

struct PurePredicateFn<F>(F);

impl<F> fmt::Debug for PurePredicateFn<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PurePredicateFn").finish_non_exhaustive()
    }
}

impl<F> ToPredicateFn for PurePredicateFn<F>
where
    F: Fn(CompositeIndex<'_>, &IndexEntry<'_>) -> bool,
{
    fn to_predicate_fn<'a, 'index: 'a>(
        &'a self,
        index: CompositeIndex<'index>,
    ) -> Box<dyn FnMut(&IndexEntry<'_>) -> bool + 'a> {
        let f = &self.0;
        Box::new(move |entry| f(index, entry))
    }
}

fn as_pure_predicate_fn<F>(f: F) -> PurePredicateFn<F>
where
    F: Fn(CompositeIndex<'_>, &IndexEntry<'_>) -> bool,
{
    PurePredicateFn(f)
}

fn box_pure_predicate_fn<'a>(
    f: impl Fn(CompositeIndex<'_>, &IndexEntry<'_>) -> bool + 'a,
) -> Box<dyn ToPredicateFn + 'a> {
    Box::new(PurePredicateFn(f))
}

fn build_predicate_fn(
    store: Arc<Store>,
    predicate: &RevsetFilterPredicate,
) -> Box<dyn ToPredicateFn> {
    match predicate {
        RevsetFilterPredicate::ParentCount(parent_count_range) => {
            let parent_count_range = parent_count_range.clone();
            box_pure_predicate_fn(move |_index, entry| {
                parent_count_range.contains(&entry.num_parents())
            })
        }
        RevsetFilterPredicate::Description(pattern) => {
            let pattern = pattern.clone();
            box_pure_predicate_fn(move |_index, entry| {
                let commit = store.get_commit(&entry.commit_id()).unwrap();
                pattern.matches(commit.description())
            })
        }
        RevsetFilterPredicate::Author(pattern) => {
            let pattern = pattern.clone();
            // TODO: Make these functions that take a needle to search for accept some
            // syntax for specifying whether it's a regex and whether it's
            // case-sensitive.
            box_pure_predicate_fn(move |_index, entry| {
                let commit = store.get_commit(&entry.commit_id()).unwrap();
                pattern.matches(&commit.author().name) || pattern.matches(&commit.author().email)
            })
        }
        RevsetFilterPredicate::Committer(pattern) => {
            let pattern = pattern.clone();
            box_pure_predicate_fn(move |_index, entry| {
                let commit = store.get_commit(&entry.commit_id()).unwrap();
                pattern.matches(&commit.committer().name)
                    || pattern.matches(&commit.committer().email)
            })
        }
        RevsetFilterPredicate::File(paths) => {
            // TODO: Add support for globs and other formats
            let matcher: Box<dyn Matcher> = if let Some(paths) = paths {
                Box::new(PrefixMatcher::new(paths))
            } else {
                Box::new(EverythingMatcher)
            };
            box_pure_predicate_fn(move |index, entry| {
                has_diff_from_parent(&store, index, entry, matcher.as_ref())
            })
        }
        RevsetFilterPredicate::HasConflict => box_pure_predicate_fn(move |_index, entry| {
            let commit = store.get_commit(&entry.commit_id()).unwrap();
            commit.has_conflict().unwrap()
        }),
    }
}

fn has_diff_from_parent(
    store: &Arc<Store>,
    index: CompositeIndex<'_>,
    entry: &IndexEntry<'_>,
    matcher: &dyn Matcher,
) -> bool {
    let commit = store.get_commit(&entry.commit_id()).unwrap();
    let parents = commit.parents();
    if let [parent] = parents.as_slice() {
        // Fast path: no need to load the root tree
        let unchanged = commit.tree_id() == parent.tree_id();
        if matcher.visit(RepoPath::root()) == Visit::AllRecursively {
            return !unchanged;
        } else if unchanged {
            return false;
        }
    }
    let from_tree = rewrite::merge_commit_trees_without_repo(store, &index, &parents).unwrap();
    let to_tree = commit.tree().unwrap();
    from_tree.diff(&to_tree, matcher).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_index::DefaultMutableIndex;

    /// Generator of unique 16-byte ChangeId excluding root id
    fn change_id_generator() -> impl FnMut() -> ChangeId {
        let mut iter = (1_u128..).map(|n| ChangeId::new(n.to_le_bytes().into()));
        move || iter.next().unwrap()
    }

    #[test]
    fn test_revset_combinator() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_3.clone()]);

        let get_pos = |id: &CommitId| index.as_composite().commit_id_to_pos(id).unwrap();
        let get_entry = |id: &CommitId| index.as_composite().entry_by_id(id).unwrap();
        let make_positions = |ids: &[&CommitId]| ids.iter().copied().map(get_pos).collect_vec();
        let make_entries = |ids: &[&CommitId]| ids.iter().copied().map(get_entry).collect_vec();
        let make_set = |ids: &[&CommitId]| -> Box<dyn InternalRevset> {
            let positions = make_positions(ids);
            Box::new(EagerRevset { positions })
        };

        let set = make_set(&[&id_4, &id_3, &id_2, &id_0]);
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(p(&get_entry(&id_4)));
        assert!(p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));
        // Uninteresting entries can be skipped
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(p(&get_entry(&id_3)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));

        let set = FilterRevset {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: as_pure_predicate_fn(|_index, entry| entry.commit_id() != id_4),
        };
        assert_eq!(
            set.entries(index.as_composite()).collect_vec(),
            make_entries(&[&id_2, &id_0])
        );
        assert_eq!(
            set.positions(index.as_composite()).collect_vec(),
            make_positions(&[&id_2, &id_0])
        );
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(!p(&get_entry(&id_4)));
        assert!(!p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));

        // Intersection by FilterRevset
        let set = FilterRevset {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.entries(index.as_composite()).collect_vec(),
            make_entries(&[&id_2])
        );
        assert_eq!(
            set.positions(index.as_composite()).collect_vec(),
            make_positions(&[&id_2])
        );
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(!p(&get_entry(&id_4)));
        assert!(!p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(!p(&get_entry(&id_0)));

        let set = UnionRevset {
            set1: make_set(&[&id_4, &id_2]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.entries(index.as_composite()).collect_vec(),
            make_entries(&[&id_4, &id_3, &id_2, &id_1])
        );
        assert_eq!(
            set.positions(index.as_composite()).collect_vec(),
            make_positions(&[&id_4, &id_3, &id_2, &id_1])
        );
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(p(&get_entry(&id_4)));
        assert!(p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(p(&get_entry(&id_1)));
        assert!(!p(&get_entry(&id_0)));

        let set = IntersectionRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.entries(index.as_composite()).collect_vec(),
            make_entries(&[&id_2])
        );
        assert_eq!(
            set.positions(index.as_composite()).collect_vec(),
            make_positions(&[&id_2])
        );
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(!p(&get_entry(&id_4)));
        assert!(!p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(!p(&get_entry(&id_0)));

        let set = DifferenceRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.entries(index.as_composite()).collect_vec(),
            make_entries(&[&id_4, &id_0])
        );
        assert_eq!(
            set.positions(index.as_composite()).collect_vec(),
            make_positions(&[&id_4, &id_0])
        );
        let mut p = set.to_predicate_fn(index.as_composite());
        assert!(p(&get_entry(&id_4)));
        assert!(!p(&get_entry(&id_3)));
        assert!(!p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));
    }

    #[test]
    fn test_positions_accumulator() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_3.clone()]);

        let get_pos = |id: &CommitId| index.as_composite().commit_id_to_pos(id).unwrap();
        let make_positions = |ids: &[&CommitId]| ids.iter().copied().map(get_pos).collect_vec();
        let make_set = |ids: &[&CommitId]| -> Box<dyn InternalRevset> {
            let positions = make_positions(ids);
            Box::new(EagerRevset { positions })
        };

        let full_set = make_set(&[&id_4, &id_3, &id_2, &id_1, &id_0]);

        // Consumes entries incrementally
        let positions_accum = PositionsAccumulator::new(
            index.as_composite(),
            full_set.positions(index.as_composite()),
        );

        assert!(positions_accum.contains(&id_3));
        assert_eq!(positions_accum.consumed_len(), 2);

        assert!(positions_accum.contains(&id_0));
        assert_eq!(positions_accum.consumed_len(), 5);

        assert!(positions_accum.contains(&id_3));
        assert_eq!(positions_accum.consumed_len(), 5);

        // Does not consume positions for unknown commits
        let positions_accum = PositionsAccumulator::new(
            index.as_composite(),
            full_set.positions(index.as_composite()),
        );

        assert!(!positions_accum.contains(&CommitId::from_hex("999999")));
        assert_eq!(positions_accum.consumed_len(), 0);

        // Does not consume without necessity
        let set = make_set(&[&id_3, &id_2, &id_1]);
        let positions_accum =
            PositionsAccumulator::new(index.as_composite(), set.positions(index.as_composite()));

        assert!(!positions_accum.contains(&id_4));
        assert_eq!(positions_accum.consumed_len(), 1);

        assert!(positions_accum.contains(&id_3));
        assert_eq!(positions_accum.consumed_len(), 1);

        assert!(!positions_accum.contains(&id_0));
        assert_eq!(positions_accum.consumed_len(), 3);

        assert!(positions_accum.contains(&id_1));
    }
}
