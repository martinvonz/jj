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
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::collections::BinaryHeap;
use std::collections::HashSet;
use std::fmt;
use std::iter;
use std::ops::Range;
use std::rc::Rc;
use std::str;
use std::sync::Arc;

use futures::StreamExt as _;
use itertools::Itertools;
use pollster::FutureExt as _;

use super::rev_walk::EagerRevWalk;
use super::rev_walk::PeekableRevWalk;
use super::rev_walk::RevWalk;
use super::rev_walk::RevWalkBuilder;
use super::revset_graph_iterator::RevsetGraphWalk;
use crate::backend::BackendError;
use crate::backend::BackendResult;
use crate::backend::ChangeId;
use crate::backend::CommitId;
use crate::backend::MillisSinceEpoch;
use crate::commit::Commit;
use crate::conflicts::materialize_merge_result_to_bytes;
use crate::conflicts::materialize_tree_value;
use crate::conflicts::ConflictMarkerStyle;
use crate::conflicts::MaterializedTreeValue;
use crate::default_index::AsCompositeIndex;
use crate::default_index::CompositeIndex;
use crate::default_index::IndexPosition;
use crate::graph::GraphNode;
use crate::matchers::Matcher;
use crate::matchers::Visit;
use crate::merged_tree::resolve_file_values;
use crate::object_id::ObjectId as _;
use crate::repo_path::RepoPath;
use crate::revset::ResolvedExpression;
use crate::revset::ResolvedPredicateExpression;
use crate::revset::Revset;
use crate::revset::RevsetContainingFn;
use crate::revset::RevsetEvaluationError;
use crate::revset::RevsetFilterPredicate;
use crate::revset::GENERATION_RANGE_FULL;
use crate::rewrite;
use crate::store::Store;
use crate::str_util::StringPattern;
use crate::union_find;

type BoxedPredicateFn<'a> =
    Box<dyn FnMut(&CompositeIndex, IndexPosition) -> Result<bool, RevsetEvaluationError> + 'a>;
