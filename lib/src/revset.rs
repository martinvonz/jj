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

use std::borrow::Borrow;
use std::cmp::{Ordering, Reverse};
use std::collections::HashSet;
use std::iter::Peekable;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::{error, fmt};

use itertools::Itertools;
use once_cell::sync::Lazy;
use pest::iterators::{Pair, Pairs};
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::backend::{BackendError, BackendResult, CommitId};
use crate::commit::Commit;
use crate::index::{HexPrefix, IndexEntry, IndexPosition, PrefixResolution, RevWalk};
use crate::matchers::{EverythingMatcher, Matcher, PrefixMatcher};
use crate::op_store::WorkspaceId;
use crate::repo::RepoRef;
use crate::repo_path::{FsPathParseError, RepoPath};
use crate::revset_graph_iterator::RevsetGraphIterator;
use crate::rewrite;
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

fn resolve_git_ref(repo: RepoRef, symbol: &str) -> Option<Vec<CommitId>> {
    let view = repo.view();
    for git_ref_prefix in &["", "refs/", "refs/heads/", "refs/tags/", "refs/remotes/"] {
        if let Some(ref_target) = view.git_refs().get(&(git_ref_prefix.to_string() + symbol)) {
            return Some(ref_target.adds());
        }
    }
    None
}

fn resolve_branch(repo: RepoRef, symbol: &str) -> Option<Vec<CommitId>> {
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
    repo: RepoRef,
    symbol: &str,
) -> Result<Option<Vec<CommitId>>, RevsetError> {
    if let Ok(binary_commit_id) = hex::decode(symbol) {
        let commit_id = CommitId::new(binary_commit_id);
        match repo.store().get_commit(&commit_id) {
            Ok(_) => Ok(Some(vec![commit_id])),
            Err(BackendError::NotFound) => Ok(None),
            Err(err) => Err(RevsetError::StoreError(err)),
        }
    } else {
        Ok(None)
    }
}

fn resolve_short_commit_id(
    repo: RepoRef,
    symbol: &str,
) -> Result<Option<Vec<CommitId>>, RevsetError> {
    if let Some(prefix) = HexPrefix::new(symbol.to_owned()) {
        match repo.index().resolve_prefix(&prefix) {
            PrefixResolution::NoMatch => Ok(None),
            PrefixResolution::AmbiguousMatch => {
                Err(RevsetError::AmbiguousCommitIdPrefix(symbol.to_owned()))
            }
            PrefixResolution::SingleMatch(commit_id) => Ok(Some(vec![commit_id])),
        }
    } else {
        Ok(None)
    }
}

fn resolve_change_id(
    repo: RepoRef,
    change_id_prefix: &str,
) -> Result<Option<Vec<CommitId>>, RevsetError> {
    if let Some(hex_prefix) = HexPrefix::new(change_id_prefix.to_owned()) {
        let mut found_change_id = None;
        let mut commit_ids = vec![];
        // TODO: Create a persistent lookup from change id to (visible?) commit ids.
        for index_entry in RevsetExpression::all().evaluate(repo, None).unwrap().iter() {
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
            return Ok(None);
        }
        Ok(Some(commit_ids))
    } else {
        Ok(None)
    }
}

pub fn resolve_symbol(
    repo: RepoRef,
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

        // Try to resolve as a commit/change id.
        match (
            resolve_short_commit_id(repo, symbol)?,
            resolve_change_id(repo, symbol)?,
        ) {
            // Likely a root_commit_id, but not limited to it.
            (Some(ids1), Some(ids2)) if ids1 == ids2 => Ok(ids1),
            // TODO: maybe unify Ambiguous*IdPrefix error variants?
            (Some(_), Some(_)) => Err(RevsetError::AmbiguousCommitIdPrefix(symbol.to_owned())),
            (Some(ids), None) | (None, Some(ids)) => Ok(ids),
            (None, None) => Err(RevsetError::NoSuchRevision(symbol.to_owned())),
        }
    }
}

#[derive(Parser)]
#[grammar = "revset.pest"]
pub struct RevsetParser;

#[derive(Debug)]
pub struct RevsetParseError {
    kind: RevsetParseErrorKind,
    pest_error: Option<Box<pest::error::Error<Rule>>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetParseErrorKind {
    #[error("Syntax error")]
    SyntaxError,
    #[error("Revset function \"{0}\" doesn't exist")]
    NoSuchFunction(String),
    #[error("Invalid arguments to revset function \"{name}\": {message}")]
    InvalidFunctionArguments { name: String, message: String },
    #[error("Invalid file pattern: {0}")]
    FsPathParseError(#[source] FsPathParseError),
    #[error("Cannot resolve file pattern without workspace")]
    FsPathWithoutWorkspace,
}

impl RevsetParseError {
    fn new(kind: RevsetParseErrorKind) -> Self {
        RevsetParseError {
            kind,
            pest_error: None,
        }
    }

    fn with_span(kind: RevsetParseErrorKind, span: pest::Span<'_>) -> Self {
        let err = pest::error::Error::new_from_span(
            pest::error::ErrorVariant::CustomError {
                message: kind.to_string(),
            },
            span,
        );
        RevsetParseError {
            kind,
            pest_error: Some(Box::new(err)),
        }
    }

    pub fn kind(&self) -> &RevsetParseErrorKind {
        &self.kind
    }
}

impl From<pest::error::Error<Rule>> for RevsetParseError {
    fn from(err: pest::error::Error<Rule>) -> Self {
        RevsetParseError {
            kind: RevsetParseErrorKind::SyntaxError,
            pest_error: Some(Box::new(err)),
        }
    }
}

impl fmt::Display for RevsetParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(err) = &self.pest_error {
            err.fmt(f)
        } else {
            self.kind.fmt(f)
        }
    }
}

