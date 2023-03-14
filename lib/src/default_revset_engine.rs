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

use std::cmp::{Ordering, Reverse};
use std::collections::HashSet;
use std::iter::Peekable;

use itertools::Itertools;

use crate::backend::{BackendError, CommitId, ObjectId};
use crate::commit::Commit;
use crate::default_index_store::IndexEntry;
use crate::default_revset_graph_iterator::RevsetGraphIterator;
use crate::hex_util::to_forward_hex;
use crate::index::{HexPrefix, PrefixResolution};
use crate::matchers::{EverythingMatcher, Matcher, PrefixMatcher};
use crate::op_store::WorkspaceId;
use crate::repo::Repo;
use crate::revset::{
    Revset, RevsetError, RevsetExpression, RevsetFilterPredicate, RevsetGraphEdge,
    RevsetIteratorExt, RevsetWorkspaceContext, GENERATION_RANGE_FULL,
};
use crate::rewrite;

fn resolve_git_ref(repo: &dyn Repo, symbol: &str) -> Option<Vec<CommitId>> {
    let view = repo.view();
    for git_ref_prefix in &["", "refs/", "refs/heads/", "refs/tags/", "refs/remotes/"] {
        if let Some(ref_target) = view.git_refs().get(&(git_ref_prefix.to_string() + symbol)) {
            return Some(ref_target.adds());
        }
    }
    None
}

fn resolve_branch(repo: &dyn Repo, symbol: &str) -> Option<Vec<CommitId>> {
    if let Some(branch_target) = repo.view().branches().get(symbol) {
        return Some(
            branch_target
                .local_target
                .as_ref()
                .map(|target| target.adds())
                .unwrap_or_default(),
        );
    }
    if let Some((name, remote_name)) = symbol.split_once('@') {
        if let Some(branch_target) = repo.view().branches().get(name) {
            if let Some(target) = branch_target.remote_targets.get(remote_name) {
                return Some(target.adds());
            }
        }
    }
    None
}

fn resolve_full_commit_id(
    repo: &dyn Repo,
    symbol: &str,
) -> Result<Option<Vec<CommitId>>, RevsetError> {
    if let Ok(binary_commit_id) = hex::decode(symbol) {
        if repo.store().commit_id_length() != binary_commit_id.len() {
            return Ok(None);
        }
        let commit_id = CommitId::new(binary_commit_id);
        match repo.store().get_commit(&commit_id) {
            // Only recognize a commit if we have indexed it
            Ok(_) if repo.index().entry_by_id(&commit_id).is_some() => Ok(Some(vec![commit_id])),
            Ok(_) | Err(BackendError::ObjectNotFound { .. }) => Ok(None),
            Err(err) => Err(RevsetError::StoreError(err)),
        }
    } else {
        Ok(None)
    }
}

fn resolve_short_commit_id(
    repo: &dyn Repo,
    symbol: &str,
) -> Result<Option<Vec<CommitId>>, RevsetError> {
    if let Some(prefix) = HexPrefix::new(symbol) {
        match repo.index().resolve_prefix(&prefix) {
            PrefixResolution::NoMatch => Ok(None),
            PrefixResolution::AmbiguousMatch => {
                Err(RevsetError::AmbiguousIdPrefix(symbol.to_owned()))
            }
            PrefixResolution::SingleMatch(commit_id) => Ok(Some(vec![commit_id])),
        }
    } else {
        Ok(None)
    }
}

fn resolve_change_id(repo: &dyn Repo, symbol: &str) -> Result<Option<Vec<CommitId>>, RevsetError> {
    if let Some(prefix) = to_forward_hex(symbol).as_deref().and_then(HexPrefix::new) {
        match repo.resolve_change_id_prefix(&prefix) {
            PrefixResolution::NoMatch => Ok(None),
            PrefixResolution::AmbiguousMatch => {
                Err(RevsetError::AmbiguousIdPrefix(symbol.to_owned()))
            }
            PrefixResolution::SingleMatch(entries) => {
                Ok(Some(entries.iter().map(|e| e.commit_id()).collect()))
            }
        }
    } else {
        Ok(None)
    }
}