pub(super) type BoxedRevWalk<'a> =
    Box<dyn RevWalk<CompositeIndex, Item = Result<IndexPosition, RevsetEvaluationError>> + 'a>;

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

    fn positions(&self) -> impl Iterator<Item = Result<IndexPosition, RevsetEvaluationError>> + '_ {
        self.inner.positions().attach(self.index.as_composite())
    }

    pub fn iter_graph_impl(
        &self,
        skip_transitive_edges: bool,
    ) -> impl Iterator<Item = Result<GraphNode<CommitId>, RevsetEvaluationError>> {
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
    fn iter<'a>(&self) -> Box<dyn Iterator<Item = Result<CommitId, RevsetEvaluationError>> + 'a>
    where
        Self: 'a,
    {
        let index = self.index.clone();
        let mut walk = self
            .inner
            .positions()
            .map(|index, pos| Ok(index.entry_by_pos(pos?).commit_id()));
        Box::new(iter::from_fn(move || walk.next(index.as_composite())))
    }

    fn commit_change_ids<'a>(
        &self,
    ) -> Box<dyn Iterator<Item = Result<(CommitId, ChangeId), RevsetEvaluationError>> + 'a>
    where
        Self: 'a,
    {
        let index = self.index.clone();
        let mut walk = self.inner.positions().map(|index, pos| {
            let entry = index.entry_by_pos(pos?);
            Ok((entry.commit_id(), entry.change_id()))
        });
        Box::new(iter::from_fn(move || walk.next(index.as_composite())))
    }

    fn iter_graph<'a>(
        &self,
    ) -> Box<dyn Iterator<Item = Result<GraphNode<CommitId>, RevsetEvaluationError>> + 'a>
    where
        Self: 'a,
    {
        let skip_transitive_edges = true;
        Box::new(self.iter_graph_impl(skip_transitive_edges))
    }

    fn is_empty(&self) -> bool {
        self.positions().next().is_none()
    }

    fn count_estimate(&self) -> Result<(usize, Option<usize>), RevsetEvaluationError> {
        if cfg!(feature = "testing") {
            // Exercise the estimation feature in tests. (If we ever have a Revset
            // implementation in production code that returns estimates, we can probably
            // remove this and rewrite the associated tests.)
            let count = self
                .positions()
                .take(10)
                .process_results(|iter| iter.count())?;
            if count < 10 {
                Ok((count, Some(count)))
            } else {
                Ok((10, None))
            }
        } else {
            let count = self.positions().process_results(|iter| iter.count())?;
            Ok((count, Some(count)))
        }
    }

    fn containing_fn<'a>(&self) -> Box<RevsetContainingFn<'a>>
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
    fn contains(&self, commit_id: &CommitId) -> Result<bool, RevsetEvaluationError> {
        let index = self.index.as_composite();
        let Some(position) = index.commit_id_to_pos(commit_id) else {
            return Ok(false);
        };

        let mut inner = self.inner.borrow_mut();
        inner.consume_to(index, position)?;
        let found = inner
            .consumed_positions
            .binary_search_by(|p| p.cmp(&position).reverse())
            .is_ok();
        Ok(found)
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
    fn consume_to(
        &mut self,
        index: &CompositeIndex,
        desired_position: IndexPosition,
    ) -> Result<(), RevsetEvaluationError> {
        let last_position = self.consumed_positions.last();
        if last_position.is_some_and(|&pos| pos <= desired_position) {
            return Ok(());
        }
        while let Some(position) = self.walk.next(index).transpose()? {
            self.consumed_positions.push(position);
            if position <= desired_position {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Adapter for precomputed `IndexPosition`s.
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
        let walk = EagerRevWalk::new(self.positions.clone().into_iter());
        Box::new(walk.map(|_index, pos| Ok(pos)))
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

/// Adapter for infallible `RevWalk` of `IndexPosition`s.
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
        Box::new(self.walk.clone().map(|_index, pos| Ok(pos)))
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
        Ok(walk.next_if(index, |&pos| pos == entry_pos).is_some())
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
        Box::new(self.candidates.positions().filter_map(move |index, pos| {
            pos.and_then(|pos| Ok(p(index, pos)?.then_some(pos)))
                .transpose()
        }))
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
        Box::new(move |index, pos| Ok(p1(index, pos)? && p2(index, pos)?))
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
        Box::new(move |index, pos| Ok(!p(index, pos)?))
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
        Box::new(move |index, pos| Ok(p1(index, pos)? || p2(index, pos)?))
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

impl<I, T, E, W1, W2, C> RevWalk<I> for UnionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I, Item = Result<T, E>>,
    W2: RevWalk<I, Item = Result<T, E>>,
    C: FnMut(&T, &T) -> Ordering,
{
    type Item = W1::Item;

    fn next(&mut self, index: &I) -> Option<Self::Item> {
        match (self.walk1.peek(index), self.walk2.peek(index)) {
            (None, _) => self.walk2.next(index),
            (_, None) => self.walk1.next(index),
            (Some(Ok(item1)), Some(Ok(item2))) => match (self.cmp)(item1, item2) {
                Ordering::Less => self.walk1.next(index),
                Ordering::Equal => {
                    self.walk2.next(index);
                    self.walk1.next(index)
                }
                Ordering::Greater => self.walk2.next(index),
            },
            (Some(Err(_)), _) => self.walk1.next(index),
            (_, Some(Err(_))) => self.walk2.next(index),
        }
    }
}

fn union_by<I, T, E, W1, W2, C>(walk1: W1, walk2: W2, cmp: C) -> UnionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I, Item = Result<T, E>>,
    W2: RevWalk<I, Item = Result<T, E>>,
    C: FnMut(&T, &T) -> Ordering,
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
        Box::new(move |index, pos| Ok(p1(index, pos)? && p2(index, pos)?))
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

impl<I, T, E, W1, W2, C> RevWalk<I> for IntersectionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I, Item = Result<T, E>>,
    W2: RevWalk<I, Item = Result<T, E>>,
    C: FnMut(&T, &T) -> Ordering,
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
                (Some(Ok(item1)), Some(Ok(item2))) => match (self.cmp)(item1, item2) {
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
                (Some(Err(_)), _) => {
                    return self.walk1.next(index);
                }
                (_, Some(Err(_))) => {
                    return self.walk2.next(index);
                }
            }
        }
    }
}

fn intersection_by<I, T, E, W1, W2, C>(
    walk1: W1,
    walk2: W2,
    cmp: C,
) -> IntersectionRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I, Item = Result<T, E>>,
    W2: RevWalk<I, Item = Result<T, E>>,
    C: FnMut(&T, &T) -> Ordering,
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
        Box::new(move |index, pos| Ok(p1(index, pos)? && !p2(index, pos)?))
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

impl<I, T, E, W1, W2, C> RevWalk<I> for DifferenceRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I, Item = Result<T, E>>,
    W2: RevWalk<I, Item = Result<T, E>>,
    C: FnMut(&T, &T) -> Ordering,
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
                (Some(Ok(item1)), Some(Ok(item2))) => match (self.cmp)(item1, item2) {
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
                (Some(Err(_)), _) => {
                    return self.walk1.next(index);
                }
                (_, Some(Err(_))) => {
                    return self.walk2.next(index);
                }
            }
        }
    }
}

