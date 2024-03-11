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
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::{fmt, iter};

use itertools::Itertools;

use super::rev_walk::{EagerRevWalk, PeekableRevWalk, RevWalk, RevWalkBuilder};
use super::revset_graph_iterator::RevsetGraphWalk;
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

type BoxedPredicateFn<'a> = Box<dyn FnMut(&CompositeIndex, IndexPosition) -> bool + 'a>;
pub(super) type BoxedRevWalk<'a> = Box<dyn RevWalk<CompositeIndex, Item = IndexPosition> + 'a>;

trait ToPredicateFn: fmt::Debug {
    /// Creates function that tests if the given entry is included in the set.
    ///
    /// The predicate function is evaluated in order of `RevsetIterator`.
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a;
}

impl<T: ToPredicateFn + ?Sized> ToPredicateFn for Box<T> {
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        <T as ToPredicateFn>::to_predicate_fn(self)
    }
}

trait InternalRevset: fmt::Debug + ToPredicateFn {
    // All revsets currently iterate in order of descending index position
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a;

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a;
}

impl<T: InternalRevset + ?Sized> InternalRevset for Box<T> {
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        <T as InternalRevset>::positions(self)
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

impl<I: AsCompositeIndex + Clone> RevsetImpl<I> {
    fn new(inner: Box<dyn InternalRevset>, index: I) -> Self {
        Self { inner, index }
    }

    fn positions(&self) -> impl Iterator<Item = IndexPosition> + '_ {
        self.inner.positions().attach(self.index.as_composite())
    }

    pub fn iter_graph_impl(
        &self,
        skip_transitive_edges: bool,
    ) -> impl Iterator<Item = (CommitId, Vec<RevsetGraphEdge>)> {
        let index = self.index.clone();
        let walk = self.inner.positions();
        let mut graph_walk = RevsetGraphWalk::new(walk, skip_transitive_edges);
        iter::from_fn(move || graph_walk.next(index.as_composite()))
    }
}

impl<I> fmt::Debug for RevsetImpl<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevsetImpl")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<I: AsCompositeIndex + Clone> Revset for RevsetImpl<I> {
    fn iter<'a>(&self) -> Box<dyn Iterator<Item = CommitId> + 'a>
    where
        Self: 'a,
    {
        let index = self.index.clone();
        let mut walk = self.inner.positions();
        Box::new(iter::from_fn(move || {
            let index = index.as_composite();
            let pos = walk.next(index)?;
            Some(index.entry_by_pos(pos).commit_id())
        }))
    }

    fn commit_change_ids<'a>(&self) -> Box<dyn Iterator<Item = (CommitId, ChangeId)> + 'a>
    where
        Self: 'a,
    {
        let index = self.index.clone();
        let mut walk = self.inner.positions();
        Box::new(iter::from_fn(move || {
            let index = index.as_composite();
            let pos = walk.next(index)?;
            let entry = index.entry_by_pos(pos);
            Some((entry.commit_id(), entry.change_id()))
        }))
    }

    fn iter_graph<'a>(&self) -> Box<dyn Iterator<Item = (CommitId, Vec<RevsetGraphEdge>)> + 'a>
    where
        Self: 'a,
    {
        let skip_transitive_edges = true;
        Box::new(self.iter_graph_impl(skip_transitive_edges))
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

    fn containing_fn<'a>(&self) -> Box<dyn Fn(&CommitId) -> bool + 'a>
    where
        Self: 'a,
    {
        let positions = PositionsAccumulator::new(self.index.clone(), self.inner.positions());
        Box::new(move |commit_id| positions.contains(commit_id))
    }
}

/// Incrementally consumes `RevWalk` of the revset collecting positions.
struct PositionsAccumulator<'a, I> {
    index: I,
    inner: RefCell<PositionsAccumulatorInner<'a>>,
}

impl<'a, I: AsCompositeIndex> PositionsAccumulator<'a, I> {
    fn new(index: I, walk: BoxedRevWalk<'a>) -> Self {
        let inner = RefCell::new(PositionsAccumulatorInner {
            walk,
            consumed_positions: Vec::new(),
        });
        PositionsAccumulator { index, inner }
    }