impl error::Error for RevsetParseError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match &self.kind {
            // SyntaxError is a wrapper for pest::error::Error.
            RevsetParseErrorKind::SyntaxError => {
                self.pest_error.as_ref().map(|e| e as &dyn error::Error)
            }
            // Otherwise the kind represents this error.
            e => e.source(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevsetFilterPredicate {
    /// Commits with number of parents in the range.
    ParentCount(Range<u32>),
    /// Commits with description containing the needle.
    Description(String),
    /// Commits with author's name or email containing the needle.
    Author(String),
    /// Commits with committer's name or email containing the needle.
    Committer(String),
    /// Commits modifying no files. Equivalent to `Not(File(["."]))`.
    Empty,
    /// Commits modifying the paths specified by the pattern.
    File(Vec<RepoPath>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RevsetExpression {
    None,
    All,
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
    Heads(Rc<RevsetExpression>),
    Roots(Rc<RevsetExpression>),
    VisibleHeads,
    PublicHeads,
    Branches,
    RemoteBranches,
    Tags,
    GitRefs,
    GitHead,
    Filter {
        candidates: Rc<RevsetExpression>,
        predicate: RevsetFilterPredicate,
    },
    Present(Rc<RevsetExpression>),
    Union(Rc<RevsetExpression>, Rc<RevsetExpression>),
    Intersection(Rc<RevsetExpression>, Rc<RevsetExpression>),
    Difference(Rc<RevsetExpression>, Rc<RevsetExpression>),
}

impl RevsetExpression {
    pub fn none() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::None)
    }

    pub fn all() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::All)
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

    pub fn visible_heads() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::VisibleHeads)
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

    pub fn filter(predicate: RevsetFilterPredicate) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Filter {
            candidates: RevsetExpression::all(),
            predicate,
        })
    }

    /// Commits in `self` that don't have descendants in `self`.
    pub fn heads(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Heads(self.clone()))
    }

    /// Commits in `self` that don't have ancestors in `self`.
    pub fn roots(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Roots(self.clone()))
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
        self.dag_range_to(&RevsetExpression::visible_heads())
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

    /// Connects any ancestors and descendants in the set by adding the commits
    /// between them.
    pub fn connected(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        self.dag_range_to(self)
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
        workspace_ctx: Option<&RevsetWorkspaceContext>,
    ) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
        evaluate_expression(repo, self, workspace_ctx)
    }
}

fn parse_expression_rule(
    pairs: Pairs<Rule>,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    static PRATT: Lazy<PrattParser<Rule>> = Lazy::new(|| {
        PrattParser::new()
            .op(Op::infix(Rule::union_op, Assoc::Left))
            .op(Op::infix(Rule::intersection_op, Assoc::Left)
                | Op::infix(Rule::difference_op, Assoc::Left))
            // Ranges can't be nested without parentheses. Associativity doesn't matter.
            .op(Op::infix(Rule::dag_range_op, Assoc::Left) | Op::infix(Rule::range_op, Assoc::Left))
            .op(Op::prefix(Rule::dag_range_pre_op) | Op::prefix(Rule::range_pre_op))
            .op(Op::postfix(Rule::dag_range_post_op) | Op::postfix(Rule::range_post_op))
            // Neighbors
            .op(Op::postfix(Rule::parents_op) | Op::postfix(Rule::children_op))
    });
    PRATT
        .map_primary(|primary| parse_primary_rule(primary.into_inner(), workspace_ctx))
        .map_prefix(|op, rhs| match op.as_rule() {
            Rule::dag_range_pre_op | Rule::range_pre_op => Ok(rhs?.ancestors()),
            r => panic!("unexpected prefix operator rule {r:?}"),
        })
        .map_postfix(|lhs, op| match op.as_rule() {
            Rule::dag_range_post_op => Ok(lhs?.descendants()),
            Rule::range_post_op => Ok(lhs?.range(&RevsetExpression::visible_heads())),
            Rule::parents_op => Ok(lhs?.parents()),
            Rule::children_op => Ok(lhs?.children()),
            r => panic!("unexpected postfix operator rule {r:?}"),
        })
        .map_infix(|lhs, op, rhs| match op.as_rule() {
            Rule::union_op => Ok(lhs?.union(&rhs?)),
            Rule::intersection_op => Ok(lhs?.intersection(&rhs?)),
            Rule::difference_op => Ok(lhs?.minus(&rhs?)),
            Rule::dag_range_op => Ok(lhs?.dag_range_to(&rhs?)),
            Rule::range_op => Ok(lhs?.range(&rhs?)),
            r => panic!("unexpected infix operator rule {r:?}"),
        })
        .parse(pairs)
}

fn parse_primary_rule(
    mut pairs: Pairs<Rule>,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::expression => parse_expression_rule(first.into_inner(), workspace_ctx),
        Rule::function_name => {
            let arguments_pair = pairs.next().unwrap();
            parse_function_expression(first, arguments_pair, workspace_ctx)
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
    name_pair: Pair<Rule>,
    arguments_pair: Pair<Rule>,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let name = name_pair.as_str();
    match name {
        "parents" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let expression = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(expression.parents())
        }
        "children" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let expression = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(expression.children())
        }
        "ancestors" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let expression = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(expression.ancestors())
        }
        "descendants" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let expression = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(expression.descendants())
        }
        "connected" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let candidates = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(candidates.connected())
        }
        "none" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::none())
        }
        "all" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::all())
        }
        "heads" => {
            if let Some(arg) = expect_one_optional_argument(name, arguments_pair)? {
                let candidates = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
                Ok(candidates.heads())
            } else {
                Ok(RevsetExpression::visible_heads())
            }
        }
        "roots" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let candidates = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(candidates.roots())
        }
        "public_heads" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::public_heads())
        }
        "branches" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::branches())
        }
        "remote_branches" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::remote_branches())
        }
        "tags" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::tags())
        }
        "git_refs" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::git_refs())
        }
        "git_head" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::git_head())
        }
        "merges" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::ParentCount(2..u32::MAX),
            ))
        }
        "description" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let needle = parse_function_argument_to_string(name, arg)?;
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description(needle),
            ))
        }
        "author" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let needle = parse_function_argument_to_string(name, arg)?;
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
                needle,
            )))
        }
        "committer" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let needle = parse_function_argument_to_string(name, arg)?;
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Committer(
                needle,
            )))
        }
        "empty" => {
            expect_no_arguments(name, arguments_pair)?;
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Empty))
        }
        "file" => {
            if let Some(ctx) = workspace_ctx {
                let arguments_span = arguments_pair.as_span();
                let paths = arguments_pair
                    .into_inner()
                    .map(|arg| {
                        let span = arg.as_span();
                        let needle = parse_function_argument_to_string(name, arg)?;
                        let path = RepoPath::parse_fs_path(ctx.cwd, ctx.workspace_root, &needle)
                            .map_err(|e| {
                                RevsetParseError::with_span(
                                    RevsetParseErrorKind::FsPathParseError(e),
                                    span,
                                )
                            })?;
                        Ok(path)
                    })
                    .collect::<Result<Vec<_>, RevsetParseError>>()?;
                if paths.is_empty() {
                    Err(RevsetParseError::with_span(
                        RevsetParseErrorKind::InvalidFunctionArguments {
                            name: name.to_owned(),
                            message: "Expected at least 1 argument".to_string(),
                        },
                        arguments_span,
                    ))
                } else {
                    Ok(RevsetExpression::filter(RevsetFilterPredicate::File(paths)))
                }
            } else {
                Err(RevsetParseError::new(
                    RevsetParseErrorKind::FsPathWithoutWorkspace,
                ))
            }
        }
        "present" => {
            let arg = expect_one_argument(name, arguments_pair)?;
            let expression = parse_expression_rule(arg.into_inner(), workspace_ctx)?;
            Ok(Rc::new(RevsetExpression::Present(expression)))
        }
        _ => Err(RevsetParseError::with_span(
            RevsetParseErrorKind::NoSuchFunction(name.to_owned()),
            name_pair.as_span(),
        )),
    }
}

