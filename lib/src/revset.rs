// Copyright 2021 Google LLC
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
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use itertools::Itertools;
use pest::iterators::Pairs;
use pest::Parser;
use thiserror::Error;

use crate::backend::{BackendError, BackendResult, CommitId};
use crate::commit::Commit;
use crate::index::{HexPrefix, IndexEntry, IndexPosition, PrefixResolution, RevWalk};
use crate::repo::RepoRef;
use crate::revset_graph_iterator::RevsetGraphIterator;
use crate::store::Store;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetError {
    #[error("Revision \"{0}\" doesn't exist")]
    NoSuchRevision(String),
    #[error("Commit id prefix \"{0}\" is ambiguous")]
    AmbiguousCommitIdPrefix(String),
    #[error("Change id prefix \"{0}\" is ambiguous")]
    AmbiguousChangeIdPrefix(String),
    #[error("Unexpected error from store: {0}")]
    StoreError(#[from] BackendError),
}

fn resolve_git_ref(repo: RepoRef, symbol: &str) -> Result<Vec<CommitId>, RevsetError> {
    let view = repo.view();
    for git_ref_prefix in &["", "refs/", "refs/heads/", "refs/tags/", "refs/remotes/"] {
        if let Some(ref_target) = view.git_refs().get(&(git_ref_prefix.to_string() + symbol)) {
            return Ok(ref_target.adds());
        }
    }
    Err(RevsetError::NoSuchRevision(symbol.to_owned()))
}

fn resolve_branch(repo: RepoRef, symbol: &str) -> Result<Vec<CommitId>, RevsetError> {
    if let Some(branch_target) = repo.view().branches().get(symbol) {
        return Ok(branch_target
            .local_target
            .as_ref()
            .map(|target| target.adds())
            .unwrap_or_default());
    }
    if let Some((name, remote_name)) = symbol.split_once('@') {
        if let Some(branch_target) = repo.view().branches().get(name) {
            if let Some(target) = branch_target.remote_targets.get(remote_name) {
                return Ok(target.adds());
            }
        }
    }
    Err(RevsetError::NoSuchRevision(symbol.to_owned()))
}

fn resolve_commit_id(repo: RepoRef, symbol: &str) -> Result<Vec<CommitId>, RevsetError> {
    // First check if it's a full commit id.
    if let Ok(binary_commit_id) = hex::decode(symbol) {
        let commit_id = CommitId::new(binary_commit_id);
        match repo.store().get_commit(&commit_id) {
            Ok(_) => return Ok(vec![commit_id]),
            Err(BackendError::NotFound) => {} // fall through
            Err(err) => return Err(RevsetError::StoreError(err)),
        }
    }

    if let Some(prefix) = HexPrefix::new(symbol.to_owned()) {
        match repo.index().resolve_prefix(&prefix) {
            PrefixResolution::NoMatch => {
                return Err(RevsetError::NoSuchRevision(symbol.to_owned()))
            }
            PrefixResolution::AmbiguousMatch => {
                return Err(RevsetError::AmbiguousCommitIdPrefix(symbol.to_owned()))
            }
            PrefixResolution::SingleMatch(commit_id) => return Ok(vec![commit_id]),
        }
    }

    Err(RevsetError::NoSuchRevision(symbol.to_owned()))
}

fn resolve_change_id(repo: RepoRef, change_id_prefix: &str) -> Result<Vec<CommitId>, RevsetError> {
    if let Some(hex_prefix) = HexPrefix::new(change_id_prefix.to_owned()) {
        let mut found_change_id = None;
        let mut commit_ids = vec![];
        // TODO: Create a persistent lookup from change id to (visible?) commit ids.
        for index_entry in RevsetExpression::all().evaluate(repo).unwrap().iter() {
            let change_id = index_entry.change_id();
            if change_id.hex().starts_with(hex_prefix.hex()) {
                if let Some(previous_change_id) = found_change_id.replace(change_id.clone()) {
                    if previous_change_id != change_id {
                        return Err(RevsetError::AmbiguousChangeIdPrefix(
                            change_id_prefix.to_owned(),
                        ));
                    }
                }
                commit_ids.push(index_entry.commit_id());
            }
        }
        if found_change_id.is_none() {
            return Err(RevsetError::NoSuchRevision(change_id_prefix.to_owned()));
        }
        Ok(commit_ids)
    } else {
        Err(RevsetError::NoSuchRevision(change_id_prefix.to_owned()))
    }
}

pub fn resolve_symbol(repo: RepoRef, symbol: &str) -> Result<Vec<CommitId>, RevsetError> {
    if symbol == "@" {
        Ok(vec![repo.view().checkout().clone()])
    } else if symbol == "root" {
        Ok(vec![repo.store().root_commit_id().clone()])
    } else {
        // Try to resolve as a tag
        if let Some(target) = repo.view().tags().get(symbol) {
            return Ok(target.adds());
        }

        // Try to resolve as a branch
        let branch_result = resolve_branch(repo, symbol);
        if !matches!(branch_result, Err(RevsetError::NoSuchRevision(_))) {
            return branch_result;
        }

        // Try to resolve as a git ref
        let git_ref_result = resolve_git_ref(repo, symbol);
        if !matches!(git_ref_result, Err(RevsetError::NoSuchRevision(_))) {
            return git_ref_result;
        }

        // Try to resolve as a commit id.
        let commit_id_result = resolve_commit_id(repo, symbol);
        if !matches!(commit_id_result, Err(RevsetError::NoSuchRevision(_))) {
            return commit_id_result;
        }

        // Try to resolve as a change id.
        let change_id_result = resolve_change_id(repo, symbol);
        if !matches!(change_id_result, Err(RevsetError::NoSuchRevision(_))) {
            return change_id_result;
        }

        Err(RevsetError::NoSuchRevision(symbol.to_owned()))
    }
}

#[derive(Parser)]
#[grammar = "revset.pest"]
pub struct RevsetParser;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetParseError {
    #[error("{0}")]
    SyntaxError(#[from] pest::error::Error<Rule>),
    #[error("Revset function \"{0}\" doesn't exist")]
    NoSuchFunction(String),
    #[error("Invalid arguments to revset function \"{name}\": {message}")]
    InvalidFunctionArguments { name: String, message: String },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RevsetExpression {
    None,
    Commits(Vec<CommitId>),
    Symbol(String),
    Parents(Rc<RevsetExpression>),
    Children(Rc<RevsetExpression>),
    Ancestors(Rc<RevsetExpression>),
    // Commits that are ancestors of "heads" but not ancestors of "roots"
    Range {
        roots: Rc<RevsetExpression>,
        heads: Rc<RevsetExpression>,
    },
    // Commits that are descendants of "roots" and ancestors of "heads"
    DagRange {
        roots: Rc<RevsetExpression>,
        heads: Rc<RevsetExpression>,
    },
    Heads,
    HeadsOf(Rc<RevsetExpression>),
    PublicHeads,
    Branches,
    RemoteBranches,
    Tags,
    GitRefs,
    GitHead,
    ParentCount {
        candidates: Rc<RevsetExpression>,
        parent_count_range: Range<u32>,
    },
    Description {
        needle: String,
        candidates: Rc<RevsetExpression>,
    },
    Union(Rc<RevsetExpression>, Rc<RevsetExpression>),
    Intersection(Rc<RevsetExpression>, Rc<RevsetExpression>),
    Difference(Rc<RevsetExpression>, Rc<RevsetExpression>),
}

impl RevsetExpression {
    pub fn none() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::None)
    }

    pub fn all() -> Rc<RevsetExpression> {
        RevsetExpression::heads().ancestors()
    }

    pub fn symbol(value: String) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Symbol(value))
    }

    pub fn commit(commit_id: CommitId) -> Rc<RevsetExpression> {
        RevsetExpression::commits(vec![commit_id])
    }

    pub fn commits(commit_ids: Vec<CommitId>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Commits(commit_ids))
    }

    pub fn heads() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Heads)
    }

    pub fn public_heads() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::PublicHeads)
    }

    pub fn branches() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Branches)
    }

    pub fn remote_branches() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::RemoteBranches)
    }

    pub fn tags() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Tags)
    }

    pub fn git_refs() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::GitRefs)
    }

    pub fn git_head() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::GitHead)
    }

    /// Commits in `self` that don't have descendants in `self`.
    // TODO: Perhaps this should be renamed to just `heads()` and the current
    // `heads()` should become `visible_heads()` or `current_heads()`.
    pub fn heads_of(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::HeadsOf(self.clone()))
    }

    /// Parents of `self`.
    pub fn parents(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Parents(self.clone()))
    }

    /// Ancestors of `self`, including `self`.
    pub fn ancestors(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Ancestors(self.clone()))
    }

    /// Children of `self`.
    pub fn children(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Children(self.clone()))
    }

    /// Descendants of `self`, including `self`.
    pub fn descendants(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        self.dag_range_to(&RevsetExpression::heads())
    }

    /// Commits that are descendants of `self` and ancestors of `heads`, both
    /// inclusive.
    pub fn dag_range_to(
        self: &Rc<RevsetExpression>,
        heads: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::DagRange {
            roots: self.clone(),
            heads: heads.clone(),
        })
    }

    /// Commits reachable from `heads` but not from `self`.
    pub fn range(
        self: &Rc<RevsetExpression>,
        heads: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Range {
            roots: self.clone(),
            heads: heads.clone(),
        })
    }

    /// Commits in `self` with number of parents in the given range.
    pub fn with_parent_count(
        self: &Rc<RevsetExpression>,
        parent_count_range: Range<u32>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::ParentCount {
            candidates: self.clone(),
            parent_count_range,
        })
    }

    /// Commits in `self` with description containing `needle`.
    pub fn with_description(self: &Rc<RevsetExpression>, needle: String) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Description {
            candidates: self.clone(),
            needle,
        })
    }

    /// Commits that are in `self` or in `other` (or both).
    pub fn union(
        self: &Rc<RevsetExpression>,
        other: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Union(self.clone(), other.clone()))
    }

    /// Commits that are in `self` and in `other`.
    pub fn intersection(
        self: &Rc<RevsetExpression>,
        other: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Intersection(self.clone(), other.clone()))
    }

    /// Commits that are in `self` but not in `other`.
    pub fn minus(
        self: &Rc<RevsetExpression>,
        other: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Difference(self.clone(), other.clone()))
    }

    pub fn evaluate<'repo>(
        &self,
        repo: RepoRef<'repo>,
    ) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
        evaluate_expression(repo, self)
    }
}