fn difference_by<I, T, E, W1, W2, C>(
    walk1: W1,
    walk2: W2,
    cmp: C,
) -> DifferenceRevWalk<I, W1, W2, C>
where
    I: ?Sized,
    W1: RevWalk<I, Item = Result<T, E>>,
    W2: RevWalk<I, Item = Result<T, E>>,
    C: FnMut(&T, &T) -> Ordering,
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
        RevsetEvaluationError::Other(
            format!("Lower bound of generation ({}) is too large", range.start).into(),
        )
    })?;
    let end = range.end.try_into().unwrap_or(u32::MAX);
    Ok(start..end)
}

impl EvaluationContext<'_> {
    fn evaluate(
        &self,
        expression: &ResolvedExpression,
    ) -> Result<Box<dyn InternalRevset>, RevsetEvaluationError> {
        let index = self.index;
        match expression {
            ResolvedExpression::Commits(commit_ids) => {
                Ok(Box::new(self.revset_for_commit_ids(commit_ids)?))
            }
            ResolvedExpression::Ancestors { heads, generation } => {
                let head_set = self.evaluate(heads)?;
                let head_positions = head_set.positions().attach(index);
                let builder =
                    RevWalkBuilder::new(index).wanted_heads(head_positions.try_collect()?);
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
                let root_positions: Vec<_> = root_set.positions().attach(index).try_collect()?;
                // Pre-filter heads so queries like 'immutable_heads()..' can
                // terminate early. immutable_heads() usually includes some
                // visible heads, which can be trivially rejected.
                let head_set = self.evaluate(heads)?;
                let head_positions = difference_by(
                    head_set.positions(),
                    EagerRevWalk::new(root_positions.iter().copied().map(Ok)),
                    |pos1, pos2| pos1.cmp(pos2).reverse(),
                )
                .attach(index);
                let builder = RevWalkBuilder::new(index)
                    .wanted_heads(head_positions.try_collect()?)
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
                let builder =
                    RevWalkBuilder::new(index).wanted_heads(head_positions.try_collect()?);
                if generation_from_roots == &(1..2) {
                    let root_positions: HashSet<_> = root_positions.try_collect()?;
                    let walk = builder
                        .ancestors_until_roots(root_positions.iter().copied())
                        .detach();
                    let candidates = RevWalkRevset { walk };
                    let predicate = as_pure_predicate_fn(move |index, pos| {
                        Ok(index
                            .entry_by_pos(pos)
                            .parent_positions()
                            .iter()
                            .any(|parent_pos| root_positions.contains(parent_pos)))
                    });
                    // TODO: Suppose heads include all visible heads, ToPredicateFn version can be
                    // optimized to only test the predicate()
                    Ok(Box::new(FilterRevset {
                        candidates,
                        predicate,
                    }))
                } else if generation_from_roots == &GENERATION_RANGE_FULL {
                    let mut positions = builder
                        .descendants(root_positions.try_collect()?)
                        .collect_vec();
                    positions.reverse();
                    Ok(Box::new(EagerRevset { positions }))
                } else {
                    // For small generation range, it might be better to build a reachable map
                    // with generation bit set, which can be calculated incrementally from roots:
                    //   reachable[pos] = (reachable[parent_pos] | ...) << 1
                    let mut positions = builder
                        .descendants_filtered_by_generation(
                            root_positions.try_collect()?,
                            to_u32_generation_range(generation_from_roots)?,
                        )
                        .map(|Reverse(pos)| pos)
                        .collect_vec();
                    positions.reverse();
                    Ok(Box::new(EagerRevset { positions }))
                }
            }
            ResolvedExpression::Reachable { sources, domain } => {
                let mut sets = union_find::UnionFind::<IndexPosition>::new();

                // Compute all reachable subgraphs.
                let domain_revset = self.evaluate(domain)?;
                let domain_vec: Vec<_> = domain_revset.positions().attach(index).try_collect()?;
                let domain_set: HashSet<_> = domain_vec.iter().copied().collect();
                for pos in &domain_set {
                    for parent_pos in index.entry_by_pos(*pos).parent_positions() {
                        if domain_set.contains(&parent_pos) {
                            sets.union(*pos, parent_pos);
                        }
                    }
                }

                // Identify disjoint sets reachable from sources.
                let set_reps: HashSet<_> = intersection_by(
                    self.evaluate(sources)?.positions(),
                    EagerRevWalk::new(domain_vec.iter().copied().map(Ok)),
                    |pos1, pos2| pos1.cmp(pos2).reverse(),
                )
                .attach(index)
                .map_ok(|pos| sets.find(pos))
                .try_collect()?;

                let positions = domain_vec
                    .into_iter()
                    .filter(|pos| set_reps.contains(&sets.find(*pos)))
                    .collect_vec();
                Ok(Box::new(EagerRevset { positions }))
            }
            ResolvedExpression::Heads(candidates) => {
                let candidate_set = self.evaluate(candidates)?;
                let head_positions: BTreeSet<_> =
                    index.heads_pos(candidate_set.positions().attach(index).try_collect()?);
                let positions = head_positions.into_iter().rev().collect();
                Ok(Box::new(EagerRevset { positions }))
            }
            ResolvedExpression::Roots(candidates) => {
                let mut positions: Vec<_> = self
                    .evaluate(candidates)?
                    .positions()
                    .attach(index)
                    .try_collect()?;
                let filled = RevWalkBuilder::new(index)
                    .wanted_heads(positions.clone())
                    .descendants(positions.iter().copied().collect())
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
            ResolvedExpression::ForkPoint(expression) => {
                let expression_set = self.evaluate(expression)?;
                let mut expression_positions_iter = expression_set.positions().attach(index);
                let Some(position) = expression_positions_iter.next() else {
                    return Ok(Box::new(EagerRevset::empty()));
                };
                let mut positions = vec![position?];
                for position in expression_positions_iter {
                    positions = index
                        .common_ancestors_pos(&positions, [position?].as_slice())
                        .into_iter()
                        .collect_vec();
                }
                positions.reverse();
                Ok(Box::new(EagerRevset { positions }))
            }
            ResolvedExpression::Latest { candidates, count } => {
                let candidate_set = self.evaluate(candidates)?;
                Ok(Box::new(self.take_latest_revset(&*candidate_set, *count)?))
            }
            ResolvedExpression::Coalesce(expression1, expression2) => {
                let set1 = self.evaluate(expression1)?;
                if set1.positions().attach(index).next().is_some() {
                    Ok(set1)
                } else {
                    self.evaluate(expression2)
                }
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

    fn revset_for_commit_ids(
        &self,
        commit_ids: &[CommitId],
    ) -> Result<EagerRevset, RevsetEvaluationError> {
        let mut positions: Vec<_> = commit_ids
            .iter()
            .map(|id| {
                // Invalid commit IDs should be rejected by the revset frontend,
                // but there are a few edge cases that break the precondition.
                // For example, in jj <= 0.22, the root commit doesn't exist in
                // the root operation.
                self.index.commit_id_to_pos(id).ok_or_else(|| {
                    RevsetEvaluationError::Other(
                        format!(
                            "Commit ID {} not found in index (index or view might be corrupted)",
                            id.hex()
                        )
                        .into(),
                    )
                })
            })
            .try_collect()?;
        positions.sort_unstable_by_key(|&pos| Reverse(pos));
        positions.dedup();
        Ok(EagerRevset { positions })
    }

    fn take_latest_revset(
        &self,
        candidate_set: &dyn InternalRevset,
        count: usize,
    ) -> Result<EagerRevset, RevsetEvaluationError> {
        if count == 0 {
            return Ok(EagerRevset::empty());
        }

        #[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
        struct Item {
            timestamp: MillisSinceEpoch,
            pos: IndexPosition, // tie-breaker
        }

        let make_rev_item = |pos| -> Result<_, RevsetEvaluationError> {
            let entry = self.index.entry_by_pos(pos?);
            let commit = self.store.get_commit(&entry.commit_id())?;
            Ok(Reverse(Item {
                timestamp: commit.committer().timestamp.timestamp,
                pos: entry.position(),
            }))
        };

        // Maintain min-heap containing the latest (greatest) count items. For small
        // count and large candidate set, this is probably cheaper than building vec
        // and applying selection algorithm.
        let mut candidate_iter = candidate_set
            .positions()
            .attach(self.index)
            .map(make_rev_item)
            .fuse();
        let mut latest_items: BinaryHeap<_> = candidate_iter.by_ref().take(count).try_collect()?;
        for item in candidate_iter {
            let item = item?;
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
        Ok(EagerRevset { positions })
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
    F: Fn(&CompositeIndex, IndexPosition) -> Result<bool, RevsetEvaluationError> + Clone,
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
    F: Fn(&CompositeIndex, IndexPosition) -> Result<bool, RevsetEvaluationError> + Clone,
{
    PurePredicateFn(f)
}

fn box_pure_predicate_fn<'a>(
    f: impl Fn(&CompositeIndex, IndexPosition) -> Result<bool, RevsetEvaluationError> + Clone + 'a,
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
                Ok(parent_count_range.contains(&entry.num_parents()))
            })
        }
        RevsetFilterPredicate::Description(pattern) => {
            let pattern = pattern.clone();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                Ok(pattern.matches(commit.description()))
            })
        }
        RevsetFilterPredicate::Author(pattern) => {
            let pattern = pattern.clone();
            // TODO: Make these functions that take a needle to search for accept some
            // syntax for specifying whether it's a regex.
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                Ok(pattern.matches(&commit.author().name)
                    || pattern.matches(&commit.author().email))
            })
        }
        RevsetFilterPredicate::Committer(pattern) => {
            let pattern = pattern.clone();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                Ok(pattern.matches(&commit.committer().name)
                    || pattern.matches(&commit.committer().email))
            })
        }
        RevsetFilterPredicate::AuthorDate(expression) => {
            let expression = *expression;
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                let author_date = &commit.author().timestamp;
                Ok(expression.matches(author_date))
            })
        }
        RevsetFilterPredicate::CommitterDate(expression) => {
            let expression = *expression;
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                let committer_date = &commit.committer().timestamp;
                Ok(expression.matches(committer_date))
            })
        }
        RevsetFilterPredicate::File(expr) => {
            let matcher: Rc<dyn Matcher> = expr.to_matcher().into();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                Ok(has_diff_from_parent(&store, index, &commit, &*matcher)?)
            })
        }
        RevsetFilterPredicate::DiffContains { text, files } => {
            let text_pattern = text.clone();
            let files_matcher: Rc<dyn Matcher> = files.to_matcher().into();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                Ok(matches_diff_from_parent(
                    &store,
                    index,
                    &commit,
                    &text_pattern,
                    &*files_matcher,
                )?)
            })
        }
        RevsetFilterPredicate::HasConflict => box_pure_predicate_fn(move |index, pos| {
            let entry = index.entry_by_pos(pos);
            let commit = store.get_commit(&entry.commit_id())?;
            Ok(commit.has_conflict()?)
        }),
        RevsetFilterPredicate::Extension(ext) => {
            let ext = ext.clone();
            box_pure_predicate_fn(move |index, pos| {
                let entry = index.entry_by_pos(pos);
                let commit = store.get_commit(&entry.commit_id())?;
                Ok(ext.matches_commit(&commit))
            })
        }
    }
}