fn expect_no_arguments(name: &str, arguments_pair: Pair<Rule>) -> Result<(), RevsetParseError> {
    let span = arguments_pair.as_span();
    let mut argument_pairs = arguments_pair.into_inner();
    if argument_pairs.next().is_none() {
        Ok(())
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: name.to_owned(),
                message: "Expected 0 arguments".to_string(),
            },
            span,
        ))
    }
}

fn expect_one_argument<'i>(
    name: &str,
    arguments_pair: Pair<'i, Rule>,
) -> Result<Pair<'i, Rule>, RevsetParseError> {
    let span = arguments_pair.as_span();
    let mut argument_pairs = arguments_pair.into_inner().fuse();
    if let (Some(arg), None) = (argument_pairs.next(), argument_pairs.next()) {
        Ok(arg)
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: name.to_owned(),
                message: "Expected 1 argument".to_string(),
            },
            span,
        ))
    }
}

fn expect_one_optional_argument<'i>(
    name: &str,
    arguments_pair: Pair<'i, Rule>,
) -> Result<Option<Pair<'i, Rule>>, RevsetParseError> {
    let span = arguments_pair.as_span();
    let mut argument_pairs = arguments_pair.into_inner().fuse();
    if let (opt_arg, None) = (argument_pairs.next(), argument_pairs.next()) {
        Ok(opt_arg)
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: name.to_owned(),
                message: "Expected 0 or 1 arguments".to_string(),
            },
            span,
        ))
    }
}

fn parse_function_argument_to_string(
    name: &str,
    pair: Pair<Rule>,
) -> Result<String, RevsetParseError> {
    let span = pair.as_span();
    let workspace_ctx = None; // string literal shouldn't depend on workspace information
    let expression = parse_expression_rule(pair.into_inner(), workspace_ctx)?;
    match expression.as_ref() {
        RevsetExpression::Symbol(symbol) => Ok(symbol.clone()),
        _ => Err(RevsetParseError::with_span(
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: name.to_string(),
                message: "Expected function argument of type string".to_owned(),
            },
            span,
        )),
    }
}

pub fn parse(
    revset_str: &str,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::program, revset_str)?;
    let first = pairs.next().unwrap();
    parse_expression_rule(first.into_inner(), workspace_ctx)
}

/// Walks `expression` tree and applies `f` recursively from leaf nodes.
///
/// If `f` returns `None`, the original expression node is reused. If no nodes
/// rewritten, returns `None`. `std::iter::successors()` could be used if
/// the transformation needs to be applied repeatedly until converged.
fn transform_expression_bottom_up(
    expression: &Rc<RevsetExpression>,
    mut f: impl FnMut(&Rc<RevsetExpression>) -> Option<Rc<RevsetExpression>>,
) -> Option<Rc<RevsetExpression>> {
    fn transform_child_rec(
        expression: &Rc<RevsetExpression>,
        f: &mut impl FnMut(&Rc<RevsetExpression>) -> Option<Rc<RevsetExpression>>,
    ) -> Option<Rc<RevsetExpression>> {
        match expression.as_ref() {
            RevsetExpression::None => None,
            RevsetExpression::All => None,
            RevsetExpression::Commits(_) => None,
            RevsetExpression::Symbol(_) => None,
            RevsetExpression::Parents(base) => {
                transform_rec(base, f).map(RevsetExpression::Parents)
            }
            RevsetExpression::Children(roots) => {
                transform_rec(roots, f).map(RevsetExpression::Children)
            }
            RevsetExpression::Ancestors(base) => {
                transform_rec(base, f).map(RevsetExpression::Ancestors)
            }
            RevsetExpression::Range { roots, heads } => transform_rec_pair((roots, heads), f)
                .map(|(roots, heads)| RevsetExpression::Range { roots, heads }),
            RevsetExpression::DagRange { roots, heads } => transform_rec_pair((roots, heads), f)
                .map(|(roots, heads)| RevsetExpression::DagRange { roots, heads }),
            RevsetExpression::VisibleHeads => None,
            RevsetExpression::Heads(candidates) => {
                transform_rec(candidates, f).map(RevsetExpression::Heads)
            }
            RevsetExpression::Roots(candidates) => {
                transform_rec(candidates, f).map(RevsetExpression::Roots)
            }
            RevsetExpression::PublicHeads => None,
            RevsetExpression::Branches => None,
            RevsetExpression::RemoteBranches => None,
            RevsetExpression::Tags => None,
            RevsetExpression::GitRefs => None,
            RevsetExpression::GitHead => None,
            RevsetExpression::Filter {
                candidates,
                predicate,
            } => transform_rec(candidates, f).map(|candidates| RevsetExpression::Filter {
                candidates,
                predicate: predicate.clone(),
            }),
            RevsetExpression::Present(candidates) => {
                transform_rec(candidates, f).map(RevsetExpression::Present)
            }
            RevsetExpression::Union(expression1, expression2) => {
                transform_rec_pair((expression1, expression2), f).map(
                    |(expression1, expression2)| RevsetExpression::Union(expression1, expression2),
                )
            }
            RevsetExpression::Intersection(expression1, expression2) => {
                transform_rec_pair((expression1, expression2), f).map(
                    |(expression1, expression2)| {
                        RevsetExpression::Intersection(expression1, expression2)
                    },
                )
            }
            RevsetExpression::Difference(expression1, expression2) => {
                transform_rec_pair((expression1, expression2), f).map(
                    |(expression1, expression2)| {
                        RevsetExpression::Difference(expression1, expression2)
                    },
                )
            }
        }
        .map(Rc::new)
    }

    fn transform_rec_pair(
        (expression1, expression2): (&Rc<RevsetExpression>, &Rc<RevsetExpression>),
        f: &mut impl FnMut(&Rc<RevsetExpression>) -> Option<Rc<RevsetExpression>>,
    ) -> Option<(Rc<RevsetExpression>, Rc<RevsetExpression>)> {
        match (transform_rec(expression1, f), transform_rec(expression2, f)) {
            (Some(new_expression1), Some(new_expression2)) => {
                Some((new_expression1, new_expression2))
            }
            (Some(new_expression1), None) => Some((new_expression1, expression2.clone())),
            (None, Some(new_expression2)) => Some((expression1.clone(), new_expression2)),
            (None, None) => None,
        }
    }

    fn transform_rec(
        expression: &Rc<RevsetExpression>,
        f: &mut impl FnMut(&Rc<RevsetExpression>) -> Option<Rc<RevsetExpression>>,
    ) -> Option<Rc<RevsetExpression>> {
        if let Some(new_expression) = transform_child_rec(expression, f) {
            // must propagate new expression tree
            Some(f(&new_expression).unwrap_or(new_expression))
        } else {
            f(expression)
        }
    }

    transform_rec(expression, &mut f)
}

