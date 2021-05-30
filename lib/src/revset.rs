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

use pest::iterators::Pairs;
use pest::Parser;
use thiserror::Error;

use crate::commit::Commit;
use crate::index::{HexPrefix, IndexEntry, IndexPosition, PrefixResolution, RevWalk};
use crate::repo::RepoRef;
use crate::revset_graph_iterator::RevsetGraphIterator;
use crate::store::{CommitId, StoreError};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetError {
    #[error("Revision \"{0}\" doesn't exist")]
    NoSuchRevision(String),
    #[error("Commit id prefix \"{0}\" is ambiguous")]
    AmbiguousCommitIdPrefix(String),
    #[error("Change id prefix \"{0}\" is ambiguous")]
    AmbiguousChangeIdPrefix(String),
    #[error("Unexpected error from store: {0}")]
    StoreError(#[from] StoreError),
}

fn resolve_git_ref(repo: RepoRef, symbol: &str) -> Result<Vec<CommitId>, RevsetError> {
    let view = repo.view();
    for git_ref_prefix in &["", "refs/", "refs/heads/", "refs/tags/", "refs/remotes/"] {
        if let Some(commit_id) = view.git_refs().get(&(git_ref_prefix.to_string() + symbol)) {
            return Ok(vec![commit_id.clone()]);
        }
    }
    Err(RevsetError::NoSuchRevision(symbol.to_owned()))
}

fn resolve_commit_id(repo: RepoRef, symbol: &str) -> Result<Vec<CommitId>, RevsetError> {
    // First check if it's a full commit id.
    if let Ok(binary_commit_id) = hex::decode(symbol) {
        let commit_id = CommitId(binary_commit_id);
        match repo.store().get_commit(&commit_id) {
            Ok(_) => return Ok(vec![commit_id]),
            Err(StoreError::NotFound) => {} // fall through
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

fn resolve_non_obsolete_change_id(
    repo: RepoRef,
    change_id_prefix: &str,
) -> Result<Vec<CommitId>, RevsetError> {
    if let Some(hex_prefix) = HexPrefix::new(change_id_prefix.to_owned()) {
        let evolution = repo.evolution();
        match evolution.resolve_change_id_prefix(&hex_prefix) {
            PrefixResolution::NoMatch => {
                Err(RevsetError::NoSuchRevision(change_id_prefix.to_owned()))
            }
            PrefixResolution::AmbiguousMatch => Err(RevsetError::AmbiguousChangeIdPrefix(
                change_id_prefix.to_owned(),
            )),
            PrefixResolution::SingleMatch(change_id) => {
                Ok(evolution.non_obsoletes(&change_id).into_iter().collect())
            }
        }
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

        // Try to resolve as a change id (the non-obsolete commits in the change).
        let change_id_result = resolve_non_obsolete_change_id(repo, symbol);
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

#[derive(Debug, PartialEq, Eq)]
pub enum RevsetExpression {
    None,
    Symbol(String),
    Parents(Rc<RevsetExpression>),
    Children {
        roots: Rc<RevsetExpression>,
        heads: Rc<RevsetExpression>,
    },
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
    AllHeads,
    PublicHeads,
    GitRefs,
    NonObsoleteHeads(Rc<RevsetExpression>),
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
    fn non_obsolete_heads() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::NonObsoleteHeads(Rc::new(
            RevsetExpression::AllHeads,
        )))
    }

    fn non_obsolete_commits() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Ancestors(
            RevsetExpression::non_obsolete_heads(),
        ))
    }

    pub fn evaluate<'repo>(
        &self,
        repo: RepoRef<'repo>,
    ) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
        evaluate_expression(repo, self)
    }
}