fn has_diff_from_parent(
    store: &Arc<Store>,
    index: &CompositeIndex,
    commit: &Commit,
    matcher: &dyn Matcher,
) -> BackendResult<bool> {
    let parents: Vec<_> = commit.parents().try_collect()?;
    if let [parent] = parents.as_slice() {
        // Fast path: no need to load the root tree
        let unchanged = commit.tree_id() == parent.tree_id();
        if matcher.visit(RepoPath::root()) == Visit::AllRecursively {
            return Ok(!unchanged);
        } else if unchanged {
            return Ok(false);
        }
    }

    // Conflict resolution is expensive, try that only for matched files.
    let from_tree = rewrite::merge_commit_trees_no_resolve_without_repo(store, &index, &parents)?;
    let to_tree = commit.tree()?;
    // TODO: handle copy tracking
    let mut tree_diff = from_tree.diff_stream(&to_tree, matcher);
    async {
        // TODO: Resolve values concurrently
        while let Some(entry) = tree_diff.next().await {
            let (from_value, to_value) = entry.values?;
            let from_value = resolve_file_values(store, &entry.path, from_value).await?;
            if from_value == to_value {
                continue;
            }
            return Ok(true);
        }
        Ok(false)
    }
    .block_on()
}

fn matches_diff_from_parent(
    store: &Arc<Store>,
    index: &CompositeIndex,
    commit: &Commit,
    text_pattern: &StringPattern,
    files_matcher: &dyn Matcher,
) -> BackendResult<bool> {
    let parents: Vec<_> = commit.parents().try_collect()?;
    // Conflict resolution is expensive, try that only for matched files.
    let from_tree = rewrite::merge_commit_trees_no_resolve_without_repo(store, &index, &parents)?;
    let to_tree = commit.tree()?;
    // TODO: handle copy tracking
    let mut tree_diff = from_tree.diff_stream(&to_tree, files_matcher);
    async {
        // TODO: Resolve values concurrently
        while let Some(entry) = tree_diff.next().await {
            let (left_value, right_value) = entry.values?;
            let left_value = resolve_file_values(store, &entry.path, left_value).await?;
            if left_value == right_value {
                continue;
            }
            // Conflicts are compared in materialized form. Alternatively,
            // conflict pairs can be compared one by one. #4062
            let left_future = materialize_tree_value(store, &entry.path, left_value);
            let right_future = materialize_tree_value(store, &entry.path, right_value);
            let (left_value, right_value) = futures::try_join!(left_future, right_future)?;
            let left_content = to_file_content(&entry.path, left_value)?;
            let right_content = to_file_content(&entry.path, right_value)?;
            // Filter lines prior to comparison. This might produce inferior
            // hunks due to lack of contexts, but is way faster than full diff.
            let left_lines = match_lines(&left_content, text_pattern);
            let right_lines = match_lines(&right_content, text_pattern);
            if left_lines.ne(right_lines) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    .block_on()
}

fn match_lines<'a: 'b, 'b>(
    text: &'a [u8],
    pattern: &'b StringPattern,
) -> impl Iterator<Item = &'a [u8]> + 'b {
    // The pattern is matched line by line so that it can be anchored to line
    // start/end. For example, exact:"" will match blank lines.
    text.split_inclusive(|b| *b == b'\n').filter(|line| {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        // TODO: add .matches_bytes() or .to_bytes_matcher()
        str::from_utf8(line).is_ok_and(|line| pattern.matches(line))
    })
}

fn to_file_content(path: &RepoPath, value: MaterializedTreeValue) -> BackendResult<Vec<u8>> {
    match value {
        MaterializedTreeValue::Absent => Ok(vec![]),
        MaterializedTreeValue::AccessDenied(_) => Ok(vec![]),
        MaterializedTreeValue::File { id, mut reader, .. } => {
            let mut content = vec![];
            reader
                .read_to_end(&mut content)
                .map_err(|err| BackendError::ReadFile {
                    path: path.to_owned(),
                    id: id.clone(),
                    source: err.into(),
                })?;
            Ok(content)
        }
        MaterializedTreeValue::Symlink { id: _, target } => Ok(target.into_bytes()),
        MaterializedTreeValue::GitSubmodule(_) => Ok(vec![]),
        MaterializedTreeValue::FileConflict { contents, .. } => {
            Ok(materialize_merge_result_to_bytes(&contents, ConflictMarkerStyle::default()).into())
        }
        MaterializedTreeValue::OtherConflict { .. } => Ok(vec![]),
        MaterializedTreeValue::Tree(id) => {
            panic!("Unexpected tree with id {id:?} in diff at path {path:?}");
        }
    }
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

    fn try_collect_vec<T, E>(iter: impl IntoIterator<Item = Result<T, E>>) -> Result<Vec<T>, E> {
        iter.into_iter().collect()
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
        assert!(p(index, get_pos(&id_4)).unwrap());
        assert!(p(index, get_pos(&id_3)).unwrap());
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).unwrap());
        // Uninteresting entries can be skipped
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_3)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).unwrap());

        let set = FilterRevset {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: as_pure_predicate_fn(|index, pos| {
                Ok(index.entry_by_pos(pos).commit_id() != id_4)
            }),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index)).unwrap(),
            make_positions(&[&id_2, &id_0])
        );
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_4)).unwrap());
        assert!(!p(index, get_pos(&id_3)).unwrap());
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).unwrap());

        // Intersection by FilterRevset
        let set = FilterRevset {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index)).unwrap(),
            make_positions(&[&id_2])
        );
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_4)).unwrap());
        assert!(!p(index, get_pos(&id_3)).unwrap());
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(!p(index, get_pos(&id_0)).unwrap());

        let set = UnionRevset {
            set1: make_set(&[&id_4, &id_2]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index)).unwrap(),
            make_positions(&[&id_4, &id_3, &id_2, &id_1])
        );
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_4)).unwrap());
        assert!(p(index, get_pos(&id_3)).unwrap());
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).unwrap());
        assert!(!p(index, get_pos(&id_0)).unwrap());

        let set = IntersectionRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index)).unwrap(),
            make_positions(&[&id_2])
        );
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_4)).unwrap());
        assert!(!p(index, get_pos(&id_3)).unwrap());
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(!p(index, get_pos(&id_0)).unwrap());

        let set = DifferenceRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index)).unwrap(),
            make_positions(&[&id_4, &id_0])
        );
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_4)).unwrap());
        assert!(!p(index, get_pos(&id_3)).unwrap());
        assert!(!p(index, get_pos(&id_2)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).unwrap());
    }

    #[test]
    fn test_revset_combinator_error_propagation() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);

        let index = index.as_composite();
        let get_pos = |id: &CommitId| index.commit_id_to_pos(id).unwrap();
        let make_positions = |ids: &[&CommitId]| ids.iter().copied().map(get_pos).collect_vec();
        let make_good_set = |ids: &[&CommitId]| -> Box<dyn InternalRevset> {
            let positions = make_positions(ids);
            Box::new(EagerRevset { positions })
        };
        let make_bad_set = |ids: &[&CommitId], bad_id: &CommitId| -> Box<dyn InternalRevset> {
            let positions = make_positions(ids);
            let bad_id = bad_id.clone();
            Box::new(FilterRevset {
                candidates: EagerRevset { positions },
                predicate: as_pure_predicate_fn(move |index, pos| {
                    if index.entry_by_pos(pos).commit_id() == bad_id {
                        Err(RevsetEvaluationError::Other("bad".into()))
                    } else {
                        Ok(true)
                    }
                }),
            })
        };

        // Error from filter predicate
        let set = make_bad_set(&[&id_2, &id_1, &id_0], &id_1);
        assert_eq!(
            try_collect_vec(set.positions().attach(index).take(1)).unwrap(),
            make_positions(&[&id_2])
        );
        assert!(try_collect_vec(set.positions().attach(index).take(2)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).is_err());

        // Error from filter candidates
        let set = FilterRevset {
            candidates: make_bad_set(&[&id_2, &id_1, &id_0], &id_1),
            predicate: as_pure_predicate_fn(|_, _| Ok(true)),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index).take(1)).unwrap(),
            make_positions(&[&id_2])
        );
        assert!(try_collect_vec(set.positions().attach(index).take(2)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).is_err());

        // Error from left side of union, immediately
        let set = UnionRevset {
            set1: make_bad_set(&[&id_1], &id_1),
            set2: make_good_set(&[&id_2, &id_1]),
        };
        assert!(try_collect_vec(set.positions().attach(index).take(1)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_2)).unwrap()); // works because bad id isn't visited
        assert!(p(index, get_pos(&id_1)).is_err());

        // Error from right side of union, lazily
        let set = UnionRevset {
            set1: make_good_set(&[&id_2, &id_1]),
            set2: make_bad_set(&[&id_1, &id_0], &id_0),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index).take(2)).unwrap(),
            make_positions(&[&id_2, &id_1])
        );
        assert!(try_collect_vec(set.positions().attach(index).take(3)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).is_err());

        // Error from left side of intersection, immediately
        let set = IntersectionRevset {
            set1: make_bad_set(&[&id_1], &id_1),
            set2: make_good_set(&[&id_2, &id_1]),
        };
        assert!(try_collect_vec(set.positions().attach(index).take(1)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).is_err());

        // Error from right side of intersection, lazily
        let set = IntersectionRevset {
            set1: make_good_set(&[&id_2, &id_1, &id_0]),
            set2: make_bad_set(&[&id_1, &id_0], &id_0),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index).take(1)).unwrap(),
            make_positions(&[&id_1])
        );
        assert!(try_collect_vec(set.positions().attach(index).take(2)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).is_err());

        // Error from left side of difference, immediately
        let set = DifferenceRevset {
            set1: make_bad_set(&[&id_1], &id_1),
            set2: make_good_set(&[&id_2, &id_1]),
        };
        assert!(try_collect_vec(set.positions().attach(index).take(1)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(!p(index, get_pos(&id_2)).unwrap());
        assert!(p(index, get_pos(&id_1)).is_err());

        // Error from right side of difference, lazily
        let set = DifferenceRevset {
            set1: make_good_set(&[&id_2, &id_1, &id_0]),
            set2: make_bad_set(&[&id_1, &id_0], &id_0),
        };
        assert_eq!(
            try_collect_vec(set.positions().attach(index).take(1)).unwrap(),
            make_positions(&[&id_2])
        );
        assert!(try_collect_vec(set.positions().attach(index).take(2)).is_err());
        let mut p = set.to_predicate_fn();
        assert!(p(index, get_pos(&id_2)).unwrap());
        assert!(!p(index, get_pos(&id_1)).unwrap());
        assert!(p(index, get_pos(&id_0)).is_err());
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

        assert!(positions_accum.contains(&id_3).unwrap());
        assert_eq!(positions_accum.consumed_len(), 2);

        assert!(positions_accum.contains(&id_0).unwrap());
        assert_eq!(positions_accum.consumed_len(), 5);

        assert!(positions_accum.contains(&id_3).unwrap());
        assert_eq!(positions_accum.consumed_len(), 5);

        // Does not consume positions for unknown commits
        let positions_accum = PositionsAccumulator::new(index, full_set.positions());

        assert!(!positions_accum
            .contains(&CommitId::from_hex("999999"))
            .unwrap());
        assert_eq!(positions_accum.consumed_len(), 0);

        // Does not consume without necessity
        let set = make_set(&[&id_3, &id_2, &id_1]);
        let positions_accum = PositionsAccumulator::new(index, set.positions());

        assert!(!positions_accum.contains(&id_4).unwrap());
        assert_eq!(positions_accum.consumed_len(), 1);

        assert!(positions_accum.contains(&id_3).unwrap());
        assert_eq!(positions_accum.consumed_len(), 1);

        assert!(!positions_accum.contains(&id_0).unwrap());
        assert_eq!(positions_accum.consumed_len(), 3);

        assert!(positions_accum.contains(&id_1).unwrap());
    }
}