/// Transforms intersection of filter expressions. The resulting tree may
/// contain redundant intersections like `all() & e`.
fn internalize_filter_intersection(
    expression: &Rc<RevsetExpression>,
) -> Option<Rc<RevsetExpression>> {
    // Since both sides must have already been "internalize"d, we don't need to
    // apply the whole bottom-up pass to new intersection node. Instead, just push
    // new 'c & g(d)' down to 'g(c & d)' while either side is a filter node.
    fn intersect_down(
        expression1: &Rc<RevsetExpression>,
        expression2: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        if let RevsetExpression::Filter {
            candidates,
            predicate,
        } = expression2.as_ref()
        {
            // e1 & f2(c2) -> f2(e1 & c2)
            // f1(c1) & f2(c2) -> f2(f1(c1) & c2) -> f2(f1(c1 & c2))
            Rc::new(RevsetExpression::Filter {
                candidates: intersect_down(expression1, candidates),
                predicate: predicate.clone(),
            })
        } else if let RevsetExpression::Filter {
            candidates,
            predicate,
        } = expression1.as_ref()
        {
            // f1(c1) & e2 -> f1(c1 & e2)
            // g1(f1(c1)) & e2 -> g1(f1(c1) & e2) -> g1(f1(c1 & e2))
            Rc::new(RevsetExpression::Filter {
                candidates: intersect_down(candidates, expression2),
                predicate: predicate.clone(),
            })
        } else {
            expression1.intersection(expression2)
        }
    }

    // Bottom-up pass pulls up filter node from leaf 'f(c) & e' -> 'f(c & e)',
    // so that a filter node can be found as a direct child of an intersection node.
    // However, the rewritten intersection node 'c & e' can also be a rewrite target
    // if 'e' is a filter node. That's why intersect_down() is also recursive.
    transform_expression_bottom_up(expression, |expression| {
        if let RevsetExpression::Intersection(expression1, expression2) = expression.as_ref() {
            match (expression1.as_ref(), expression2.as_ref()) {
                (_, RevsetExpression::Filter { .. }) | (RevsetExpression::Filter { .. }, _) => {
                    Some(intersect_down(expression1, expression2))
                }
                _ => None, // don't recreate identical node
            }
        } else {
            None
        }
    })
}

/// Eliminates redundant intersection with `all()`.
fn fold_intersection_with_all(expression: &Rc<RevsetExpression>) -> Option<Rc<RevsetExpression>> {
    transform_expression_bottom_up(expression, |expression| {
        if let RevsetExpression::Intersection(expression1, expression2) = expression.as_ref() {
            match (expression1.as_ref(), expression2.as_ref()) {
                (_, RevsetExpression::All) => Some(expression1.clone()),
                (RevsetExpression::All, _) => Some(expression2.clone()),
                _ => None,
            }
        } else {
            None
        }
    })
}

/// Rewrites the given `expression` tree to reduce evaluation cost. Returns new
/// tree.
pub fn optimize(expression: Rc<RevsetExpression>) -> Rc<RevsetExpression> {
    let expression = internalize_filter_intersection(&expression).unwrap_or(expression);
    fold_intersection_with_all(&expression).unwrap_or(expression)
}

pub trait Revset<'repo> {
    // All revsets currently iterate in order of descending index position
    fn iter<'revset>(&'revset self) -> RevsetIterator<'revset, 'repo>;

    fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
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

    pub fn reversed(self) -> ReverseRevsetIterator<'repo> {
        ReverseRevsetIterator {
            entries: self.into_iter().collect_vec(),
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

pub struct ReverseRevsetIterator<'repo> {
    entries: Vec<IndexEntry<'repo>>,
}

impl<'repo> Iterator for ReverseRevsetIterator<'repo> {
    type Item = IndexEntry<'repo>;

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.pop()
    }
}

struct EagerRevset<'repo> {
    index_entries: Vec<IndexEntry<'repo>>,
}

impl EagerRevset<'static> {
    pub const fn empty() -> Self {
        EagerRevset {
            index_entries: Vec::new(),
        }
    }
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
        loop {
            let candidate = self.candidate_iter.next()?;
            if candidate
                .parent_positions()
                .iter()
                .any(|parent_pos| self.roots.contains(parent_pos))
            {
                return Some(candidate);
            }
        }
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
        self.iter.find(self.predicate)
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

/// Workspace information needed to evaluate revset expression.
#[derive(Clone, Debug)]
pub struct RevsetWorkspaceContext<'a> {
    pub cwd: &'a Path,
    pub workspace_id: &'a WorkspaceId,
    pub workspace_root: &'a Path,
}