fn parse_expression_rule(mut pairs: Pairs<Rule>) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::infix_expression => parse_infix_expression_rule(first.into_inner()),
        _ => {
            panic!(
                "unxpected revset parse rule {:?} in: {:?}",
                first.as_rule(),
                first.as_str()
            );
        }
    }
}

fn parse_infix_expression_rule(
    mut pairs: Pairs<Rule>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let mut expression1 = parse_range_expression_rule(pairs.next().unwrap().into_inner())?;
    while let Some(operator) = pairs.next() {
        let expression2 = parse_range_expression_rule(pairs.next().unwrap().into_inner())?;
        expression1 = match operator.as_rule() {
            Rule::union_op => expression1.union(&expression2),
            Rule::intersection_op => expression1.intersection(&expression2),
            Rule::difference_op => expression1.minus(&expression2),
            _ => {
                panic!(
                    "unxpected revset infix operator rule {:?}",
                    operator.as_rule()
                );
            }
        }
    }
    Ok(expression1)
}

fn parse_range_expression_rule(
    mut pairs: Pairs<Rule>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::dag_range_op => {
            return Ok(
                parse_neighbors_expression_rule(pairs.next().unwrap().into_inner())?.ancestors(),
            );
        }
        Rule::neighbors_expression => {
            // Fall through
        }
        _ => {
            panic!("unxpected revset range operator rule {:?}", first.as_rule());
        }
    }
    let mut expression = parse_neighbors_expression_rule(first.into_inner())?;
    if let Some(next) = pairs.next() {
        match next.as_rule() {
            Rule::dag_range_op => {
                if let Some(heads_pair) = pairs.next() {
                    let heads_expression =
                        parse_neighbors_expression_rule(heads_pair.into_inner())?;
                    expression = expression.dag_range_to(&heads_expression);
                } else {
                    expression = expression.descendants();
                }
            }
            Rule::range_op => {
                let expression2 =
                    parse_neighbors_expression_rule(pairs.next().unwrap().into_inner())?;
                expression = expression.range(&expression2);
            }
            _ => {
                panic!("unxpected revset range operator rule {:?}", next.as_rule());
            }
        }
    }
    Ok(expression)
}