pub fn resolve_symbol(
    repo: &dyn Repo,
    symbol: &str,
    workspace_id: Option<&WorkspaceId>,
) -> Result<Vec<CommitId>, RevsetError> {
    if symbol.ends_with('@') {
        let target_workspace = if symbol == "@" {
            if let Some(workspace_id) = workspace_id {
                workspace_id.clone()
            } else {
                return Err(RevsetError::NoSuchRevision(symbol.to_owned()));
            }
        } else {
            WorkspaceId::new(symbol.strip_suffix('@').unwrap().to_string())
        };
        if let Some(commit_id) = repo.view().get_wc_commit_id(&target_workspace) {
            Ok(vec![commit_id.clone()])
        } else {
            Err(RevsetError::NoSuchRevision(symbol.to_owned()))
        }
    } else if symbol == "root" {
        Ok(vec![repo.store().root_commit_id().clone()])
    } else {
        // Try to resolve as a tag
        if let Some(target) = repo.view().tags().get(symbol) {
            return Ok(target.adds());
        }

        // Try to resolve as a branch
        if let Some(ids) = resolve_branch(repo, symbol) {
            return Ok(ids);
        }

        // Try to resolve as a git ref
        if let Some(ids) = resolve_git_ref(repo, symbol) {
            return Ok(ids);
        }

        // Try to resolve as a full commit id. We assume a full commit id is unambiguous
        // even if it's shorter than change id.
        if let Some(ids) = resolve_full_commit_id(repo, symbol)? {
            return Ok(ids);
        }

        // Try to resolve as a commit id.
        if let Some(ids) = resolve_short_commit_id(repo, symbol)? {
            return Ok(ids);
        }

        // Try to resolve as a change id.
        if let Some(ids) = resolve_change_id(repo, symbol)? {
            return Ok(ids);
        }

        Err(RevsetError::NoSuchRevision(symbol.to_owned()))
    }
}

trait ToPredicateFn<'index> {
    /// Creates function that tests if the given entry is included in the set.
    ///
    /// The predicate function is evaluated in order of `RevsetIterator`.
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_>;
}

impl<'index, T> ToPredicateFn<'index> for Box<T>
where
    T: ToPredicateFn<'index> + ?Sized,
{
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        <T as ToPredicateFn<'index>>::to_predicate_fn(self)
    }
}

trait InternalRevset<'index>: ToPredicateFn<'index> {
    // All revsets currently iterate in order of descending index position
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_>;
}

struct RevsetImpl<'index> {
    inner: Box<dyn InternalRevset<'index> + 'index>,
}

impl<'index> RevsetImpl<'index> {
    fn new(revset: Box<dyn InternalRevset<'index> + 'index>) -> Self {
        Self { inner: revset }
    }
}

impl<'index> ToPredicateFn<'index> for RevsetImpl<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        self.inner.to_predicate_fn()
    }
}

impl<'index> Revset<'index> for RevsetImpl<'index> {
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        self.inner.iter()
    }

    fn iter_graph(
        &self,
    ) -> Box<dyn Iterator<Item = (IndexEntry<'index>, Vec<RevsetGraphEdge>)> + '_> {
        Box::new(RevsetGraphIterator::new(self))
    }

    fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

struct EagerRevset<'index> {
    index_entries: Vec<IndexEntry<'index>>,
}

impl EagerRevset<'static> {
    pub const fn empty() -> Self {
        EagerRevset {
            index_entries: Vec::new(),
        }
    }
}

impl<'index> InternalRevset<'index> for EagerRevset<'index> {
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        Box::new(self.index_entries.iter().cloned())
    }
}

impl<'index> ToPredicateFn<'index> for EagerRevset<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        predicate_fn_from_iter(self.iter())
    }
}