pub fn evaluate_expression<'repo>(
    repo: RepoRef<'repo>,
    expression: &RevsetExpression,
    workspace_ctx: Option<&RevsetWorkspaceContext>,
) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
    match expression {
        RevsetExpression::None => Ok(Box::new(EagerRevset::empty())),
        RevsetExpression::All => {
            // Since `all()` does not include hidden commits, some of the logical
            // transformation rules may subtly change the evaluated set. For example,
            // `all() & x` is not `x` if `x` is hidden. This wouldn't matter in practice,
            // but if it does, the heads set could be extended to include the commits
            // (and `remote_branches()`) specified in the revset expression. Alternatively,
            // some optimization rules could be removed, but that means `author(_) & x`
            // would have to test `:heads() & x`.
            evaluate_expression(
                repo,
                &RevsetExpression::visible_heads().ancestors(),
                workspace_ctx,
            )
        }
        RevsetExpression::Commits(commit_ids) => Ok(revset_for_commit_ids(repo, commit_ids)),
        RevsetExpression::Symbol(symbol) => {
            let commit_ids = resolve_symbol(repo, symbol, workspace_ctx.map(|c| c.workspace_id))?;
            evaluate_expression(repo, &RevsetExpression::Commits(commit_ids), workspace_ctx)
        }
        RevsetExpression::Parents(base_expression) => {
            // TODO: Make this lazy
            let base_set = base_expression.evaluate(repo, workspace_ctx)?;
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
            let root_set = roots.evaluate(repo, workspace_ctx)?;
            let candidates_expression = roots.descendants();
            let candidate_set = candidates_expression.evaluate(repo, workspace_ctx)?;
            Ok(Box::new(ChildrenRevset {
                root_set,
                candidate_set,
            }))
        }
        RevsetExpression::Ancestors(base_expression) => RevsetExpression::none()
            .range(base_expression)
            .evaluate(repo, workspace_ctx),
        RevsetExpression::Range { roots, heads } => {
            let root_set = roots.evaluate(repo, workspace_ctx)?;
            let root_ids = root_set.iter().commit_ids().collect_vec();
            let head_set = heads.evaluate(repo, workspace_ctx)?;
            let head_ids = head_set.iter().commit_ids().collect_vec();
            let walk = repo.index().walk_revs(&head_ids, &root_ids);
            Ok(Box::new(RevWalkRevset { walk }))
        }
        // Clippy doesn't seem to understand that we collect the iterator in order to iterate in
        // reverse
        #[allow(clippy::needless_collect)]
        RevsetExpression::DagRange { roots, heads } => {
            let root_set = roots.evaluate(repo, workspace_ctx)?;
            let candidate_set = heads.ancestors().evaluate(repo, workspace_ctx)?;
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
        RevsetExpression::VisibleHeads => Ok(revset_for_commit_ids(
            repo,
            &repo.view().heads().iter().cloned().collect_vec(),
        )),
        RevsetExpression::Heads(candidates) => {
            let candidate_set = candidates.evaluate(repo, workspace_ctx)?;
            let candidate_ids = candidate_set.iter().commit_ids().collect_vec();
            Ok(revset_for_commit_ids(
                repo,
                &repo.index().heads(&candidate_ids),
            ))
        }
        RevsetExpression::Roots(candidates) => {
            let connected_set = candidates.connected().evaluate(repo, workspace_ctx)?;
            let filled: HashSet<_> = connected_set.iter().map(|entry| entry.position()).collect();
            let mut index_entries = vec![];
            let candidate_set = candidates.evaluate(repo, workspace_ctx)?;
            for candidate in candidate_set.iter() {
                if !candidate
                    .parent_positions()
                    .iter()
                    .any(|parent| filled.contains(parent))
                {
                    index_entries.push(candidate);
                }
            }
            Ok(Box::new(EagerRevset { index_entries }))
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
        RevsetExpression::Filter {
            candidates,
            predicate,
        } => {
            let candidates = candidates.evaluate(repo, workspace_ctx)?;
            match predicate {
                RevsetFilterPredicate::ParentCount(parent_count_range) => {
                    let parent_count_range = parent_count_range.clone();
                    Ok(Box::new(FilterRevset {
                        candidates,
                        predicate: Box::new(move |entry| {
                            parent_count_range.contains(&entry.num_parents())
                        }),
                    }))
                }
                RevsetFilterPredicate::Description(needle) => {
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
                RevsetFilterPredicate::Author(needle) => {
                    let needle = needle.clone();
                    // TODO: Make these functions that take a needle to search for accept some
                    // syntax for specifying whether it's a regex and whether it's
                    // case-sensitive.
                    Ok(Box::new(FilterRevset {
                        candidates,
                        predicate: Box::new(move |entry| {
                            let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
                            commit.author().name.contains(needle.as_str())
                                || commit.author().email.contains(needle.as_str())
                        }),
                    }))
                }
                RevsetFilterPredicate::Committer(needle) => {
                    let needle = needle.clone();
                    Ok(Box::new(FilterRevset {
                        candidates,
                        predicate: Box::new(move |entry| {
                            let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
                            commit.committer().name.contains(needle.as_str())
                                || commit.committer().email.contains(needle.as_str())
                        }),
                    }))
                }
                RevsetFilterPredicate::Empty => Ok(Box::new(FilterRevset {
                    candidates,
                    predicate: Box::new(move |entry| {
                        !has_diff_from_parent(repo, entry, &EverythingMatcher)
                    }),
                })),
                RevsetFilterPredicate::File(paths) => {
                    // TODO: Add support for globs and other formats
                    let matcher: Box<dyn Matcher> = Box::new(PrefixMatcher::new(paths));
                    Ok(filter_by_diff(repo, matcher, candidates))
                }
            }
        }
        RevsetExpression::Present(candidates) => match candidates.evaluate(repo, workspace_ctx) {
            Ok(set) => Ok(set),
            Err(RevsetError::NoSuchRevision(_)) => Ok(Box::new(EagerRevset::empty())),
            r @ Err(
                RevsetError::AmbiguousCommitIdPrefix(_)
                | RevsetError::AmbiguousChangeIdPrefix(_)
                | RevsetError::StoreError(_),
            ) => r,
        },
        RevsetExpression::Union(expression1, expression2) => {
            let set1 = expression1.evaluate(repo, workspace_ctx)?;
            let set2 = expression2.evaluate(repo, workspace_ctx)?;
            Ok(Box::new(UnionRevset { set1, set2 }))
        }
        RevsetExpression::Intersection(expression1, expression2) => {
            let set1 = expression1.evaluate(repo, workspace_ctx)?;
            let set2 = expression2.evaluate(repo, workspace_ctx)?;
            Ok(Box::new(IntersectionRevset { set1, set2 }))
        }
        RevsetExpression::Difference(expression1, expression2) => {
            let set1 = expression1.evaluate(repo, workspace_ctx)?;
            let set2 = expression2.evaluate(repo, workspace_ctx)?;
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

pub fn filter_by_diff<'revset, 'repo: 'revset>(
    repo: RepoRef<'repo>,
    matcher: impl Borrow<dyn Matcher + 'repo> + 'repo,
    candidates: Box<dyn Revset<'repo> + 'revset>,
) -> Box<dyn Revset<'repo> + 'revset> {
    Box::new(FilterRevset {
        candidates,
        predicate: Box::new(move |entry| has_diff_from_parent(repo, entry, matcher.borrow())),
    })
}

fn has_diff_from_parent(repo: RepoRef<'_>, entry: &IndexEntry<'_>, matcher: &dyn Matcher) -> bool {
    let commit = repo.store().get_commit(&entry.commit_id()).unwrap();
    let parents = commit.parents();
    let from_tree = rewrite::merge_commit_trees(repo, &parents);
    let to_tree = commit.tree();
    from_tree.diff(&to_tree, matcher).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(revset_str: &str) -> Result<Rc<RevsetExpression>, RevsetParseErrorKind> {
        // Set up pseudo context to resolve file(path)
        let workspace_ctx = RevsetWorkspaceContext {
            cwd: Path::new("/"),
            workspace_id: &WorkspaceId::default(),
            workspace_root: Path::new("/"),
        };
        // Map error to comparable object
        super::parse(revset_str, Some(&workspace_ctx)).map_err(|e| e.kind)
    }

    #[test]
    fn test_revset_expression_building() {
        let wc_symbol = RevsetExpression::symbol("@".to_string());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        assert_eq!(
            wc_symbol,
            Rc::new(RevsetExpression::Symbol("@".to_string()))
        );
        assert_eq!(
            wc_symbol.heads(),
            Rc::new(RevsetExpression::Heads(wc_symbol.clone()))
        );
        assert_eq!(
            wc_symbol.roots(),
            Rc::new(RevsetExpression::Roots(wc_symbol.clone()))
        );
        assert_eq!(
            wc_symbol.parents(),
            Rc::new(RevsetExpression::Parents(wc_symbol.clone()))
        );
        assert_eq!(
            wc_symbol.ancestors(),
            Rc::new(RevsetExpression::Ancestors(wc_symbol.clone()))
        );
        assert_eq!(
            foo_symbol.children(),
            Rc::new(RevsetExpression::Children(foo_symbol.clone()))
        );
        assert_eq!(
            foo_symbol.descendants(),
            Rc::new(RevsetExpression::DagRange {
                roots: foo_symbol.clone(),
                heads: RevsetExpression::visible_heads(),
            })
        );
        assert_eq!(
            foo_symbol.dag_range_to(&wc_symbol),
            Rc::new(RevsetExpression::DagRange {
                roots: foo_symbol.clone(),
                heads: wc_symbol.clone(),
            })
        );
        assert_eq!(
            foo_symbol.connected(),
            Rc::new(RevsetExpression::DagRange {
                roots: foo_symbol.clone(),
                heads: foo_symbol.clone(),
            })
        );
        assert_eq!(
            foo_symbol.range(&wc_symbol),
            Rc::new(RevsetExpression::Range {
                roots: foo_symbol.clone(),
                heads: wc_symbol.clone()
            })
        );
        assert_eq!(
            foo_symbol.union(&wc_symbol),
            Rc::new(RevsetExpression::Union(
                foo_symbol.clone(),
                wc_symbol.clone()
            ))
        );
        assert_eq!(
            foo_symbol.intersection(&wc_symbol),
            Rc::new(RevsetExpression::Intersection(
                foo_symbol.clone(),
                wc_symbol.clone()
            ))
        );
        assert_eq!(
            foo_symbol.minus(&wc_symbol),
            Rc::new(RevsetExpression::Difference(foo_symbol, wc_symbol.clone()))
        );
    }

    #[test]
    fn test_parse_revset() {
        let wc_symbol = RevsetExpression::symbol("@".to_string());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        let bar_symbol = RevsetExpression::symbol("bar".to_string());
        // Parse a single symbol (specifically the "checkout" symbol)
        assert_eq!(parse("@"), Ok(wc_symbol.clone()));
        // Parse a single symbol
        assert_eq!(parse("foo"), Ok(foo_symbol.clone()));
        // Internal '.', '-', and '+' are allowed
        assert_eq!(
            parse("foo.bar-v1+7"),
            Ok(RevsetExpression::symbol("foo.bar-v1+7".to_string()))
        );
        assert_eq!(
            parse("foo.bar-v1+7-"),
            Ok(RevsetExpression::symbol("foo.bar-v1+7".to_string()).parents())
        );
        // '.' is not allowed at the beginning or end
        assert_eq!(parse(".foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo."), Err(RevsetParseErrorKind::SyntaxError));
        // Multiple '.', '-', '+' are not allowed
        assert_eq!(parse("foo.+bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo--bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo+-bar"), Err(RevsetParseErrorKind::SyntaxError));
        // Parse a parenthesized symbol
        assert_eq!(parse("(foo)"), Ok(foo_symbol.clone()));
        // Parse a quoted symbol
        assert_eq!(parse("\"foo\""), Ok(foo_symbol.clone()));
        // Parse the "parents" operator
        assert_eq!(parse("@-"), Ok(wc_symbol.parents()));
        // Parse the "children" operator
        assert_eq!(parse("@+"), Ok(wc_symbol.children()));
        // Parse the "ancestors" operator
        assert_eq!(parse(":@"), Ok(wc_symbol.ancestors()));
        // Parse the "descendants" operator
        assert_eq!(parse("@:"), Ok(wc_symbol.descendants()));
        // Parse the "dag range" operator
        assert_eq!(parse("foo:bar"), Ok(foo_symbol.dag_range_to(&bar_symbol)));
        // Parse the "range" prefix operator
        assert_eq!(parse("..@"), Ok(wc_symbol.ancestors()));
        assert_eq!(
            parse("@.."),
            Ok(wc_symbol.range(&RevsetExpression::visible_heads()))
        );
        assert_eq!(parse("foo..bar"), Ok(foo_symbol.range(&bar_symbol)));
        // Parse the "intersection" operator
        assert_eq!(parse("foo & bar"), Ok(foo_symbol.intersection(&bar_symbol)));
        // Parse the "union" operator
        assert_eq!(parse("foo | bar"), Ok(foo_symbol.union(&bar_symbol)));
        // Parse the "difference" operator
        assert_eq!(parse("foo ~ bar"), Ok(foo_symbol.minus(&bar_symbol)));
        // Parentheses are allowed before suffix operators
        assert_eq!(parse("(@)-"), Ok(wc_symbol.parents()));
        // Space is allowed around expressions
        assert_eq!(parse(" :@ "), Ok(wc_symbol.ancestors()));
        // Space is not allowed around prefix operators
        assert_eq!(parse(" : @ "), Err(RevsetParseErrorKind::SyntaxError));
        // Incomplete parse
        assert_eq!(parse("foo | -"), Err(RevsetParseErrorKind::SyntaxError));
        // Space is allowed around infix operators and function arguments
        assert_eq!(
            parse("   description(  arg1 ) ~    file(  arg1 ,   arg2 )  ~ heads(  )  "),
            Ok(
                RevsetExpression::filter(RevsetFilterPredicate::Description("arg1".to_string()))
                    .minus(&RevsetExpression::filter(RevsetFilterPredicate::File(
                        vec![
                            RepoPath::from_internal_string("arg1"),
                            RepoPath::from_internal_string("arg2"),
                        ]
                    )))
                    .minus(&RevsetExpression::visible_heads())
            )
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
        // Set operator associativity/precedence
        assert_eq!(parse("x|y|z").unwrap(), parse("(x|y)|z").unwrap());
        assert_eq!(parse("x&y|z").unwrap(), parse("(x&y)|z").unwrap());
        assert_eq!(parse("x|y&z").unwrap(), parse("x|(y&z)").unwrap());
        assert_eq!(parse("x|y~z").unwrap(), parse("x|(y~z)").unwrap());
        // Parse repeated "ancestors"/"descendants"/"dag range"/"range" operators
        assert_eq!(parse(":foo:"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("::foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo::"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo::bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse(":foo:bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo:bar:"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("....foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo...."), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo.....bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("..foo..bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo..bar.."), Err(RevsetParseErrorKind::SyntaxError));
        // Parse combinations of "parents"/"children" operators and the range operators.
        // The former bind more strongly.
        assert_eq!(parse("foo-+"), Ok(foo_symbol.parents().children()));
        assert_eq!(parse("foo-:"), Ok(foo_symbol.parents().descendants()));
        assert_eq!(parse(":foo+"), Ok(foo_symbol.children().ancestors()));
    }

    #[test]
    fn test_parse_revset_function() {
        let wc_symbol = RevsetExpression::symbol("@".to_string());
        assert_eq!(parse("parents(@)"), Ok(wc_symbol.parents()));
        assert_eq!(parse("parents((@))"), Ok(wc_symbol.parents()));
        assert_eq!(parse("parents(\"@\")"), Ok(wc_symbol.parents()));
        assert_eq!(
            parse("ancestors(parents(@))"),
            Ok(wc_symbol.parents().ancestors())
        );
        assert_eq!(parse("parents(@"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(
            parse("parents(@,@)"),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "parents".to_string(),
                message: "Expected 1 argument".to_string()
            })
        );
        assert_eq!(
            parse(r#"description("")"#),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description("".to_string())
            ))
        );
        assert_eq!(
            parse("description(foo)"),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description("foo".to_string())
            ))
        );
        assert_eq!(
            parse("description(heads())"),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "description".to_string(),
                message: "Expected function argument of type string".to_string()
            })
        );
        assert_eq!(
            parse("description((foo))"),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description("foo".to_string())
            ))
        );
        assert_eq!(
            parse("description(\"(foo)\")"),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description("(foo)".to_string())
            ))
        );
        assert_eq!(
            parse("empty()"),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Empty))
        );
        assert!(parse("empty(foo)").is_err());
        assert!(parse("file()").is_err());
        assert_eq!(
            parse("file(foo)"),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::File(vec![
                RepoPath::from_internal_string("foo")
            ])))
        );
        assert_eq!(
            parse("file(foo, bar, baz)"),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::File(vec![
                RepoPath::from_internal_string("foo"),
                RepoPath::from_internal_string("bar"),
                RepoPath::from_internal_string("baz"),
            ])))
        );
    }

    #[test]
    fn test_optimize_subtree() {
        // Check that transform_expression_bottom_up() never rewrites enum variant
        // (e.g. Range -> DagRange) nor reorders arguments unintentionally.

        assert_eq!(
            optimize(parse("parents(branches() & all())").unwrap()),
            RevsetExpression::branches().parents()
        );
        assert_eq!(
            optimize(parse("children(branches() & all())").unwrap()),
            RevsetExpression::branches().children()
        );
        assert_eq!(
            optimize(parse("ancestors(branches() & all())").unwrap()),
            RevsetExpression::branches().ancestors()
        );
        assert_eq!(
            optimize(parse("descendants(branches() & all())").unwrap()),
            RevsetExpression::branches().descendants()
        );

        assert_eq!(
            optimize(parse("(branches() & all())..(all() & tags())").unwrap()),
            RevsetExpression::branches().range(&RevsetExpression::tags())
        );
        assert_eq!(
            optimize(parse("(branches() & all()):(all() & tags())").unwrap()),
            RevsetExpression::branches().dag_range_to(&RevsetExpression::tags())
        );

        assert_eq!(
            optimize(parse("heads(branches() & all())").unwrap()),
            RevsetExpression::branches().heads()
        );
        assert_eq!(
            optimize(parse("roots(branches() & all())").unwrap()),
            RevsetExpression::branches().roots()
        );

        assert_eq!(
            optimize(parse("present(branches() & all())").unwrap()),
            Rc::new(RevsetExpression::Present(RevsetExpression::branches()))
        );

        assert_eq!(
            optimize(parse("(branches() & all()) | (all() & tags())").unwrap()),
            RevsetExpression::branches().union(&RevsetExpression::tags())
        );
        assert_eq!(
            optimize(parse("(branches() & all()) & (all() & tags())").unwrap()),
            RevsetExpression::branches().intersection(&RevsetExpression::tags())
        );
        assert_eq!(
            optimize(parse("(branches() & all()) ~ (all() & tags())").unwrap()),
            RevsetExpression::branches().minus(&RevsetExpression::tags())
        );
    }

    #[test]
    fn test_optimize_unchanged_subtree() {
        fn unwrap_union(
            expression: &RevsetExpression,
        ) -> (&Rc<RevsetExpression>, &Rc<RevsetExpression>) {
            match expression {
                RevsetExpression::Union(left, right) => (left, right),
                _ => panic!("unexpected expression: {expression:?}"),
            }
        }

        // transform_expression_bottom_up() should not recreate tree unnecessarily.
        let parsed = parse("foo-").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(&parsed, &optimized));

        let parsed = parse("branches() | tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(&parsed, &optimized));

        let parsed = parse("branches() & tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(&parsed, &optimized));

        let parsed = parse("branches() ~ tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(&parsed, &optimized));

        // Only left subtree should be rewritten.
        let parsed = parse("(branches() & all()) | tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert_eq!(
            unwrap_union(&optimized).0.as_ref(),
            &RevsetExpression::Branches
        );
        assert!(Rc::ptr_eq(
            unwrap_union(&parsed).1,
            unwrap_union(&optimized).1
        ));

        // Only right subtree should be rewritten.
        let parsed = parse("branches() | (all() & tags())").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(
            unwrap_union(&parsed).0,
            unwrap_union(&optimized).0
        ));
        assert_eq!(unwrap_union(&optimized).1.as_ref(), &RevsetExpression::Tags);
    }

    #[test]
    fn test_optimize_filter_intersection() {
        insta::assert_debug_snapshot!(optimize(parse("author(foo)").unwrap()), @r###"
        Filter {
            candidates: All,
            predicate: Author(
                "foo",
            ),
        }
        "###);

        insta::assert_debug_snapshot!(optimize(parse("foo & description(bar)").unwrap()), @r###"
        Filter {
            candidates: Symbol(
                "foo",
            ),
            predicate: Description(
                "bar",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("author(foo) & bar").unwrap()), @r###"
        Filter {
            candidates: Symbol(
                "bar",
            ),
            predicate: Author(
                "foo",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("author(foo) & committer(bar)").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: All,
                predicate: Author(
                    "foo",
                ),
            },
            predicate: Committer(
                "bar",
            ),
        }
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & author(baz)").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: Symbol(
                    "foo",
                ),
                predicate: Description(
                    "bar",
                ),
            },
            predicate: Author(
                "baz",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("committer(foo) & bar & author(baz)").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: Symbol(
                    "bar",
                ),
                predicate: Committer(
                    "foo",
                ),
            },
            predicate: Author(
                "baz",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("committer(foo) & file(bar) & baz").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: Symbol(
                    "baz",
                ),
                predicate: Committer(
                    "foo",
                ),
            },
            predicate: File(
                [
                    "bar",
                ],
            ),
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("committer(foo) & file(bar) & author(baz)").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: Filter {
                    candidates: All,
                    predicate: Committer(
                        "foo",
                    ),
                },
                predicate: File(
                    [
                        "bar",
                    ],
                ),
            },
            predicate: Author(
                "baz",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo & file(bar) & baz").unwrap()), @r###"
        Filter {
            candidates: Intersection(
                Symbol(
                    "foo",
                ),
                Symbol(
                    "baz",
                ),
            ),
            predicate: File(
                [
                    "bar",
                ],
            ),
        }
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & author(baz) & qux").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: Intersection(
                    Symbol(
                        "foo",
                    ),
                    Symbol(
                        "qux",
                    ),
                ),
                predicate: Description(
                    "bar",
                ),
            },
            predicate: Author(
                "baz",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & parents(author(baz)) & qux").unwrap()), @r###"
        Filter {
            candidates: Intersection(
                Intersection(
                    Symbol(
                        "foo",
                    ),
                    Parents(
                        Filter {
                            candidates: All,
                            predicate: Author(
                                "baz",
                            ),
                        },
                    ),
                ),
                Symbol(
                    "qux",
                ),
            ),
            predicate: Description(
                "bar",
            ),
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & parents(author(baz) & qux)").unwrap()), @r###"
        Filter {
            candidates: Intersection(
                Symbol(
                    "foo",
                ),
                Parents(
                    Filter {
                        candidates: Symbol(
                            "qux",
                        ),
                        predicate: Author(
                            "baz",
                        ),
                    },
                ),
            ),
            predicate: Description(
                "bar",
            ),
        }
        "###);

        // Symbols have to be pushed down to the innermost filter node.
        insta::assert_debug_snapshot!(
            optimize(parse("(a & author(A)) & (b & author(B)) & (c & author(C))").unwrap()), @r###"
        Filter {
            candidates: Filter {
                candidates: Filter {
                    candidates: Intersection(
                        Intersection(
                            Symbol(
                                "a",
                            ),
                            Symbol(
                                "b",
                            ),
                        ),
                        Symbol(
                            "c",
                        ),
                    ),
                    predicate: Author(
                        "A",
                    ),
                },
                predicate: Author(
                    "B",
                ),
            },
            predicate: Author(
                "C",
            ),
        }
        "###);

        // 'all()' moves in to 'filter()' first, so 'A & filter()' can be found.
        insta::assert_debug_snapshot!(
            optimize(parse("foo & (all() & description(bar)) & (author(baz) & all())").unwrap()),
            @r###"
        Filter {
            candidates: Filter {
                candidates: Symbol(
                    "foo",
                ),
                predicate: Description(
                    "bar",
                ),
            },
            predicate: Author(
                "baz",
            ),
        }
        "###);
    }
}