fn parse_neighbors_expression_rule(
    mut pairs: Pairs<Rule>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let mut expression = parse_primary_rule(pairs.next().unwrap().into_inner())?;
    for operator in pairs {
        match operator.as_rule() {
            Rule::parents_op => {
                expression = expression.parents();
            }
            Rule::children_op => {
                expression = expression.children();
            }
            _ => {
                panic!(
                    "unxpected revset neighbors operator rule {:?}",
                    operator.as_rule()
                );
            }
        }
    }
    Ok(expression)
}

fn parse_primary_rule(mut pairs: Pairs<Rule>) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::expression => parse_expression_rule(first.into_inner()),
        Rule::function_name => {
            let name = first.as_str().to_owned();
            let argument_pairs = pairs.next().unwrap().into_inner();
            parse_function_expression(name, argument_pairs)
        }
        Rule::symbol => parse_symbol_rule(first.into_inner()),
        _ => {
            panic!("unxpected revset parse rule: {:?}", first.as_str());
        }
    }
}

fn parse_symbol_rule(mut pairs: Pairs<Rule>) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::identifier => Ok(RevsetExpression::symbol(first.as_str().to_owned())),
        Rule::literal_string => {
            return Ok(RevsetExpression::symbol(
                first
                    .as_str()
                    .strip_prefix('"')
                    .unwrap()
                    .strip_suffix('"')
                    .unwrap()
                    .to_owned(),
            ));
        }
        _ => {
            panic!("unxpected symbol parse rule: {:?}", first.as_str());
        }
    }
}