struct RevWalkRevset<'index, T>
where
    // RevWalkRevset<'index> appears to be needed to assert 'index outlives 'a
    // in to_predicate_fn<'a>(&'a self) -> Box<dyn 'a>.
    T: Iterator<Item = IndexEntry<'index>>,
{
    walk: T,
}

impl<'index, T> InternalRevset<'index> for RevWalkRevset<'index, T>
where
    T: Iterator<Item = IndexEntry<'index>> + Clone,
{
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        Box::new(self.walk.clone())
    }
}

impl<'index, T> ToPredicateFn<'index> for RevWalkRevset<'index, T>
where
    T: Iterator<Item = IndexEntry<'index>> + Clone,
{
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        predicate_fn_from_iter(self.iter())
    }
}

fn predicate_fn_from_iter<'index, 'iter>(
    iter: impl Iterator<Item = IndexEntry<'index>> + 'iter,
) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + 'iter> {
    let mut iter = iter.fuse().peekable();
    Box::new(move |entry| {
        while iter.next_if(|e| e.position() > entry.position()).is_some() {
            continue;
        }
        iter.next_if(|e| e.position() == entry.position()).is_some()
    })
}

struct ChildrenRevset<'index> {
    // The revisions we want to find children for
    root_set: RevsetImpl<'index>,
    // Consider only candidates from this set
    candidate_set: RevsetImpl<'index>,
}

impl<'index> InternalRevset<'index> for ChildrenRevset<'index> {
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        let roots: HashSet<_> = self
            .root_set
            .iter()
            .map(|parent| parent.position())
            .collect();

        Box::new(self.candidate_set.iter().filter(move |candidate| {
            candidate
                .parent_positions()
                .iter()
                .any(|parent_pos| roots.contains(parent_pos))
        }))
    }
}

impl<'index> ToPredicateFn<'index> for ChildrenRevset<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        // TODO: can be optimized if candidate_set contains all heads
        predicate_fn_from_iter(self.iter())
    }
}

struct FilterRevset<'index, P> {
    candidates: RevsetImpl<'index>,
    predicate: P,
}

impl<'index, P> InternalRevset<'index> for FilterRevset<'index, P>
where
    P: ToPredicateFn<'index>,
{
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        let p = self.predicate.to_predicate_fn();
        Box::new(self.candidates.iter().filter(p))
    }
}

impl<'index, P> ToPredicateFn<'index> for FilterRevset<'index, P>
where
    P: ToPredicateFn<'index>,
{
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        // TODO: optimize 'p1' out if candidates = All
        let mut p1 = self.candidates.to_predicate_fn();
        let mut p2 = self.predicate.to_predicate_fn();
        Box::new(move |entry| p1(entry) && p2(entry))
    }
}

struct UnionRevset<'index> {
    set1: RevsetImpl<'index>,
    set2: RevsetImpl<'index>,
}

impl<'index> InternalRevset<'index> for UnionRevset<'index> {
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        Box::new(UnionRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        })
    }
}

impl<'index> ToPredicateFn<'index> for UnionRevset<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        let mut p1 = self.set1.to_predicate_fn();
        let mut p2 = self.set2.to_predicate_fn();
        Box::new(move |entry| p1(entry) || p2(entry))
    }
}

struct UnionRevsetIterator<
    'index,
    I1: Iterator<Item = IndexEntry<'index>>,
    I2: Iterator<Item = IndexEntry<'index>>,
> {
    iter1: Peekable<I1>,
    iter2: Peekable<I2>,
}

impl<'index, I1: Iterator<Item = IndexEntry<'index>>, I2: Iterator<Item = IndexEntry<'index>>>
    Iterator for UnionRevsetIterator<'index, I1, I2>
{
    type Item = IndexEntry<'index>;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.iter1.peek(), self.iter2.peek()) {
            (None, _) => self.iter2.next(),
            (_, None) => self.iter1.next(),
            (Some(entry1), Some(entry2)) => match entry1.position().cmp(&entry2.position()) {
                Ordering::Less => self.iter2.next(),
                Ordering::Equal => {
                    self.iter1.next();
                    self.iter2.next()
                }
                Ordering::Greater => self.iter1.next(),
            },
        }
    }
}

