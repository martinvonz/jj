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

use pest::iterators::Pairs;
use pest::Parser;
use thiserror::Error;

use crate::commit::Commit;
use crate::index::{HexPrefix, IndexEntry, PrefixResolution, RevWalk};
use crate::repo::RepoRef;
use crate::store::{CommitId, StoreError};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetError {
    #[error("Revision \"{0}\" doesn't exist")]
    NoSuchRevision(String),
    #[error("Commit id prefix \"{0}\" is ambiguous")]
    AmbiguousCommitIdPrefix(String),
    #[error("Unexpected error from store: {0}")]
    StoreError(#[from] StoreError),
}

// TODO: Decide if we should allow a single symbol to resolve to multiple
// revisions. For example, we may want to resolve a change id to all the
// matching commits. Depending on how we decide to handle divergent git refs and
// similar, we may also want those to resolve to multiple commits.
pub fn resolve_symbol(repo: RepoRef, symbol: &str) -> Result<Commit, RevsetError> {
    // TODO: Support change ids.
    if symbol == "@" {
        Ok(repo.store().get_commit(repo.view().checkout())?)
    } else if symbol == "root" {
        Ok(repo.store().root_commit())
    } else {
        // Try to resolve as a git ref
        let view = repo.view();
        for git_ref_prefix in &["", "refs/", "refs/heads/", "refs/tags/", "refs/remotes/"] {
            if let Some(commit_id) = view.git_refs().get(&(git_ref_prefix.to_string() + symbol)) {
                return Ok(repo.store().get_commit(&commit_id)?);
            }
        }

        // Try to resolve as a commit id. First check if it's a full commit id.
        if let Ok(binary_commit_id) = hex::decode(symbol) {
            let commit_id = CommitId(binary_commit_id);
            match repo.store().get_commit(&commit_id) {
                Ok(commit) => return Ok(commit),
                Err(StoreError::NotFound) => {} // fall through
                Err(err) => return Err(RevsetError::StoreError(err)),
            }
        }

        if let Some(prefix) = HexPrefix::new(symbol.to_string()) {
            match repo.index().resolve_prefix(&prefix) {
                PrefixResolution::NoMatch => {
                    return Err(RevsetError::NoSuchRevision(symbol.to_owned()))
                }
                PrefixResolution::AmbiguousMatch => {
                    return Err(RevsetError::AmbiguousCommitIdPrefix(symbol.to_owned()))
                }
                PrefixResolution::SingleMatch(commit_id) => {
                    return Ok(repo.store().get_commit(&commit_id)?)
                }
            }
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
    SyntaxError(String),
    #[error("Revset function \"{0}\" doesn't exist")]
    NoSuchFunction(String),
    #[error("Invalid arguments to revset function \"{name}\": {message}")]
    InvalidFunctionArguments { name: String, message: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum RevsetExpression {
    Symbol(String),
    Parents(Box<RevsetExpression>),
    Ancestors(Box<RevsetExpression>),
    AllHeads,
    PublicHeads,
    NonObsoleteHeads(Box<RevsetExpression>),
    Description {
        needle: String,
        base_expression: Box<RevsetExpression>,
    },
    Union(Box<RevsetExpression>, Box<RevsetExpression>),
    Intersection(Box<RevsetExpression>, Box<RevsetExpression>),
    Difference(Box<RevsetExpression>, Box<RevsetExpression>),
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
    let mut expression1 = parse_prefix_expression_rule(pairs.next().unwrap().into_inner())?;
    while let Some(operator) = pairs.next() {
        let expression2 = parse_prefix_expression_rule(pairs.next().unwrap().into_inner())?;
        match operator.as_rule() {
            Rule::union => {
                expression1 = RevsetExpression::Union(Box::new(expression1), Box::new(expression2))
            }
            Rule::intersection => {
                expression1 =
                    RevsetExpression::Intersection(Box::new(expression1), Box::new(expression2))
            }
            Rule::difference => {
                expression1 =
                    RevsetExpression::Difference(Box::new(expression1), Box::new(expression2))
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

fn parse_prefix_expression_rule(
    mut pairs: Pairs<Rule>,
) -> Result<RevsetExpression, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::primary => parse_primary_rule(first.into_inner()),
        Rule::parents => Ok(RevsetExpression::Parents(Box::new(
            parse_prefix_expression_rule(pairs)?,
        ))),
        Rule::ancestors => Ok(RevsetExpression::Ancestors(Box::new(
            parse_prefix_expression_rule(pairs)?,
        ))),
        _ => {
            panic!(
                "unxpected revset prefix operator rule {:?}",
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
        Rule::symbol => Ok(RevsetExpression::Symbol(first.as_str().to_owned())),
        _ => {
            panic!("unxpected revset parse rule: {:?}", first.as_str());
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
                Ok(RevsetExpression::Parents(Box::new(
                    parse_function_argument_to_expression(
                        &name,
                        argument_pairs.next().unwrap().into_inner(),
                    )?,
                )))
            } else {
                Err(RevsetParseError::InvalidFunctionArguments {
                    name,
                    message: "Expected 1 argument".to_string(),
                })
            }
        }
        "ancestors" => {
            if arg_count == 1 {
                Ok(RevsetExpression::Ancestors(Box::new(
                    parse_function_argument_to_expression(
                        &name,
                        argument_pairs.next().unwrap().into_inner(),
                    )?,
                )))
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
                Ok(RevsetExpression::NonObsoleteHeads(Box::new(
                    RevsetExpression::AllHeads,
                )))
            } else if arg_count == 1 {
                Ok(RevsetExpression::NonObsoleteHeads(Box::new(
                    parse_function_argument_to_expression(
                        &name,
                        argument_pairs.next().unwrap().into_inner(),
                    )?,
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
            let base_expression = if arg_count == 1 {
                RevsetExpression::Ancestors(Box::new(RevsetExpression::NonObsoleteHeads(Box::new(
                    RevsetExpression::AllHeads,
                ))))
            } else {
                parse_function_argument_to_expression(
                    &name,
                    argument_pairs.next().unwrap().into_inner(),
                )?
            };
            Ok(RevsetExpression::Description {
                needle,
                base_expression: Box::new(base_expression),
            })
        }
        _ => Err(RevsetParseError::NoSuchFunction(name)),
    }
}

fn parse_function_argument_to_expression(
    name: &str,
    mut pairs: Pairs<Rule>,
) -> Result<RevsetExpression, RevsetParseError> {
    // Make a clone of the pairs for error messages
    let pairs_clone = pairs.clone();
    let first = pairs.next().unwrap();
    assert!(pairs.next().is_none());
    match first.as_rule() {
        Rule::expression => Ok(parse_expression_rule(first.into_inner())?),
        _ => Err(RevsetParseError::InvalidFunctionArguments {
            name: name.to_string(),
            message: format!(
                "Expected function argument of type expression, found: {}",
                pairs_clone.as_str()
            ),
        }),
    }
}

fn parse_function_argument_to_string(
    name: &str,
    mut pairs: Pairs<Rule>,
) -> Result<String, RevsetParseError> {
    // Make a clone of the pairs for error messages
    let pairs_clone = pairs.clone();
    let first = pairs.next().unwrap();
    assert!(pairs.next().is_none());
    match first.as_rule() {
        Rule::literal_string => {
            return Ok(first
                .as_str()
                .strip_prefix('"')
                .unwrap()
                .strip_suffix('"')
                .unwrap()
                .to_owned());
        }
        Rule::expression => {
            let first = first.into_inner().next().unwrap();
            if first.as_rule() == Rule::infix_expression {
                let first = first.into_inner().next().unwrap();
                if first.as_rule() == Rule::prefix_expression {
                    let first = first.into_inner().next().unwrap();
                    if first.as_rule() == Rule::primary {
                        let first = first.into_inner().next().unwrap();
                        if first.as_rule() == Rule::symbol {
                            return Ok(first.as_str().to_owned());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Err(RevsetParseError::InvalidFunctionArguments {
        name: name.to_string(),
        message: format!(
            "Expected function argument of type string, found: {}",
            pairs_clone.as_str()
        ),
    })
}

pub fn parse(revset_str: &str) -> Result<RevsetExpression, RevsetParseError> {
    // TODO: Return a better error message when parsing fails (such as when the user
    // puts whitespace between a prefix operator and the operand)
    let mut pairs: Pairs<Rule> = RevsetParser::parse(Rule::expression, revset_str).unwrap();
    let first = pairs.next().unwrap();
    assert!(pairs.next().is_none());
    if first.as_span().end() != revset_str.len() {
        return Err(RevsetParseError::SyntaxError(format!(
            "Failed to parse revset \"{}\" past position {}",
            revset_str,
            first.as_span().end()
        )));
    }

    parse_expression_rule(first.into_inner())
}

pub trait Revset<'repo> {
    // All revsets currently iterate in order of descending index position
    fn iter<'revset>(&'revset self) -> Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>;
}

struct EagerRevset<'repo> {
    index_entries: Vec<IndexEntry<'repo>>,
}

impl<'repo> Revset<'repo> for EagerRevset<'repo> {
    fn iter<'revset>(&'revset self) -> Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset> {
        Box::new(self.index_entries.iter().cloned())
    }
}

struct RevWalkRevset<'repo> {
    walk: RevWalk<'repo>,
}

impl<'repo> Revset<'repo> for RevWalkRevset<'repo> {
    fn iter<'revset>(&'revset self) -> Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset> {
        Box::new(RevWalkRevsetIterator {
            walk: self.walk.clone(),
        })
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

struct UnionRevset<'revset, 'repo: 'revset> {
    set1: Box<dyn Revset<'repo> + 'revset>,
    set2: Box<dyn Revset<'repo> + 'revset>,
}

impl<'repo> Revset<'repo> for UnionRevset<'_, 'repo> {
    fn iter<'revset>(&'revset self) -> Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset> {
        Box::new(UnionRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        })
    }
}

struct UnionRevsetIterator<'revset, 'repo> {
    iter1: Peekable<Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>>,
    iter2: Peekable<Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>>,
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
    fn iter<'revset>(&'revset self) -> Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset> {
        Box::new(IntersectionRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        })
    }
}

struct IntersectionRevsetIterator<'revset, 'repo> {
    iter1: Peekable<Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>>,
    iter2: Peekable<Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>>,
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
    fn iter<'revset>(&'revset self) -> Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset> {
        Box::new(DifferenceRevsetIterator {
            iter1: self.set1.iter().peekable(),
            iter2: self.set2.iter().peekable(),
        })
    }
}

struct DifferenceRevsetIterator<'revset, 'repo> {
    iter1: Peekable<Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>>,
    iter2: Peekable<Box<dyn Iterator<Item = IndexEntry<'repo>> + 'revset>>,
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

pub fn evaluate_expression<'revset, 'repo: 'revset>(
    repo: RepoRef<'repo>,
    expression: &RevsetExpression,
) -> Result<Box<dyn Revset<'repo> + 'revset>, RevsetError> {
    match expression {
        RevsetExpression::Symbol(symbol) => {
            let commit_id = resolve_symbol(repo, &symbol)?.id().clone();
            Ok(Box::new(EagerRevset {
                index_entries: vec![repo.index().entry_by_id(&commit_id).unwrap()],
            }))
        }
        RevsetExpression::Parents(base_expression) => {
            // TODO: Make this lazy
            let base_set = evaluate_expression(repo, base_expression.as_ref())?;
            let mut parent_entries: Vec<_> =
                base_set.iter().flat_map(|entry| entry.parents()).collect();
            parent_entries.sort_by_key(|b| Reverse(b.position()));
            parent_entries.dedup_by_key(|entry| entry.position());
            Ok(Box::new(EagerRevset {
                index_entries: parent_entries,
            }))
        }
        RevsetExpression::Ancestors(base_expression) => {
            let base_set = evaluate_expression(repo, base_expression.as_ref())?;
            let base_ids: Vec<_> = base_set.iter().map(|entry| entry.commit_id()).collect();
            let walk = repo.index().walk_revs(&base_ids, &[]);
            Ok(Box::new(RevWalkRevset { walk }))
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
            let base_set = evaluate_expression(repo, base_expression.as_ref())?;
            Ok(non_obsolete_heads(repo, base_set))
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
        RevsetExpression::Description {
            needle,
            base_expression,
        } => {
            // TODO: Definitely make this lazy. We should have a common way of defining
            // revsets that simply filter a base revset.
            let base_set = evaluate_expression(repo, base_expression.as_ref())?;
            let mut commit_ids = vec![];
            for entry in base_set.iter() {
                let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
                if commit.description().contains(needle.as_str()) {
                    commit_ids.push(entry.commit_id());
                }
            }
            let index = repo.index();
            let mut index_entries: Vec<_> = commit_ids
                .iter()
                .map(|id| index.entry_by_id(id).unwrap())
                .collect();
            index_entries.sort_by_key(|b| Reverse(b.position()));
            Ok(Box::new(EagerRevset { index_entries }))
        }
        RevsetExpression::Union(expression1, expression2) => {
            let set1 = evaluate_expression(repo, expression1.as_ref())?;
            let set2 = evaluate_expression(repo, expression2.as_ref())?;
            Ok(Box::new(UnionRevset { set1, set2 }))
        }
        RevsetExpression::Intersection(expression1, expression2) => {
            let set1 = evaluate_expression(repo, expression1.as_ref())?;
            let set2 = evaluate_expression(repo, expression2.as_ref())?;
            Ok(Box::new(IntersectionRevset { set1, set2 }))
        }
        RevsetExpression::Difference(expression1, expression2) => {
            let set1 = evaluate_expression(repo, expression1.as_ref())?;
            let set2 = evaluate_expression(repo, expression2.as_ref())?;
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
    index_entries.sort_by_key(|b| Reverse(b.position()));
    Box::new(EagerRevset { index_entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_revset() {
        assert_eq!(parse("@"), Ok(RevsetExpression::Symbol("@".to_string())));
        assert_eq!(
            parse("foo"),
            Ok(RevsetExpression::Symbol("foo".to_string()))
        );
        assert_eq!(
            parse("(foo)"),
            Ok(RevsetExpression::Symbol("foo".to_string()))
        );
        assert_eq!(
            parse(":@"),
            Ok(RevsetExpression::Parents(Box::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse(":(@)"),
            Ok(RevsetExpression::Parents(Box::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("*:@"),
            Ok(RevsetExpression::Ancestors(Box::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse(" *:@ "),
            Ok(RevsetExpression::Ancestors(Box::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("   description(  arg1 ,   arg2 ) -    parents(   arg1  )  - all_heads(  )  "),
            Ok(RevsetExpression::Difference(
                Box::new(RevsetExpression::Difference(
                    Box::new(RevsetExpression::Description {
                        needle: "arg1".to_string(),
                        base_expression: Box::new(RevsetExpression::Symbol("arg2".to_string()))
                    }),
                    Box::new(RevsetExpression::Parents(Box::new(
                        RevsetExpression::Symbol("arg1".to_string())
                    )))
                )),
                Box::new(RevsetExpression::AllHeads)
            ))
        );
    }

    #[test]
    fn test_parse_revset_function() {
        assert_eq!(
            parse("parents(@)"),
            Ok(RevsetExpression::Parents(Box::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("parents((@))"),
            Ok(RevsetExpression::Parents(Box::new(
                RevsetExpression::Symbol("@".to_string())
            )))
        );
        assert_eq!(
            parse("parents(\"@\")"),
            Err(RevsetParseError::InvalidFunctionArguments {
                name: "parents".to_string(),
                message: "Expected function argument of type expression, found: \"@\"".to_string()
            })
        );
        assert_eq!(
            parse("ancestors(parents(@))"),
            Ok(RevsetExpression::Ancestors(Box::new(
                RevsetExpression::Parents(Box::new(RevsetExpression::Symbol("@".to_string())))
            )))
        );
        assert_eq!(
            parse("parents(@"),
            Err(RevsetParseError::SyntaxError(
                "Failed to parse revset \"parents(@\" past position 7".to_string()
            ))
        );
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
                base_expression: Box::new(RevsetExpression::Symbol("bar".to_string()))
            })
        );
        assert_eq!(
            parse("description(foo(),bar)"),
            Err(RevsetParseError::InvalidFunctionArguments {
                name: "description".to_string(),
                message: "Expected function argument of type string, found: foo()".to_string()
            })
        );
        assert_eq!(
            parse("description((foo),bar)"),
            Err(RevsetParseError::InvalidFunctionArguments {
                name: "description".to_string(),
                message: "Expected function argument of type string, found: (foo)".to_string()
            })
        );
        assert_eq!(
            parse("description(\"(foo)\",bar)"),
            Ok(RevsetExpression::Description {
                needle: "(foo)".to_string(),
                base_expression: Box::new(RevsetExpression::Symbol("bar".to_string()))
            })
        );
    }
}