fn parse_expression_rule(mut pairs: Pairs<Rule>) -> Result<RevsetExpression, RevsetParseError> {
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
) -> Result<RevsetExpression, RevsetParseError> {
    let mut expression1 = parse_range_expression_rule(pairs.next().unwrap().into_inner())?;
    while let Some(operator) = pairs.next() {
        let expression2 = parse_range_expression_rule(pairs.next().unwrap().into_inner())?;
        match operator.as_rule() {
            Rule::union_op => {
                expression1 = RevsetExpression::Union(Rc::new(expression1), Rc::new(expression2))
            }
            Rule::intersection_op => {
                expression1 =
                    RevsetExpression::Intersection(Rc::new(expression1), Rc::new(expression2))
            }
            Rule::difference_op => {
                expression1 =
                    RevsetExpression::Difference(Rc::new(expression1), Rc::new(expression2))
            }
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
) -> Result<RevsetExpression, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::ancestors_op => {
            return Ok(RevsetExpression::Ancestors(Rc::new(
                parse_children_expression_rule(pairs.next().unwrap().into_inner())?,
            )));
        }
        Rule::children_expression => {
            // Fall through
        }
        _ => {
            panic!("unxpected revset range operator rule {:?}", first.as_rule());
        }
    }
    let mut expression = parse_children_expression_rule(first.into_inner())?;
    if let Some(next) = pairs.next() {
        match next.as_rule() {
            Rule::descendants_op => {
                expression = RevsetExpression::DagRange {
                    roots: Rc::new(expression),
                    heads: RevsetExpression::non_obsolete_heads(),
                };
            }
            Rule::dag_range_op => {
                let heads_expression =
                    parse_children_expression_rule(pairs.next().unwrap().into_inner())?;
                expression = RevsetExpression::DagRange {
                    roots: Rc::new(expression),
                    heads: Rc::new(heads_expression),
                };
            }
            Rule::range_op => {
                let expression2 =
                    parse_children_expression_rule(pairs.next().unwrap().into_inner())?;
                expression = RevsetExpression::Range {
                    roots: Rc::new(expression),
                    heads: Rc::new(expression2),
                };
            }
            _ => {
                panic!("unxpected revset range operator rule {:?}", next.as_rule());
            }
        }
    }
    Ok(expression)
}

fn parse_children_expression_rule(
    mut pairs: Pairs<Rule>,
) -> Result<RevsetExpression, RevsetParseError> {
    let mut expression = parse_parents_expression_rule(pairs.next().unwrap().into_inner())?;
    for operator in pairs {
        match operator.as_rule() {
            Rule::children_op => {
                expression = RevsetExpression::Children {
                    roots: Rc::new(expression),
                    heads: RevsetExpression::non_obsolete_heads(),
                };
            }
            _ => {
                panic!(
                    "unxpected revset children operator rule {:?}",
                    operator.as_rule()
                );
            }
        }
    }
    Ok(expression)
}

fn parse_parents_expression_rule(
    mut pairs: Pairs<Rule>,
) -> Result<RevsetExpression, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::primary => parse_primary_rule(first.into_inner()),
        Rule::parents_op => Ok(RevsetExpression::Parents(Rc::new(
            parse_parents_expression_rule(pairs)?,
        ))),
        _ => {
            panic!(
                "unxpected revset parents operator rule {:?}",
                first.as_rule()
            );
        }
    }
}