fn parse_function_expression(
    name: String,
    mut argument_pairs: Pairs<Rule>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let arg_count = argument_pairs.clone().count();
    match name.as_str() {
        "parents" => {
            if arg_count == 1 {
                Ok(parse_expression_rule(argument_pairs.next().unwrap().into_inner())?.parents())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "children" => {
            if arg_count == 1 {
                let expression =
                    parse_expression_rule(argument_pairs.next().unwrap().into_inner())?;
                Ok(expression.children())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "ancestors" => {
            if arg_count == 1 {
                Ok(parse_expression_rule(argument_pairs.next().unwrap().into_inner())?.ancestors())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "descendants" => {
            if arg_count == 1 {
                let expression =
                    parse_expression_rule(argument_pairs.next().unwrap().into_inner())?;
                Ok(expression.descendants())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "none" => {
            if arg_count == 0 {
                Ok(RevsetExpression::none())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "all" => {
            if arg_count == 0 {
                Ok(RevsetExpression::all())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "heads" => {
            if arg_count == 0 {
                Ok(RevsetExpression::heads())
            } else if arg_count == 1 {
                let candidates =
                    parse_expression_rule(argument_pairs.next().unwrap().into_inner())?;
                Ok(candidates.heads_of())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 or 1 arguments".to_string(),
                })
            }
        }
        "public_heads" => {
            if arg_count == 0 {
                Ok(RevsetExpression::public_heads())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "branches" => {
            if arg_count == 0 {
                Ok(RevsetExpression::branches())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "remote_branches" => {
            if arg_count == 0 {
                Ok(RevsetExpression::remote_branches())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "tags" => {
            if arg_count == 0 {
                Ok(RevsetExpression::tags())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "git_refs" => {
            if arg_count == 0 {
                Ok(RevsetExpression::git_refs())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "git_head" => {
            if arg_count == 0 {
                Ok(RevsetExpression::git_head())
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "merges" => {
            if arg_count > 1 {
                return Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 or 1 arguments".to_string(),
                });
            }
            let candidates = if arg_count == 0 {
                RevsetExpression::all()
            } else {
                parse_expression_rule(argument_pairs.next().unwrap().into_inner())?
            };
            Ok(candidates.with_parent_count(2..u32::MAX))
        }
        "description" => {
            if !(1..=2).contains(&arg_count) {
                return Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 or 2 arguments".to_string(),
                });
            }
            let needle = parse_function_argument_to_string(
                &name,
                argument_pairs.next().unwrap().into_inner(),
            )?;
            let candidates = if arg_count == 1 {
                RevsetExpression::all()
            } else {
                parse_expression_rule(argument_pairs.next().unwrap().into_inner())?
            };
            Ok(candidates.with_description(needle))
        }
        _ => Err(RevsetParseError::NoSuchFunction(name)),
    }
}

fn parse_function_argument_to_string(
    name: &str,
    pairs: Pairs<Rule>,
) -> Result<String, RevsetParseError> {
    let expression = parse_expression_rule(pairs.clone())?;
    match expression.as_ref() {
        RevsetExpression::Symbol(symbol) => Ok(symbol.clone()),
        _ => Err(RevsetParseError::InvalidFunctionArguments {
            name: name.to_string(),
            message: format!(
                "Expected function argument of type string, found: {}",
                pairs.as_str()
            ),
        }),
    }
}

pub fn parse(revset_str: &str) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::expression, revset_str)?;
    let first = pairs.next().unwrap();
    assert!(pairs.next().is_none());
    if first.as_span().end() != revset_str.len() {
        let pos = pest::Position::new(revset_str, first.as_span().end()).unwrap();
        let err = pest::error::Error::new_from_pos(
            pest::error::ErrorVariant::CustomError {
                message: "Incomplete parse".to_string(),
            },
            pos,
        );
        return Err(RevsetParseError::SyntaxError(err));
    }

    parse_expression_rule(first.into_inner())
}

pub trait Revset<'repo> {
    // All revsets currently iterate in order of descending index position
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo>;
}

pub struct RevsetIterator<'revset, 'repo: 'revset> {
    inner: Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>,
}

impl<'revset, 'repo> RevsetIterator<'revset, 'repo> {
    fn new(inner: Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>) -> Self {
        Self { inner }
    }

    pub fn commit_ids(self) -> RevsetCommitIdIterator<'revset, 'repo> {
        RevsetCommitIdIterator(self.inner)
    }

    pub fn commits(self, store: &Arc<Store>) -> RevsetCommitIterator<'revset, 'repo> {
        RevsetCommitIterator {
            iter: self.inner,
            store: store.clone(),
        }
    }

    pub fn graph(self) -> RevsetGraphIterator<'revset, 'repo> {
        RevsetGraphIterator::new(self)
    }
}

impl<'repo> Iterator for RevsetIterator<'_, 'repo> {
    type Item = IndexEntry<'repo>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

pub struct RevsetCommitIdIterator<'revset, 'repo: 'revset>(
    Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>,
);

impl Iterator for RevsetCommitIdIterator<'_, '_> {
    type Item = CommitId;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|index_entry| index_entry.commit_id())
    }
}

pub struct RevsetCommitIterator<'revset, 'repo: 'revset> {
    store: Arc<Store>,
    iter: Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>,
}

impl Iterator for RevsetCommitIterator<'_, '_> {
    type Item = BackendResult<Commit>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|index_entry| self.store.get_commit(&index_entry.commit_id()))
    }
}

struct EagerRevset<'repo> {
    index_entries: Vec<IndexEntry<'repo>>,
}

impl<'repo> Revset<'repo> for EagerRevset<'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        RevsetIterator::new(Box::new(self.index_entries.iter().cloned()))
    }
}

struct RevWalkRevset<'repo> {
    walk: RevWalk<'repo>,
}

impl<'repo> Revset<'repo> for RevWalkRevset<'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        RevsetIterator::new(Box::new(RevWalkRevsetIterator {
            walk: self.walk.clone(),
        }))
    }
}