    /// Checks whether the commit is in the revset.
    fn contains(&self, commit_id: &CommitId) -> bool {
        let index = self.index.as_composite();
        let Some(position) = index.commit_id_to_pos(commit_id) else {
            return false;
        };

        let mut inner = self.inner.borrow_mut();
        inner.consume_to(index, position);
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
struct PositionsAccumulatorInner<'a> {
    walk: BoxedRevWalk<'a>,
    consumed_positions: Vec<IndexPosition>,
}

impl PositionsAccumulatorInner<'_> {
    /// Consumes `RevWalk` to a desired position but not deeper.
    fn consume_to(&mut self, index: &CompositeIndex, desired_position: IndexPosition) {
        let last_position = self.consumed_positions.last();
        if last_position.map_or(false, |&pos| pos <= desired_position) {
            return;
        }
        while let Some(position) = self.walk.next(index) {
            self.consumed_positions.push(position);
            if position <= desired_position {
                return;
            }
        }
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
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        Box::new(EagerRevWalk::new(self.positions.clone().into_iter()))
    }

    fn into_predicate<'a>(self: Box<Self>) -> Box<dyn ToPredicateFn + 'a>
    where
        Self: 'a,
    {
        self
    }
}

impl ToPredicateFn for EagerRevset {
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        let walk = EagerRevWalk::new(self.positions.clone().into_iter());
        predicate_fn_from_rev_walk(walk)
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
    W: RevWalk<CompositeIndex, Item = IndexPosition> + Clone,
{
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        Box::new(self.walk.clone())
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
    W: RevWalk<CompositeIndex, Item = IndexPosition> + Clone,
{
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        predicate_fn_from_rev_walk(self.walk.clone())
    }
}

fn predicate_fn_from_rev_walk<'a, W>(walk: W) -> BoxedPredicateFn<'a>
where
    W: RevWalk<CompositeIndex, Item = IndexPosition> + 'a,
{
    let mut walk = walk.peekable();
    Box::new(move |index, entry_pos| {
        while walk.next_if(index, |&pos| pos > entry_pos).is_some() {
            continue;
        }
        walk.next_if(index, |&pos| pos == entry_pos).is_some()
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
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        let mut p = self.predicate.to_predicate_fn();
        Box::new(
            self.candidates
                .positions()
                .filter(move |index, &pos| p(index, pos)),
        )
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
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        let mut p1 = self.candidates.to_predicate_fn();
        let mut p2 = self.predicate.to_predicate_fn();
        Box::new(move |index, pos| p1(index, pos) && p2(index, pos))
    }
}

#[derive(Debug)]
struct NotInPredicate<S>(S);

impl<S: ToPredicateFn> ToPredicateFn for NotInPredicate<S> {
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        let mut p = self.0.to_predicate_fn();
        Box::new(move |index, pos| !p(index, pos))
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
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        Box::new(union_by(
            self.set1.positions(),
            self.set2.positions(),
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
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        let mut p1 = self.set1.to_predicate_fn();
        let mut p2 = self.set2.to_predicate_fn();
        Box::new(move |index, pos| p1(index, pos) || p2(index, pos))
    }
}

/// `RevWalk` node that merges two sorted walk nodes.
///
/// The input items should be sorted in ascending order by the `cmp` function.
struct UnionRevWalk<I: ?Sized, W1: RevWalk<I>, W2: RevWalk<I>, C> {
    walk1: PeekableRevWalk<I, W1>,
    walk2: PeekableRevWalk<I, W2>,
    cmp: C,
}

impl<I, W1, W2, C> RevWalk<I> for UnionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I>,
    W2: RevWalk<I, Item = W1::Item>,
    C: FnMut(&W1::Item, &W2::Item) -> Ordering,
{
    type Item = W1::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        match (self.walk1.peek(index), self.walk2.peek(index)) {
            (None, _) => self.walk2.next(index),
            (_, None) => self.walk1.next(index),
            (Some(item1), Some(item2)) => match (self.cmp)(item1, item2) {
                Ordering::Less => self.walk1.next(index),
                Ordering::Equal => {
                    self.walk2.next(index);
                    self.walk1.next(index)
                }
                Ordering::Greater => self.walk2.next(index),
            },
        }
    }
}

fn union_by<I, W1, W2, C>(walk1: W1, walk2: W2, cmp: C) -> UnionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I>,
    W2: RevWalk<I, Item = W1::Item>,
    C: FnMut(&W1::Item, &W2::Item) -> Ordering,
{
    UnionRevWalk {
        walk1: walk1.peekable(),
        walk2: walk2.peekable(),
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
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        Box::new(intersection_by(
            self.set1.positions(),
            self.set2.positions(),
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
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        let mut p1 = self.set1.to_predicate_fn();
        let mut p2 = self.set2.to_predicate_fn();
        Box::new(move |index, pos| p1(index, pos) && p2(index, pos))
    }
}

/// `RevWalk` node that intersects two sorted walk nodes.
///
/// The input items should be sorted in ascending order by the `cmp` function.
struct IntersectionRevWalk<I: ?Sized, W1: RevWalk<I>, W2: RevWalk<I>, C> {
    walk1: PeekableRevWalk<I, W1>,
    walk2: PeekableRevWalk<I, W2>,
    cmp: C,
}

impl<I, W1, W2, C> RevWalk<I> for IntersectionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I>,
    W2: RevWalk<I>,
    C: FnMut(&W1::Item, &W2::Item) -> Ordering,
{
    type Item = W1::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        loop {
            match (self.walk1.peek(index), self.walk2.peek(index)) {
                (None, _) => {
                    return None;
                }
                (_, None) => {
                    return None;
                }
                (Some(item1), Some(item2)) => match (self.cmp)(item1, item2) {
                    Ordering::Less => {
                        self.walk1.next(index);
                    }
                    Ordering::Equal => {
                        self.walk2.next(index);
                        return self.walk1.next(index);
                    }
                    Ordering::Greater => {
                        self.walk2.next(index);
                    }
                },
            }
        }
    }
}

fn intersection_by<I, W1, W2, C>(walk1: W1, walk2: W2, cmp: C) -> IntersectionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I>,
    W2: RevWalk<I>,
    C: FnMut(&W1::Item, &W2::Item) -> Ordering,
{
    IntersectionRevWalk {
        walk1: walk1.peekable(),
        walk2: walk2.peekable(),
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
    fn positions<'a>(&self) -> BoxedRevWalk<'a>
    where
        Self: 'a,
    {
        Box::new(difference_by(
            self.set1.positions(),
            self.set2.positions(),
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
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        let mut p1 = self.set1.to_predicate_fn();
        let mut p2 = self.set2.to_predicate_fn();
        Box::new(move |index, pos| p1(index, pos) && !p2(index, pos))
    }
}

/// `RevWalk` node that subtracts `walk2` items from `walk1`.
///
/// The input items should be sorted in ascending order by the `cmp` function.
struct DifferenceRevWalk<I: ?Sized, W1: RevWalk<I>, W2: RevWalk<I>, C> {
    walk1: PeekableRevWalk<I, W1>,
    walk2: PeekableRevWalk<I, W2>,
    cmp: C,
}

impl<I, W1, W2, C> RevWalk<I> for DifferenceRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I>,
    W2: RevWalk<I>,
    C: FnMut(&W1::Item, &W2::Item) -> Ordering,
{
    type Item = W1::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        loop {
            match (self.walk1.peek(index), self.walk2.peek(index)) {
                (None, _) => {
                    return None;
                }
                (_, None) => {
                    return self.walk1.next(index);
                }
                (Some(item1), Some(item2)) => match (self.cmp)(item1, item2) {
                    Ordering::Less => {
                        return self.walk1.next(index);
                    }
                    Ordering::Equal => {
                        self.walk2.next(index);
                        self.walk1.next(index);
                    }
                    Ordering::Greater => {
                        self.walk2.next(index);
                    }
                },
            }
        }
    }
}

fn difference_by<I, W1, W2, C>(walk1: W1, walk2: W2, cmp: C) -> DifferenceRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I>,
    W2: RevWalk<I>,
    C: FnMut(&W1::Item, &W2::Item) -> Ordering,
{
    DifferenceRevWalk {
        walk1: walk1.peekable(),
        walk2: walk2.peekable(),
        cmp,
    }
}

pub fn evaluate<I: AsCompositeIndex + Clone>(
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
    index: &'index CompositeIndex,
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
                let head_positions = head_set.positions().attach(index);
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
                let root_positions = root_set.positions().attach(index).collect_vec();
                // Pre-filter heads so queries like 'immutable_heads()..' can
                // terminate early. immutable_heads() usually includes some
                // visible heads, which can be trivially rejected.
                let head_set = self.evaluate(heads)?;
                let head_positions = difference_by(
                    head_set.positions(),
                    EagerRevWalk::new(root_positions.iter().copied()),
                    |pos1, pos2| pos1.cmp(pos2).reverse(),
                )
                .attach(index);
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
                let root_positions = root_set.positions().attach(index);
                let head_set = self.evaluate(heads)?;
                let head_positions = head_set.positions().attach(index);
                let builder = RevWalkBuilder::new(index).wanted_heads(head_positions);
                if generation_from_roots == &(1..2) {
                    let root_positions: HashSet<_> = root_positions.collect();
                    let walk = builder
                        .ancestors_until_roots(root_positions.iter().copied())
                        .detach();
                    let candidates = RevWalkRevset { walk };
                    let predicate = as_pure_predicate_fn(move |index, pos| {
                        index
                            .entry_by_pos(pos)
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
                    index.heads_pos(candidate_set.positions().attach(index).collect());
                let positions = head_positions.into_iter().rev().collect();
                Ok(Box::new(EagerRevset { positions }))
            }
            ResolvedExpression::Roots(candidates) => {
                let mut positions = self
                    .evaluate(candidates)?
                    .positions()
                    .attach(index)
                    .collect_vec();
                let filled = RevWalkBuilder::new(index)
                    .wanted_heads(positions.iter().copied())
                    .descendants(positions.iter().copied())
                    .collect_positions_set();
                positions.retain(|&pos| {
                    !index
                        .entry_by_pos(pos)
                        .parent_positions()
                        .iter()
                        .any(|parent| filled.contains(parent))
                });
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

        let make_rev_item = |pos| {
            let entry = self.index.entry_by_pos(pos);
            let commit = self.store.get_commit(&entry.commit_id()).unwrap();
            Reverse(Item {
                timestamp: commit.committer().timestamp.timestamp,
                pos: entry.position(),
            })
        };

        // Maintain min-heap containing the latest (greatest) count items. For small
        // count and large candidate set, this is probably cheaper than building vec
        // and applying selection algorithm.
        let mut candidate_iter = candidate_set
            .positions()
            .attach(self.index)
            .map(make_rev_item)
            .fuse();
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
    F: Fn(&CompositeIndex, IndexPosition) -> bool + Clone,
{
    fn to_predicate_fn<'a>(&self) -> BoxedPredicateFn<'a>
    where
        Self: 'a,
    {
        Box::new(self.0.clone())
    }
}

fn as_pure_predicate_fn<F>(f: F) -> PurePredicateFn<F>
where
    F: Fn(&CompositeIndex, IndexPosition) -> bool + Clone,
{
    PurePredicateFn(f)
}

fn box_pure_predicate_fn<'a>(
    f: impl Fn(&CompositeIndex, IndexPosition) -> bool + Clone + 'a,
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
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                parent_count_range.contains(&entry.num_parents())
            })
        }
        RevsetFilterPredicate::Description(pattern) => {
            let pattern = pattern.clone();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id()).unwrap();
                pattern.matches(commit.description())
            })
        }
        RevsetFilterPredicate::Author(pattern) => {
            let pattern = pattern.clone();
            // TODO: Make these functions that take a needle to search for accept some
            // syntax for specifying whether it's a regex and whether it's
            // case-sensitive.
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id()).unwrap();
                pattern.matches(&commit.author().name) || pattern.matches(&commit.author().email)
            })
        }
        RevsetFilterPredicate::Committer(pattern) => {
            let pattern = pattern.clone();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id()).unwrap();
                pattern.matches(&commit.committer().name)
                    || pattern.matches(&commit.committer().email)
            })
        }
        RevsetFilterPredicate::File(paths) => {
            // TODO: Add support for globs and other formats
            let matcher: Rc<dyn Matcher> = if let Some(paths) = paths {
                Rc::new(PrefixMatcher::new(paths))
            } else {
                Rc::new(EverythingMatcher)
            };
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                has_diff_from_parent(&store, index, &entry, matcher.as_ref())
            })
        }
        RevsetFilterPredicate::HasConflict => box_pure_predicate_fn(move |index, pos| {
            let entry = index.entry_by_pos(pos);
            let commit = store.get_commit(&entry.commit_id()).unwrap();
            commit.has_conflict().unwrap()
        }),
    }
}