struct IntersectionRevset<'index> {
    set1: RevsetImpl<'index>,
    set2: RevsetImpl<'index>,
}

impl<'index> InternalRevset<'index> for IntersectionRevset<'index> {
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        Box::new(IntersectionRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        })
    }
}

impl<'index> ToPredicateFn<'index> for IntersectionRevset<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        let mut p1 = self.set1.to_predicate_fn();
        let mut p2 = self.set2.to_predicate_fn();
        Box::new(move |entry| p1(entry) && p2(entry))
    }
}

struct IntersectionRevsetIterator<
    'index,
    I1: Iterator<Item = IndexEntry<'index>>,
    I2: Iterator<Item = IndexEntry<'index>>,
> {
    iter1: Peekable<I1>,
    iter2: Peekable<I2>,
}

impl<'index, I1: Iterator<Item = IndexEntry<'index>>, I2: Iterator<Item = IndexEntry<'index>>>
    Iterator for IntersectionRevsetIterator<'index, I1, I2>
{
    type Item = IndexEntry<'index>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.iter1.peek(), self.iter2.peek()) {
                (None, _) => {
                    return None;
                }
                (_, None) => {
                    return None;
                }
                (Some(entry1), Some(entry2)) => match entry1.position().cmp(&entry2.position()) {
                    Ordering::Less => {
                        self.iter2.next();
                    }
                    Ordering::Equal => {
                        self.iter1.next();
                        return self.iter2.next();
                    }
                    Ordering::Greater => {
                        self.iter1.next();
                    }
                },
            }
        }
    }
}

struct DifferenceRevset<'index> {
    // The minuend (what to subtract from)
    set1: RevsetImpl<'index>,
    // The subtrahend (what to subtract)
    set2: RevsetImpl<'index>,
}

impl<'index> InternalRevset<'index> for DifferenceRevset<'index> {
    fn iter(&self) -> Box<dyn Iterator<Item = IndexEntry<'index>> + '_> {
        Box::new(DifferenceRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        })
    }
}

impl<'index> ToPredicateFn<'index> for DifferenceRevset<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        // TODO: optimize 'p1' out for unary negate?
        let mut p1 = self.set1.to_predicate_fn();
        let mut p2 = self.set2.to_predicate_fn();
        Box::new(move |entry| p1(entry) && !p2(entry))
    }
}

struct DifferenceRevsetIterator<
    'index,
    I1: Iterator<Item = IndexEntry<'index>>,
    I2: Iterator<Item = IndexEntry<'index>>,
> {
    iter1: Peekable<I1>,
    iter2: Peekable<I2>,
}

impl<'index, I1: Iterator<Item = IndexEntry<'index>>, I2: Iterator<Item = IndexEntry<'index>>>
    Iterator for DifferenceRevsetIterator<'index, I1, I2>
{
    type Item = IndexEntry<'index>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.iter1.peek(), self.iter2.peek()) {
                (None, _) => {
                    return None;
                }
                (_, None) => {
                    return self.iter1.next();
                }
                (Some(entry1), Some(entry2)) => match entry1.position().cmp(&entry2.position()) {
                    Ordering::Less => {
                        self.iter2.next();
                    }
                    Ordering::Equal => {
                        self.iter2.next();
                        self.iter1.next();
                    }
                    Ordering::Greater => {
                        return self.iter1.next();
                    }
                },
            }
        }
    }
}

pub fn evaluate<'index>(
    repo: &'index dyn Repo,
    expression: &RevsetExpression,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<Box<dyn Revset<'index> + 'index>, RevsetError> {
    let revset_impl = evaluate_impl(repo, expression, workspace_ctx)?;
    Ok(Box::new(revset_impl))
}