fn parse_primary_rule(mut pairs: Pairs<Rule>) -> Result<RevsetExpression, RevsetParseError> {
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

fn parse_symbol_rule(mut pairs: Pairs<Rule>) -> Result<RevsetExpression, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::identifier => Ok(RevsetExpression::Symbol(first.as_str().to_owned())),
        Rule::literal_string => {
            return Ok(RevsetExpression::Symbol(
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
) -> Result<RevsetExpression, RevsetParseError> {
    let arg_count = argument_pairs.clone().count();
    match name.as_str() {
        "parents" => {
            if arg_count == 1 {
                Ok(RevsetExpression::Parents(Rc::new(parse_expression_rule(
                    argument_pairs.next().unwrap().into_inner(),
                )?)))
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
                Ok(RevsetExpression::Children {
                    roots: Rc::new(expression),
                    heads: RevsetExpression::non_obsolete_heads(),
                })
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "ancestors" => {
            if arg_count == 1 {
                Ok(RevsetExpression::Ancestors(Rc::new(parse_expression_rule(
                    argument_pairs.next().unwrap().into_inner(),
                )?)))
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
                Ok(RevsetExpression::DagRange {
                    roots: Rc::new(expression),
                    heads: RevsetExpression::non_obsolete_heads(),
                })
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "all_heads" => {
            if arg_count == 0 {
                Ok(RevsetExpression::AllHeads)
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "non_obsolete_heads" => {
            if arg_count == 0 {
                Ok(RevsetExpression::NonObsoleteHeads(Rc::new(
                    RevsetExpression::AllHeads,
                )))
            } else if arg_count == 1 {
                Ok(RevsetExpression::NonObsoleteHeads(Rc::new(
                    parse_expression_rule(argument_pairs.next().unwrap().into_inner())?,
                )))
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 or 1 argument".to_string(),
                })
            }
        }
        "public_heads" => {
            if arg_count == 0 {
                Ok(RevsetExpression::PublicHeads)
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 0 arguments".to_string(),
                })
            }
        }
        "git_refs" => {
            if arg_count == 0 {
                Ok(RevsetExpression::GitRefs)
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
                RevsetExpression::non_obsolete_commits()
            } else {
                Rc::new(parse_expression_rule(
                    argument_pairs.next().unwrap().into_inner(),
                )?)
            };
            Ok(RevsetExpression::ParentCount {
                candidates,
                parent_count_range: 2..u32::MAX,
            })
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
                RevsetExpression::non_obsolete_commits()
            } else {
                Rc::new(parse_expression_rule(
                    argument_pairs.next().unwrap().into_inner(),
                )?)
            };
            Ok(RevsetExpression::Description { needle, candidates })
        }
        _ => Err(RevsetParseError::NoSuchFunction(name)),
    }
}

fn parse_function_argument_to_string(
    name: &str,
    pairs: Pairs<Rule>,
) -> Result<String, RevsetParseError> {
    let expression = parse_expression_rule(pairs.clone())?;
    match expression {
        RevsetExpression::Symbol(symbol) => Ok(symbol),
        _ => Err(RevsetParseError::InvalidFunctionArguments {
            name: name.to_string(),
            message: format!(
                "Expected function argument of type string, found: {}",
                pairs.as_str()
            ),
        }),
    }
}

pub fn parse(revset_str: &str) -> Result<RevsetExpression, RevsetParseError> {
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
        while let Some(next) = self.iter.next() {
            if (self.predicate)(&next) {
                return Some(next);
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
        RevsetExpression::Symbol(symbol) => {
            let commit_ids = resolve_symbol(repo, &symbol)?;
            let index = repo.index();
            let mut index_entries: Vec<_> = commit_ids
                .iter()
                .map(|id| index.entry_by_id(id).unwrap())
                .collect();
            index_entries.sort_by_key(|b| Reverse(b.position()));
            Ok(Box::new(EagerRevset { index_entries }))
        }
        RevsetExpression::Parents(base_expression) => {
            // TODO: Make this lazy
            let base_set = base_expression.evaluate(repo)?;
            let mut parent_entries: Vec<_> =
                base_set.iter().flat_map(|entry| entry.parents()).collect();
            parent_entries.sort_by_key(|b| Reverse(b.position()));
            parent_entries.dedup();
            Ok(Box::new(EagerRevset {
                index_entries: parent_entries,
            }))
        }
        RevsetExpression::Children { roots, heads } => {
            let root_set = roots.evaluate(repo)?;
            let candidates_expression = RevsetExpression::Ancestors(heads.clone());
            let candidate_set = candidates_expression.evaluate(repo)?;
            Ok(Box::new(ChildrenRevset {
                root_set,
                candidate_set,
            }))
        }
        RevsetExpression::Ancestors(base_expression) => RevsetExpression::Range {
            roots: Rc::new(RevsetExpression::None),
            heads: base_expression.clone(),
        }
        .evaluate(repo),
        RevsetExpression::Range { roots, heads } => {
            let root_set = roots.evaluate(repo)?;
            let root_ids: Vec<_> = root_set.iter().map(|entry| entry.commit_id()).collect();
            let head_set = heads.evaluate(repo)?;
            let head_ids: Vec<_> = head_set.iter().map(|entry| entry.commit_id()).collect();
            let walk = repo.index().walk_revs(&head_ids, &root_ids);
            Ok(Box::new(RevWalkRevset { walk }))
        }
        // Clippy doesn't seem to understand that we collect the iterator in order to iterate in
        // reverse
        #[allow(clippy::needless_collect)]
        RevsetExpression::DagRange { roots, heads } => {
            let root_set = roots.evaluate(repo)?;
            let candidate_set = RevsetExpression::Ancestors(heads.clone()).evaluate(repo)?;
            let mut reachable: HashSet<_> = root_set.iter().map(|entry| entry.position()).collect();
            let mut result = vec![];
            let candidates: Vec<_> = candidate_set.iter().collect();
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
        RevsetExpression::AllHeads => {
            let index = repo.index();
            let heads = repo.view().heads();
            let mut index_entries: Vec<_> = heads
                .iter()
                .map(|id| index.entry_by_id(id).unwrap())
                .collect();
            index_entries.sort_by_key(|b| Reverse(b.position()));
            Ok(Box::new(EagerRevset { index_entries }))
        }
        RevsetExpression::NonObsoleteHeads(base_expression) => {
            let base_set = base_expression.evaluate(repo)?;
            Ok(non_obsolete_heads(repo, base_set))
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
        RevsetExpression::PublicHeads => {
            let index = repo.index();
            let heads = repo.view().public_heads();
            let mut index_entries: Vec<_> = heads
                .iter()
                .map(|id| index.entry_by_id(id).unwrap())
                .collect();
            index_entries.sort_by_key(|b| Reverse(b.position()));
            Ok(Box::new(EagerRevset { index_entries }))
        }
        RevsetExpression::GitRefs => {
            let index = repo.index();
            let mut index_entries: Vec<_> = repo
                .view()
                .git_refs()
                .values()
                .map(|id| index.entry_by_id(id).unwrap())
                .collect();
            index_entries.sort_by_key(|b| Reverse(b.position()));
            index_entries.dedup();
            Ok(Box::new(EagerRevset { index_entries }))
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

fn non_obsolete_heads<'revset, 'repo: 'revset>(
    repo: RepoRef<'repo>,
    heads: Box<dyn Revset<'repo> + 'repo>,
) -> Box<dyn Revset<'repo> + 'revset> {
    let mut commit_ids = HashSet::new();
    let mut work: Vec<_> = heads.iter().collect();
    let evolution = repo.evolution();
    while !work.is_empty() {
        let index_entry = work.pop().unwrap();
        let commit_id = index_entry.commit_id();
        if commit_ids.contains(&commit_id) {
            continue;
        }
        if !index_entry.is_pruned() && !evolution.is_obsolete(&commit_id) {
            commit_ids.insert(commit_id);
        } else {
            for parent_entry in index_entry.parents() {
                work.push(parent_entry);
            }
        }
    }
    let index = repo.index();
    let commit_ids = index.heads(&commit_ids);
    let mut index_entries: Vec<_> = commit_ids
        .iter()
        .map(|id| index.entry_by_id(id).unwrap())
        .collect();
    index_entries.sort_by_key(|b| Reverse(b.position()));
    Box::new(EagerRevset { index_entries })
}

pub fn revset_for_commits<'revset, 'repo: 'revset>(
    repo: RepoRef<'repo>,
    commits: &[&Commit],
) -> Box<dyn Revset<'repo> + 'revset> {
    let index = repo.index();
    let mut index_entries: Vec<_> = commits
        .iter()
        .map(|commit| index.entry_by_id(commit.id()).unwrap())
        .collect();
    index_entries.sort_by_key(|b| Reverse(b.position()));
    Box::new(EagerRevset { index_entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_revset() {
        // Parse a single symbol (specifically the "checkout" symbol)
        assert_eq!(parse("@"), Ok(RevsetExpression::Symbol("@".to_string())));
        // Parse a single symbol
        assert_eq!(
            parse("foo"),
            Ok(RevsetExpression::Symbol("foo".to_string()))
        );
        // Parse a parenthesized symbol
        assert_eq!(
            parse("(foo)"),
            Ok(RevsetExpression::Symbol("foo".to_string()))
        );
        // Parse a quoted symbol
        assert_eq!(
            parse("\"foo\""),
            Ok(RevsetExpression::Symbol("foo".to_string()))
        );
        // Parse the "parents" operator
        assert_eq!(
            parse(":@"),
            Ok(RevsetExpression::Parents(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        // Parse the "children" operator
        assert_eq!(
            parse("@:"),
            Ok(RevsetExpression::Children {
                roots: Rc::new(RevsetExpression::Symbol("@".to_string())),
                heads: RevsetExpression::non_obsolete_heads(),
            })
        );
        // Parse the "ancestors" operator
        assert_eq!(
            parse(",,@"),
            Ok(RevsetExpression::Ancestors(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        // Parse the "descendants" operator
        assert_eq!(
            parse("@,,"),
            Ok(RevsetExpression::DagRange {
                roots: Rc::new(RevsetExpression::Symbol("@".to_string())),
                heads: RevsetExpression::non_obsolete_heads(),
            })
        );
        // Parse the "dag range" operator
        assert_eq!(
            parse("foo,,bar"),
            Ok(RevsetExpression::DagRange {
                roots: Rc::new(RevsetExpression::Symbol("foo".to_string())),
                heads: Rc::new(RevsetExpression::Symbol("bar".to_string())),
            })
        );
        // Parse the "intersection" operator
        assert_eq!(
            parse("foo & bar"),
            Ok(RevsetExpression::Intersection(
                Rc::new(RevsetExpression::Symbol("foo".to_string())),
                Rc::new(RevsetExpression::Symbol("bar".to_string()))
            ))
        );
        // Parse the "union" operator
        assert_eq!(
            parse("foo | bar"),
            Ok(RevsetExpression::Union(
                Rc::new(RevsetExpression::Symbol("foo".to_string())),
                Rc::new(RevsetExpression::Symbol("bar".to_string()))
            ))
        );
        // Parse the "difference" operator
        assert_eq!(
            parse("foo - bar"),
            Ok(RevsetExpression::Difference(
                Rc::new(RevsetExpression::Symbol("foo".to_string())),
                Rc::new(RevsetExpression::Symbol("bar".to_string()))
            ))
        );
        // Parentheses are allowed after prefix operators
        assert_eq!(
            parse(":(@)"),
            Ok(RevsetExpression::Parents(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        // Space is allowed around expressions
        assert_eq!(
            parse(" ,,@ "),
            Ok(RevsetExpression::Ancestors(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        // Space is not allowed around prefix operators
        assert_matches!(parse(" ,, @ "), Err(RevsetParseError::SyntaxError(_)));
        // Incomplete parse
        assert_matches!(parse("foo | :"), Err(RevsetParseError::SyntaxError(_)));
        // Space is allowed around infix operators and function arguments
        assert_eq!(
            parse("   description(  arg1 ,   arg2 ) -    parents(   arg1  )  - all_heads(  )  "),
            Ok(RevsetExpression::Difference(
                Rc::new(RevsetExpression::Difference(
                    Rc::new(RevsetExpression::Description {
                        needle: "arg1".to_string(),
                        candidates: Rc::new(RevsetExpression::Symbol("arg2".to_string()))
                    }),
                    Rc::new(RevsetExpression::Parents(Rc::new(
                        RevsetExpression::Symbol("arg1".to_string())
                    )))
                )),
                Rc::new(RevsetExpression::AllHeads)
            ))
        );
    }

    #[test]
    fn test_parse_revset_operator_combinations() {
        // Parse repeated "parents" operator
        assert_eq!(
            parse(":::foo"),
            Ok(RevsetExpression::Parents(Rc::new(
                RevsetExpression::Parents(Rc::new(RevsetExpression::Parents(Rc::new(
                    RevsetExpression::Symbol("foo".to_string())
                ))))
            )))
        );
        // Parse repeated "children" operator
        assert_eq!(
            parse("foo:::"),
            Ok(RevsetExpression::Children {
                roots: Rc::new(RevsetExpression::Children {
                    roots: Rc::new(RevsetExpression::Children {
                        roots: Rc::new(RevsetExpression::Symbol("foo".to_string())),
                        heads: RevsetExpression::non_obsolete_heads(),
                    }),
                    heads: RevsetExpression::non_obsolete_heads(),
                }),
                heads: RevsetExpression::non_obsolete_heads()
            })
        );
        // Parse repeated "ancestors"/"descendants"/"dag range" operators
        assert_matches!(parse(",,foo,,"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse(",,,,foo"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("foo,,,,"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("foo,,,,bar"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse(",,foo,,bar"), Err(RevsetParseError::SyntaxError(_)));
        assert_matches!(parse("foo,,bar,,"), Err(RevsetParseError::SyntaxError(_)));
        // Parse combinations of "parents"/"children" operators and the range operators.
        // The former bind more strongly.
        assert_eq!(
            parse(":foo:"),
            Ok(RevsetExpression::Children {
                roots: Rc::new(RevsetExpression::Parents(Rc::new(
                    RevsetExpression::Symbol("foo".to_string())
                ))),
                heads: RevsetExpression::non_obsolete_heads(),
            })
        );
        assert_eq!(
            parse(":foo,,"),
            Ok(RevsetExpression::DagRange {
                roots: Rc::new(RevsetExpression::Parents(Rc::new(
                    RevsetExpression::Symbol("foo".to_string())
                ))),
                heads: RevsetExpression::non_obsolete_heads(),
            })
        );
        assert_eq!(
            parse(",,foo:"),
            Ok(RevsetExpression::Ancestors(Rc::new(
                RevsetExpression::Children {
                    roots: Rc::new(RevsetExpression::Symbol("foo".to_string())),
                    heads: RevsetExpression::non_obsolete_heads()
                }
            ),))
        );
    }

    #[test]
    fn test_parse_revset_function() {
        assert_eq!(
            parse("parents(@)"),
            Ok(RevsetExpression::Parents(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("parents((@))"),
            Ok(RevsetExpression::Parents(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("parents(\"@\")"),
            Ok(RevsetExpression::Parents(Rc::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("ancestors(parents(@))"),
            Ok(RevsetExpression::Ancestors(Rc::new(
                RevsetExpression::Parents(Rc::new(RevsetExpression::Symbol("@".to_string())))
            )))
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
            Ok(RevsetExpression::Description {
                needle: "foo".to_string(),
                candidates: Rc::new(RevsetExpression::Symbol("bar".to_string()))
            })
        );
        assert_eq!(
            parse("description(all_heads(),bar)"),
            Err(RevsetParseError::InvalidFunctionArguments {
                name: "description".to_string(),
                message: "Expected function argument of type string, found: all_heads()"
                    .to_string()
            })
        );
        assert_eq!(
            parse("description((foo),bar)"),
            Ok(RevsetExpression::Description {
                needle: "foo".to_string(),
                candidates: Rc::new(RevsetExpression::Symbol("bar".to_string()))
            })
        );
        assert_eq!(
            parse("description(\"(foo)\",bar)"),
            Ok(RevsetExpression::Description {
                needle: "(foo)".to_string(),
                candidates: Rc::new(RevsetExpression::Symbol("bar".to_string()))
            })
        );
    }
}