struct RevWalkRevsetIterator<'repo> {
    walk: RevWalk<'repo>,
}

impl<'repo> Iterator for RevWalkRevsetIterator<'repo> {
    type Item = IndexEntry<'repo>;

    fn next(&mut self) -> Option<Self::Item> {
        self.walk.next()
    }
}

struct ChildrenRevset<'revset, 'repo: 'revset> {
    // The revisions we want to find children for
    root_set: Box<dyn Revset<'repo> + 'revset>,
    // Consider only candidates from this set
    candidate_set: Box<dyn Revset<'repo> + 'revset>,
}

impl<'repo> Revset<'repo> for ChildrenRevset<'_, 'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        let roots = self
            .root_set
            .iter()
            .map(|parent| parent.position())
            .collect();

        RevsetIterator::new(Box::new(ChildrenRevsetIterator {
            candidate_iter: self.candidate_set.iter(),
            roots,
        }))
    }
}

struct ChildrenRevsetIterator<'revset, 'repo> {
    candidate_iter: RevsetIterator<'revset, 'repo>,
    roots: HashSet<IndexPosition>,
}

impl<'repo> Iterator for ChildrenRevsetIterator<'_, 'repo> {
    type Item = IndexEntry<'repo>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(candidate) = self.candidate_iter.next() {
            if candidate
                .parent_positions()
                .iter()
                .any(|parent_pos| self.roots.contains(parent_pos))
            {
                return Some(candidate);
            }
        }
        None
    }
}

struct FilterRevset<'revset, 'repo: 'revset> {
    candidates: Box<dyn Revset<'repo> + 'revset>,
    predicate: Box<dyn Fn(&IndexEntry<'repo>) -> bool + 'repo>,
}

impl<'repo> Revset<'repo> for FilterRevset<'_, 'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        RevsetIterator::new(Box::new(FilterRevsetIterator {
            iter: self.candidates.iter(),
            predicate: self.predicate.as_ref(),
        }))
    }
}

struct FilterRevsetIterator<'revset, 'repo> {
    iter: RevsetIterator<'revset, 'repo>,
    predicate: &'revset dyn Fn(&IndexEntry<'repo>) -> bool,
}

impl<'revset, 'repo> Iterator for FilterRevsetIterator<'revset, 'repo> {
    type Item = IndexEntry<'repo>;

    fn next(&mut self) -> Option<Self::Item> {
        for entry in &mut self.iter {
            if (self.predicate)(&entry) {
                return Some(entry);
            }
        }
        None
    }
}

struct UnionRevset<'revset, 'repo: 'revset> {
    set1: Box<dyn Revset<'repo> + 'revset>,
    set2: Box<dyn Revset<'repo> + 'revset>,
}

impl<'repo> Revset<'repo> for UnionRevset<'_, 'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        RevsetIterator::new(Box::new(UnionRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        }))
    }
}

struct UnionRevsetIterator<'revset, 'repo> {
    iter1: Peekable<RevsetIterator<'revset, 'repo>>,
    iter2: Peekable<RevsetIterator<'revset, 'repo>>,
}

impl<'revset, 'repo> Iterator for UnionRevsetIterator<'revset, 'repo> {
    type Item = IndexEntry<'repo>;

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

struct IntersectionRevset<'revset, 'repo: 'revset> {
    set1: Box<dyn Revset<'repo> + 'revset>,
    set2: Box<dyn Revset<'repo> + 'revset>,
}

impl<'repo> Revset<'repo> for IntersectionRevset<'_, 'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        RevsetIterator::new(Box::new(IntersectionRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        }))
    }
}

struct IntersectionRevsetIterator<'revset, 'repo> {
    iter1: Peekable<RevsetIterator<'revset, 'repo>>,
    iter2: Peekable<RevsetIterator<'revset, 'repo>>,
}

impl<'revset, 'repo> Iterator for IntersectionRevsetIterator<'revset, 'repo> {
    type Item = IndexEntry<'repo>;

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

struct DifferenceRevset<'revset, 'repo: 'revset> {
    // The minuend (what to subtract from)
    set1: Box<dyn Revset<'repo> + 'revset>,
    // The subtrahend (what to subtract)
    set2: Box<dyn Revset<'repo> + 'revset>,
}