fn evaluate_impl<'index>(
    repo: &'index dyn Repo,
    expression: &RevsetExpression,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<RevsetImpl<'index>, RevsetError> {
    match expression {
        RevsetExpression::None => Ok(RevsetImpl::new(Box::new(EagerRevset::empty()))),
        RevsetExpression::All => {
            // Since `all()` does not include hidden commits, some of the logical
            // transformation rules may subtly change the evaluated set. For example,
            // `all() & x` is not `x` if `x` is hidden. This wouldn't matter in practice,
            // but if it does, the heads set could be extended to include the commits
            // (and `remote_branches()`) specified in the revset expression. Alternatively,
            // some optimization rules could be removed, but that means `author(_) & x`
            // would have to test `:heads() & x`.
            evaluate_impl(
                repo,
                &RevsetExpression::visible_heads().ancestors(),
                workspace_ctx,
            )
        }
        RevsetExpression::Commits(commit_ids) => Ok(revset_for_commit_ids(repo, commit_ids)),
        RevsetExpression::Symbol(symbol) => {
            let commit_ids = resolve_symbol(repo, symbol, workspace_ctx.map(|c| c.workspace_id))?;
            evaluate_impl(repo, &RevsetExpression::Commits(commit_ids), workspace_ctx)
        }
        RevsetExpression::Children(roots) => {
            let root_set = evaluate_impl(repo, roots, workspace_ctx)?;
            let candidates_expression = roots.descendants();
            let candidate_set = evaluate_impl(repo, &candidates_expression, workspace_ctx)?;
            Ok(RevsetImpl::new(Box::new(ChildrenRevset {
                root_set,
                candidate_set,
            })))
        }
        RevsetExpression::Ancestors { heads, generation } => {
            let range_expression = RevsetExpression::Range {
                roots: RevsetExpression::none(),
                heads: heads.clone(),
                generation: generation.clone(),
            };
            evaluate_impl(repo, &range_expression, workspace_ctx)
        }
        RevsetExpression::Range {
            roots,
            heads,
            generation,
        } => {
            let root_set = evaluate_impl(repo, roots, workspace_ctx)?;
            let root_ids = root_set.iter().commit_ids().collect_vec();
            let head_set = evaluate_impl(repo, heads, workspace_ctx)?;
            let head_ids = head_set.iter().commit_ids().collect_vec();
            let walk = repo.index().walk_revs(&head_ids, &root_ids);
            if generation == &GENERATION_RANGE_FULL {
                Ok(RevsetImpl::new(Box::new(RevWalkRevset { walk })))
            } else {
                let walk = walk.filter_by_generation(generation.clone());
                Ok(RevsetImpl::new(Box::new(RevWalkRevset { walk })))
            }
        }
        RevsetExpression::DagRange { roots, heads } => {
            let root_set = evaluate_impl(repo, roots, workspace_ctx)?;
            let candidate_set = evaluate_impl(repo, &heads.ancestors(), workspace_ctx)?;
            let mut reachable: HashSet<_> = root_set.iter().map(|entry| entry.position()).collect();
            let mut result = vec![];
            let candidates = candidate_set.iter().collect_vec();
            for candidate in candidates.into_iter().rev() {
                if reachable.contains(&candidate.position())
                    || candidate
                        .parent_positions()
                        .iter()
                        .any(|parent_pos| reachable.contains(parent_pos))
                {
                    reachable.insert(candidate.position());
                    result.push(candidate);
                }
            }
            result.reverse();
            Ok(RevsetImpl::new(Box::new(EagerRevset {
                index_entries: result,
            })))
        }
        RevsetExpression::VisibleHeads => Ok(revset_for_commit_ids(
            repo,
            &repo.view().heads().iter().cloned().collect_vec(),
        )),
        RevsetExpression::Heads(candidates) => {
            let candidate_set = evaluate_impl(repo, candidates, workspace_ctx)?;
            let candidate_ids = candidate_set.iter().commit_ids().collect_vec();
            Ok(revset_for_commit_ids(
                repo,
                &repo.index().heads(&mut candidate_ids.iter()),
            ))
        }
        RevsetExpression::Roots(candidates) => {
            let connected_set = evaluate_impl(repo, &candidates.connected(), workspace_ctx)?;
            let filled: HashSet<_> = connected_set.iter().map(|entry| entry.position()).collect();
            let mut index_entries = vec![];
            let candidate_set = evaluate_impl(repo, candidates, workspace_ctx)?;
            for candidate in candidate_set.iter() {
                if !candidate
                    .parent_positions()
                    .iter()
                    .any(|parent| filled.contains(parent))
                {
                    index_entries.push(candidate);
                }
            }
            Ok(RevsetImpl::new(Box::new(EagerRevset { index_entries })))
        }
        RevsetExpression::PublicHeads => Ok(revset_for_commit_ids(
            repo,
            &repo.view().public_heads().iter().cloned().collect_vec(),
        )),
        RevsetExpression::Branches(needle) => {
            let mut commit_ids = vec![];
            for (branch_name, branch_target) in repo.view().branches() {
                if !branch_name.contains(needle) {
                    continue;
                }
                if let Some(local_target) = &branch_target.local_target {
                    commit_ids.extend(local_target.adds());
                }
            }
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::RemoteBranches {
            branch_needle,
            remote_needle,
        } => {
            let mut commit_ids = vec![];
            for (branch_name, branch_target) in repo.view().branches() {
                if !branch_name.contains(branch_needle) {
                    continue;
                }
                for (remote_name, remote_target) in branch_target.remote_targets.iter() {
                    if remote_name.contains(remote_needle) {
                        commit_ids.extend(remote_target.adds());
                    }
                }
            }
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::Tags => {
            let mut commit_ids = vec![];
            for ref_target in repo.view().tags().values() {
                commit_ids.extend(ref_target.adds());
            }
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::GitRefs => {
            let mut commit_ids = vec![];
            for ref_target in repo.view().git_refs().values() {
                commit_ids.extend(ref_target.adds());
            }
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::GitHead => {
            let mut commit_ids = vec![];
            if let Some(ref_target) = repo.view().git_head() {
                commit_ids.extend(ref_target.adds());
            }
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::Filter(predicate) => Ok(RevsetImpl::new(Box::new(FilterRevset {
            candidates: evaluate_impl(repo, &RevsetExpression::All, workspace_ctx)?,
            predicate: build_predicate_fn(repo, predicate),
        }))),
        RevsetExpression::AsFilter(candidates) => evaluate_impl(repo, candidates, workspace_ctx),
        RevsetExpression::Present(candidates) => {
            match evaluate_impl(repo, candidates, workspace_ctx) {
                Ok(set) => Ok(set),
                Err(RevsetError::NoSuchRevision(_)) => {
                    Ok(RevsetImpl::new(Box::new(EagerRevset::empty())))
                }
                r @ Err(RevsetError::AmbiguousIdPrefix(_) | RevsetError::StoreError(_)) => r,
            }
        }
        RevsetExpression::NotIn(complement) => {
            let set1 = evaluate_impl(repo, &RevsetExpression::All, workspace_ctx)?;
            let set2 = evaluate_impl(repo, complement, workspace_ctx)?;
            Ok(RevsetImpl::new(Box::new(DifferenceRevset { set1, set2 })))
        }
        RevsetExpression::Union(expression1, expression2) => {
            let set1 = evaluate_impl(repo, expression1, workspace_ctx)?;
            let set2 = evaluate_impl(repo, expression2, workspace_ctx)?;
            Ok(RevsetImpl::new(Box::new(UnionRevset { set1, set2 })))
        }
        RevsetExpression::Intersection(expression1, expression2) => {
            match expression2.as_ref() {
                RevsetExpression::Filter(predicate) => {
                    Ok(RevsetImpl::new(Box::new(FilterRevset {
                        candidates: evaluate_impl(repo, expression1, workspace_ctx)?,
                        predicate: build_predicate_fn(repo, predicate),
                    })))
                }
                RevsetExpression::AsFilter(expression2) => {
                    Ok(RevsetImpl::new(Box::new(FilterRevset {
                        candidates: evaluate_impl(repo, expression1, workspace_ctx)?,
                        predicate: evaluate_impl(repo, expression2, workspace_ctx)?,
                    })))
                }
                _ => {
                    // TODO: 'set2' can be turned into a predicate, and use FilterRevset
                    // if a predicate function can terminate the 'set1' iterator early.
                    let set1 = evaluate_impl(repo, expression1, workspace_ctx)?;
                    let set2 = evaluate_impl(repo, expression2, workspace_ctx)?;
                    Ok(RevsetImpl::new(Box::new(IntersectionRevset { set1, set2 })))
                }
            }
        }
        RevsetExpression::Difference(expression1, expression2) => {
            let set1 = evaluate_impl(repo, expression1, workspace_ctx)?;
            let set2 = evaluate_impl(repo, expression2, workspace_ctx)?;
            Ok(RevsetImpl::new(Box::new(DifferenceRevset { set1, set2 })))
        }
    }
}

fn revset_for_commit_ids<'index>(
    repo: &'index dyn Repo,
    commit_ids: &[CommitId],
) -> RevsetImpl<'index> {
    let index = repo.index();
    let mut index_entries = vec![];
    for id in commit_ids {
        index_entries.push(index.entry_by_id(id).unwrap());
    }
    index_entries.sort_by_key(|b| Reverse(b.position()));
    index_entries.dedup();
    RevsetImpl::new(Box::new(EagerRevset { index_entries }))
}

pub fn revset_for_commits<'index>(
    repo: &'index dyn Repo,
    commits: &[&Commit],
) -> Box<dyn Revset<'index> + 'index> {
    let index = repo.index();
    let mut index_entries = commits
        .iter()
        .map(|commit| index.entry_by_id(commit.id()).unwrap())
        .collect_vec();
    index_entries.sort_by_key(|b| Reverse(b.position()));
    Box::new(RevsetImpl::new(Box::new(EagerRevset { index_entries })))
}

type PurePredicateFn<'index> = Box<dyn Fn(&IndexEntry<'index>) -> bool + 'index>;

