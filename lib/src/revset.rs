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

use std::cmp::Reverse;

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
}

fn parse_expression_rule(mut pairs: Pairs<Rule>) -> Result<RevsetExpression, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::symbol => Ok(RevsetExpression::Symbol(first.as_str().to_owned())),
        Rule::parents => {
            let expression = pairs.next().unwrap();
            Ok(RevsetExpression::Parents(Box::new(parse_expression_rule(
                expression.into_inner(),
            )?)))
        }
        Rule::ancestors => {
            let expression = pairs.next().unwrap();
            Ok(RevsetExpression::Ancestors(Box::new(
                parse_expression_rule(expression.into_inner())?,
            )))
        }
        Rule::function_name => {
            let name = first.as_str().to_owned();
            let argument_pairs = pairs.next().unwrap().into_inner();
            parse_function_expression(name, argument_pairs)
        }
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

pub fn parse(revset_str: &str) -> Result<RevsetExpression, RevsetParseError> {
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

pub fn evaluate_expression<'repo>(
    repo: RepoRef<'repo>,
    expression: &RevsetExpression,
) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
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
    }
}