fn has_diff_from_parent(
    store: &Arc<Store>,
    index: &CompositeIndex,
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

        let index = index.as_composite();
        let get_pos = |id: &CommitId| index.commit_id_to_pos(id).unwrap();
        let make_positions = |ids: &[&CommitId]| ids.iter().copied().map(get_pos).collect_vec();
        let make_set = |ids: &[&CommitId]| -> Box<dyn InternalRevset> {
            let positions = make_positions(ids);
            Box::new(EagerRevset { positions })
        };

        let set = make_set(&[&id_4, &id_3, &id_2, &id_0]);
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_4)));
        assert!(p(index, get_pos(&id_3)));
        assert!(p(index, get_pos(&id_2)));
        assert!(!p(index, get_pos(&id_1)));
        assert!(p(index, get_pos(&id_0)));
        // Uninteresting entries can be skipped
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_3)));
        assert!(!p(index, get_pos(&id_1)));
        assert!(p(index, get_pos(&id_0)));

        let set = FilterRevset {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: as_pure_predicate_fn(|index, pos| {
                index.entry_by_pos(pos).commit_id() != id_4
            }),
        };
        assert_eq!(
            set.positions().attach(index).collect_vec(),
            make_positions(&[&id_2, &id_0])
        );
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_4)));
        assert!(!p(index, get_pos(&id_3)));
        assert!(p(index, get_pos(&id_2)));
        assert!(!p(index, get_pos(&id_1)));
        assert!(p(index, get_pos(&id_0)));

        // Intersection by FilterRevset
        let set = FilterRevset {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.positions().attach(index).collect_vec(),
            make_positions(&[&id_2])
        );
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_4)));
        assert!(!p(index, get_pos(&id_3)));
        assert!(p(index, get_pos(&id_2)));
        assert!(!p(index, get_pos(&id_1)));
        assert!(!p(index, get_pos(&id_0)));

        let set = UnionRevset {
            set1: make_set(&[&id_4, &id_2]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.positions().attach(index).collect_vec(),
            make_positions(&[&id_4, &id_3, &id_2, &id_1])
        );
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_4)));
        assert!(p(index, get_pos(&id_3)));
        assert!(p(index, get_pos(&id_2)));
        assert!(p(index, get_pos(&id_1)));
        assert!(!p(index, get_pos(&id_0)));

        let set = IntersectionRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.positions().attach(index).collect_vec(),
            make_positions(&[&id_2])
        );
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_4)));
        assert!(!p(index, get_pos(&id_3)));
        assert!(p(index, get_pos(&id_2)));
        assert!(!p(index, get_pos(&id_1)));
        assert!(!p(index, get_pos(&id_0)));

        let set = DifferenceRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            set.positions().attach(index).collect_vec(),
            make_positions(&[&id_4, &id_0])
        );
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_4)));
        assert!(!p(index, get_pos(&id_3)));
        assert!(!p(index, get_pos(&id_2)));
        assert!(!p(index, get_pos(&id_1)));
        assert!(p(index, get_pos(&id_0)));
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

        let index = index.as_composite();
        let get_pos = |id: &CommitId| index.commit_id_to_pos(id).unwrap();
        let make_positions = |ids: &[&CommitId]| ids.iter().copied().map(get_pos).collect_vec();
        let make_set = |ids: &[&CommitId]| -> Box<dyn InternalRevset> {
            let positions = make_positions(ids);
            Box::new(EagerRevset { positions })
        };

        let full_set = make_set(&[&id_4, &id_3, &id_2, &id_1, &id_0]);

        // Consumes entries incrementally
        let positions_accum = PositionsAccumulator::new(index, full_set.positions());

        assert!(positions_accum.contains(&id_3));
        assert_eq!(positions_accum.consumed_len(), 2);

        assert!(positions_accum.contains(&id_0));
        assert_eq!(positions_accum.consumed_len(), 5);

        assert!(positions_accum.contains(&id_3));
        assert_eq!(positions_accum.consumed_len(), 5);

        // Does not consume positions for unknown commits
        let positions_accum = PositionsAccumulator::new(index, full_set.positions());

        assert!(!positions_accum.contains(&CommitId::from_hex("999999")));
        assert_eq!(positions_accum.consumed_len(), 0);

        // Does not consume without necessity
        let set = make_set(&[&id_3, &id_2, &id_1]);
        let positions_accum = PositionsAccumulator::new(index, set.positions());

        assert!(!positions_accum.contains(&id_4));
        assert_eq!(positions_accum.consumed_len(), 1);

        assert!(positions_accum.contains(&id_3));
        assert_eq!(positions_accum.consumed_len(), 1);

        assert!(!positions_accum.contains(&id_0));
        assert_eq!(positions_accum.consumed_len(), 3);

        assert!(positions_accum.contains(&id_1));
    }
}