impl<'index> ToPredicateFn<'index> for PurePredicateFn<'index> {
    fn to_predicate_fn(&self) -> Box<dyn FnMut(&IndexEntry<'index>) -> bool + '_> {
        Box::new(self)
    }
}

fn build_predicate_fn<'index>(
    repo: &'index dyn Repo,
    predicate: &RevsetFilterPredicate,
) -> PurePredicateFn<'index> {
    match predicate {
        RevsetFilterPredicate::ParentCount(parent_count_range) => {
            let parent_count_range = parent_count_range.clone();
            Box::new(move |entry| parent_count_range.contains(&entry.num_parents()))
        }
        RevsetFilterPredicate::Description(needle) => {
            let needle = needle.clone();
            Box::new(move |entry| {
                repo.store()
                    .get_commit(&entry.commit_id())
                    .unwrap()
                    .description()
                    .contains(needle.as_str())
            })
        }
        RevsetFilterPredicate::Author(needle) => {
            let needle = needle.clone();
            // TODO: Make these functions that take a needle to search for accept some
            // syntax for specifying whether it's a regex and whether it's
            // case-sensitive.
            Box::new(move |entry| {
                let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
                commit.author().name.contains(needle.as_str())
                    || commit.author().email.contains(needle.as_str())
            })
        }
        RevsetFilterPredicate::Committer(needle) => {
            let needle = needle.clone();
            Box::new(move |entry| {
                let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
                commit.committer().name.contains(needle.as_str())
                    || commit.committer().email.contains(needle.as_str())
            })
        }
        RevsetFilterPredicate::File(paths) => {
            // TODO: Add support for globs and other formats
            let matcher: Box<dyn Matcher> = if let Some(paths) = paths {
                Box::new(PrefixMatcher::new(paths))
            } else {
                Box::new(EverythingMatcher)
            };
            Box::new(move |entry| has_diff_from_parent(repo, entry, matcher.as_ref()))
        }
    }
}