impl<'repo> Revset<'repo> for DifferenceRevset<'_, 'repo> {
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo> {
        RevsetIterator::new(Box::new(DifferenceRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        }))
    }
}

struct DifferenceRevsetIterator<'revset, 'repo> {
    iter1: Peekable<RevsetIterator<'revset, 'repo>>,
    iter2: Peekable<RevsetIterator<'revset, 'repo>>,
}

impl<'revset, 'repo> Iterator for DifferenceRevsetIterator<'revset, 'repo> {
    type Item = IndexEntry<'repo>;

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

pub fn evaluate_expression<'repo>(
    repo: RepoRef<'repo>,
    expression: &RevsetExpression,
) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
    match expression {
        RevsetExpression::None => Ok(Box::new(EagerRevset {
            index_entries: vec![],
        })),
        RevsetExpression::Commits(commit_ids) => Ok(revset_for_commit_ids(repo, commit_ids)),
        RevsetExpression::Symbol(symbol) => {
            let commit_ids = resolve_symbol(repo, symbol)?;
            evaluate_expression(repo, &RevsetExpression::Commits(commit_ids))
        }
        RevsetExpression::Parents(base_expression) => {
            // TODO: Make this lazy
            let base_set = base_expression.evaluate(repo)?;
            let mut parent_entries = base_set
                .iter()
                .flat_map(|entry| entry.parents())
                .collect_vec();
            parent_entries.sort_by_key(|b| Reverse(b.position()));
            parent_entries.dedup();
            Ok(Box::new(EagerRevset {
                index_entries: parent_entries,
            }))
        }
        RevsetExpression::Children(roots) => {
            let root_set = roots.evaluate(repo)?;
            let candidates_expression = roots.descendants();
            let candidate_set = candidates_expression.evaluate(repo)?;
            Ok(Box::new(ChildrenRevset {
                root_set,
                candidate_set,
            }))
        }
        RevsetExpression::Ancestors(base_expression) => RevsetExpression::none()
            .range(base_expression)
            .evaluate(repo),
        RevsetExpression::Range { roots, heads } => {
            let root_set = roots.evaluate(repo)?;
            let root_ids = root_set.iter().commit_ids().collect_vec();
            let head_set = heads.evaluate(repo)?;
            let head_ids = head_set.iter().commit_ids().collect_vec();
            let walk = repo.index().walk_revs(&head_ids, &root_ids);
            Ok(Box::new(RevWalkRevset { walk }))
        }
        // Clippy doesn't seem to understand that we collect the iterator in order to iterate in
        // reverse
        #[allow(clippy::needless_collect)]
        RevsetExpression::DagRange { roots, heads } => {
            let root_set = roots.evaluate(repo)?;
            let candidate_set = heads.ancestors().evaluate(repo)?;
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
            Ok(Box::new(EagerRevset {
                index_entries: result,
            }))
        }
        RevsetExpression::Heads => Ok(revset_for_commit_ids(
            repo,
            &repo.view().heads().iter().cloned().collect_vec(),
        )),
        RevsetExpression::HeadsOf(candidates) => {
            let candidate_set = candidates.evaluate(repo)?;
            let candidate_ids = candidate_set.iter().commit_ids().collect_vec();
            Ok(revset_for_commit_ids(
                repo,
                &repo.index().heads(&candidate_ids),
            ))
        }
        RevsetExpression::ParentCount {
            candidates,
            parent_count_range,
        } => {
            let candidates = candidates.evaluate(repo)?;
            let parent_count_range = parent_count_range.clone();
            Ok(Box::new(FilterRevset {
                candidates,
                predicate: Box::new(move |entry| parent_count_range.contains(&entry.num_parents())),
            }))
        }
        RevsetExpression::PublicHeads => Ok(revset_for_commit_ids(
            repo,
            &repo.view().public_heads().iter().cloned().collect_vec(),
        )),
        RevsetExpression::Branches => {
            let mut commit_ids = vec![];
            for branch_target in repo.view().branches().values() {
                if let Some(local_target) = &branch_target.local_target {
                    commit_ids.extend(local_target.adds());
                }
            }
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::RemoteBranches => {
            let mut commit_ids = vec![];
            for branch_target in repo.view().branches().values() {
                for remote_target in branch_target.remote_targets.values() {
                    commit_ids.extend(remote_target.adds());
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
            let commit_ids = repo.view().git_head().into_iter().collect_vec();
            Ok(revset_for_commit_ids(repo, &commit_ids))
        }
        RevsetExpression::Description { needle, candidates } => {
            let candidates = candidates.evaluate(repo)?;
            let repo = repo;
            let needle = needle.clone();
            Ok(Box::new(FilterRevset {
                candidates,
                predicate: Box::new(move |entry| {
                    repo.store()
                        .get_commit(&entry.commit_id())
                        .unwrap()
                        .description()
                        .contains(needle.as_str())
                }),
            }))
        }
        RevsetExpression::Union(expression1, expression2) => {
            let set1 = expression1.evaluate(repo)?;
            let set2 = expression2.evaluate(repo)?;
            Ok(Box::new(UnionRevset { set1, set2 }))
        }
        RevsetExpression::Intersection(expression1, expression2) => {
            let set1 = expression1.evaluate(repo)?;
            let set2 = expression2.evaluate(repo)?;
            Ok(Box::new(IntersectionRevset { set1, set2 }))
        }
        RevsetExpression::Difference(expression1, expression2) => {
            let set1 = expression1.evaluate(repo)?;
            let set2 = expression2.evaluate(repo)?;
            Ok(Box::new(DifferenceRevset { set1, set2 }))
        }
    }
}

fn revset_for_commit_ids<'revset, 'repo: 'revset>(
    repo: RepoRef<'repo>,
    commit_ids: &[CommitId],
) -> Box<dyn Revset<'repo> + 'revset> {
    let index = repo.index();
    let mut index_entries = vec![];
    for id in commit_ids {
        index_entries.push(index.entry_by_id(id).unwrap());
    }
    index_entries.sort_by_key(|b| Reverse(b.position()));
    index_entries.dedup();
    Box::new(EagerRevset { index_entries })
}

pub fn revset_for_commits<'revset, 'repo: 'revset>(
    repo: RepoRef<'repo>,
    commits: &[&Commit],
) -> Box<dyn Revset<'repo> + 'revset> {
    let index = repo.index();
    let mut index_entries = commits
        .iter()
        .map(|commit| index.entry_by_id(commit.id()).unwrap())
        .collect_vec();
    index_entries.sort_by_key(|b| Reverse(b.position()));
    Box::new(EagerRevset { index_entries })
}

#[cfg(test)]
mod tests {
    use std::assert_matches::assert_matches;

    use super::*;

    #[test]
    fn test_revset_expression_building() {
        let checkout_symbol = RevsetExpression::symbol("@".to_string());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        assert_eq!(
            checkout_symbol,
            Rc::new(RevsetExpression::Symbol("@".to_string()))
        );
        assert_eq!(
            checkout_symbol.heads_of(),
            Rc::new(RevsetExpression::HeadsOf(checkout_symbol.clone()))
        );
        assert_eq!(
            checkout_symbol.parents(),
            Rc::new(RevsetExpression::Parents(checkout_symbol.clone()))
        );
        assert_eq!(
            checkout_symbol.ancestors(),
            Rc::new(RevsetExpression::Ancestors(checkout_symbol.clone()))
        );
        assert_eq!(
            foo_symbol.children(),
            Rc::new(RevsetExpression::Children(foo_symbol.clone()))
        );
        assert_eq!(
            foo_symbol.descendants(),
            Rc::new(RevsetExpression::DagRange {
                roots: foo_symbol.clone(),
                heads: RevsetExpression::heads(),
            })
        );
        assert_eq!(
            foo_symbol.dag_range_to(&checkout_symbol),
            Rc::new(RevsetExpression::DagRange {
                roots: foo_symbol.clone(),
                heads: checkout_symbol.clone(),
            })
        );
        assert_eq!(
            foo_symbol.range(&checkout_symbol),
            Rc::new(RevsetExpression::Range {
                roots: foo_symbol.clone(),
                heads: checkout_symbol.clone()
            })
        );
        assert_eq!(
            foo_symbol.with_parent_count(3..5),
            Rc::new(RevsetExpression::ParentCount {
                candidates: foo_symbol.clone(),
                parent_count_range: 3..5
            })
        );
        assert_eq!(
            foo_symbol.with_description("needle".to_string()),
            Rc::new(RevsetExpression::Description {
                candidates: foo_symbol.clone(),
                needle: "needle".to_string()
            })
        );
        assert_eq!(
            foo_symbol.union(&checkout_symbol),
            Rc::new(RevsetExpression::Union(
                foo_symbol.clone(),
                checkout_symbol.clone()
            ))
        );
        assert_eq!(
            foo_symbol.intersection(&checkout_symbol),
            Rc::new(RevsetExpression::Intersection(
                foo_symbol.clone(),
                checkout_symbol.clone()
            ))
        );
        assert_eq!(
            foo_symbol.minus(&checkout_symbol),
            Rc::new(RevsetExpression::Difference(
                foo_symbol,
                checkout_symbol.clone()
            ))
        );
    }