fn has_diff_from_parent(repo: &dyn Repo, entry: &IndexEntry<'_>, matcher: &dyn Matcher) -> bool {
    let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
    let parents = commit.parents();
    let from_tree = rewrite::merge_commit_trees(repo, &parents);
    let to_tree = commit.tree();
    from_tree.diff(&to_tree, matcher).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ChangeId, CommitId};
    use crate::default_index_store::MutableIndexImpl;
    use crate::index::Index;

    /// Generator of unique 16-byte ChangeId excluding root id
    fn change_id_generator() -> impl FnMut() -> ChangeId {
        let mut iter = (1_u128..).map(|n| ChangeId::new(n.to_le_bytes().into()));
        move || iter.next().unwrap()
    }

    #[test]
    fn test_revset_combinator() {
        let mut new_change_id = change_id_generator();
        let mut index = MutableIndexImpl::full(3, 16);
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

        let get_entry = |id: &CommitId| index.entry_by_id(id).unwrap();
        let make_entries = |ids: &[&CommitId]| ids.iter().map(|id| get_entry(id)).collect_vec();
        let make_set = |ids: &[&CommitId]| -> RevsetImpl {
            let index_entries = make_entries(ids);
            RevsetImpl::new(Box::new(EagerRevset { index_entries }))
        };

        let set = make_set(&[&id_4, &id_3, &id_2, &id_0]);
        let mut p = set.to_predicate_fn();
        assert!(p(&get_entry(&id_4)));
        assert!(p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));
        // Uninteresting entries can be skipped
        let mut p = set.to_predicate_fn();
        assert!(p(&get_entry(&id_3)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));

        let set = FilterRevset::<PurePredicateFn> {
            candidates: make_set(&[&id_4, &id_2, &id_0]),
            predicate: Box::new(|entry| entry.commit_id() != id_4),
        };
        assert_eq!(set.iter().collect_vec(), make_entries(&[&id_2, &id_0]));
        let mut p = set.to_predicate_fn();
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
        assert_eq!(set.iter().collect_vec(), make_entries(&[&id_2]));
        let mut p = set.to_predicate_fn();
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
            set.iter().collect_vec(),
            make_entries(&[&id_4, &id_3, &id_2, &id_1])
        );
        let mut p = set.to_predicate_fn();
        assert!(p(&get_entry(&id_4)));
        assert!(p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(p(&get_entry(&id_1)));
        assert!(!p(&get_entry(&id_0)));

        let set = IntersectionRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(set.iter().collect_vec(), make_entries(&[&id_2]));
        let mut p = set.to_predicate_fn();
        assert!(!p(&get_entry(&id_4)));
        assert!(!p(&get_entry(&id_3)));
        assert!(p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(!p(&get_entry(&id_0)));

        let set = DifferenceRevset {
            set1: make_set(&[&id_4, &id_2, &id_0]),
            set2: make_set(&[&id_3, &id_2, &id_1]),
        };
        assert_eq!(set.iter().collect_vec(), make_entries(&[&id_4, &id_0]));
        let mut p = set.to_predicate_fn();
        assert!(p(&get_entry(&id_4)));
        assert!(!p(&get_entry(&id_3)));
        assert!(!p(&get_entry(&id_2)));
        assert!(!p(&get_entry(&id_1)));
        assert!(p(&get_entry(&id_0)));
    }
}