    #[test]
    fn test_parse_revset() {
        let checkout_symbol = RevsetExpression::symbol("@".to_string());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        let bar_symbol = RevsetExpression::symbol("bar".to_string());
        // Parse a single symbol (specifically the "checkout" symbol)
        assert_eq!(parse("@"), Ok(checkout_symbol.clone()));
        // Parse a single symbol
        assert_eq!(parse("foo"), Ok(foo_symbol.clone()));
        // Parse a parenthesized symbol
        assert_eq!(parse("(foo)"), Ok(foo_symbol.clone()));
        // Parse a quoted symbol
        assert_eq!(parse("\"foo\""), Ok(foo_symbol.clone()));
        // Parse the "parents" operator
        assert_eq!(parse("@-"), Ok(checkout_symbol.parents()));
        // Parse the "children" operator
        assert_eq!(parse("@+"), Ok(checkout_symbol.children()));
        // Parse the "ancestors" operator
        assert_eq!(parse(":@"), Ok(checkout_symbol.ancestors()));
        // Parse the "descendants" operator
        assert_eq!(parse("@:"), Ok(checkout_symbol.descendants()));
        // Parse the "dag range" operator
        assert_eq!(parse("foo:bar"), Ok(foo_symbol.dag_range_to(&bar_symbol)));
        // Parse the "intersection" operator
        assert_eq!(parse("foo & bar"), Ok(foo_symbol.intersection(&bar_symbol)));
        // Parse the "union" operator
        assert_eq!(parse("foo | bar"), Ok(foo_symbol.union(&bar_symbol)));
        // Parse the "difference" operator
        assert_eq!(parse("foo ~ bar"), Ok(foo_symbol.minus(&bar_symbol)));
        // Parentheses are allowed before suffix operators
        assert_eq!(parse("(@)-"), Ok(checkout_symbol.parents()));
        // Space is allowed around expressions
        assert_eq!(parse(" :@ "), Ok(checkout_symbol.ancestors()));
        // Space is not allowed around prefix operators
        assert_matches!(parse(" : @ "), Err(RevsetParseError::SyntaxError(_)));
        // Incomplete parse
        assert_matches!(parse("foo | -"), Err(RevsetParseError::SyntaxError(_)));
        // Space is allowed around infix operators and function arguments
        assert_eq!(
            parse("   description(  arg1 ,   arg2 ) ~    parents(   arg1  )  ~ heads(  )  "),
            Ok(RevsetExpression::symbol("arg2".to_string())
                .with_description("arg1".to_string())
                .minus(&RevsetExpression::symbol("arg1".to_string()).parents())
                .minus(&RevsetExpression::heads()))
        );
    }

    #[test]
    fn test_parse_revset_operator_combinations() {
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        // Parse repeated "parents" operator
        assert_eq!(
            parse("foo---"),
            Ok(foo_symbol.parents().parents().parents())
        );
        // Parse repeated "children" operator
        assert_eq!(
            parse("foo+++"),
            Ok(foo_symbol.children().children().children())
        );
        // Parse repeated "ancestors"/"descendants"/"dag range" operators
        assert_matches!(parse(":foo:"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("::foo"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("foo::"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("foo::bar"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse(":foo:bar"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("foo:bar:"), Err(RevsetParseError::SyntaxError(_)));
        // Parse combinations of "parents"/"children" operators and the range operators.
        // The former bind more strongly.
        assert_eq!(parse("foo-+"), Ok(foo_symbol.parents().children()));
        assert_eq!(parse("foo-:"), Ok(foo_symbol.parents().descendants()));
        assert_eq!(parse(":foo+"), Ok(foo_symbol.children().ancestors()));
    }

    #[test]
    fn test_parse_revset_function() {
        let checkout_symbol = RevsetExpression::symbol("@".to_string());
        assert_eq!(parse("parents(@)"), Ok(checkout_symbol.parents()));
        assert_eq!(parse("parents((@))"), Ok(checkout_symbol.parents()));
        assert_eq!(parse("parents(\"@\")"), Ok(checkout_symbol.parents()));
        assert_eq!(
            parse("ancestors(parents(@))"),
            Ok(checkout_symbol.parents().ancestors())
        );
        assert_matches!(parse("parents(@"), Err(RevsetParseError::SyntaxError(_)));
        assert_eq!(
            parse("parents(@,@)"),
            Err(RevsetParseError::InvalidFunctionArguments {
                name: "parents".to_string(),
                message: "Expected 1 argument".to_string()
            })
        );
        assert_eq!(
            parse("description(foo,bar)"),
            Ok(RevsetExpression::symbol("bar".to_string()).with_description("foo".to_string()))
        );
        assert_eq!(
            parse("description(heads(),bar)"),
            Err(RevsetParseError::InvalidFunctionArguments {
                name: "description".to_string(),
                message: "Expected function argument of type string, found: heads()".to_string()
            })
        );
        assert_eq!(
            parse("description((foo),bar)"),
            Ok(RevsetExpression::symbol("bar".to_string()).with_description("foo".to_string()))
        );
        assert_eq!(
            parse("description(\"(foo)\",bar)"),
            Ok(RevsetExpression::symbol("bar".to_string()).with_description("(foo)".to_string()))
        );
    }
}
