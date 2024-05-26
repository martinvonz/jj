// Copyright 2021 The Jujutsu Authors
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

use std::any::Any;
use std::collections::{hash_map, HashMap};
use std::convert::Infallible;
use std::ops::Range;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::{error, fmt};

use itertools::Itertools;
use once_cell::sync::Lazy;
use thiserror::Error;

use crate::backend::{BackendError, BackendResult, ChangeId, CommitId};
use crate::commit::Commit;
use crate::dsl_util::{collect_similar, AliasExpandError as _};
use crate::fileset::{FilePattern, FilesetExpression};
use crate::graph::GraphEdge;
use crate::hex_util::to_forward_hex;
use crate::id_prefix::IdPrefixContext;
use crate::object_id::{HexPrefix, PrefixResolution};
use crate::op_store::WorkspaceId;
use crate::repo::Repo;
use crate::repo_path::RepoPathUiConverter;
pub use crate::revset_parser::{
    BinaryOp, ExpressionKind, ExpressionNode, FunctionCallNode, RevsetAliasesMap, RevsetParseError,
    RevsetParseErrorKind, UnaryOp,
};
use crate::store::Store;
use crate::str_util::StringPattern;
use crate::{dsl_util, git, revset_parser};

/// Error occurred during symbol resolution.
#[derive(Debug, Error)]
pub enum RevsetResolutionError {
    #[error("Revision \"{name}\" doesn't exist")]
    NoSuchRevision {
        name: String,
        candidates: Vec<String>,
    },
    #[error("Workspace \"{name}\" doesn't have a working copy")]
    WorkspaceMissingWorkingCopy { name: String },
    #[error("An empty string is not a valid revision")]
    EmptyString,
    #[error("Commit ID prefix \"{0}\" is ambiguous")]
    AmbiguousCommitIdPrefix(String),
    #[error("Change ID prefix \"{0}\" is ambiguous")]
    AmbiguousChangeIdPrefix(String),
    #[error("Unexpected error from store")]
    StoreError(#[source] BackendError),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Error occurred during revset evaluation.
#[derive(Debug, Error)]
pub enum RevsetEvaluationError {
    #[error("Unexpected error from store")]
    StoreError(#[source] BackendError),
    #[error("{0}")]
    Other(String),
}

// assumes index has less than u64::MAX entries.
pub const GENERATION_RANGE_FULL: Range<u64> = 0..u64::MAX;
pub const GENERATION_RANGE_EMPTY: Range<u64> = 0..0;

/// Global flag applied to the entire expression.
///
/// The core revset engine doesn't use this value. It's up to caller to
/// interpret it to change the evaluation behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevsetModifier {
    /// Expression can be evaluated to multiple revisions even if a single
    /// revision is expected by default.
    All,
}

/// Symbol or function to be resolved to `CommitId`s.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevsetCommitRef {
    WorkingCopy(WorkspaceId),
    WorkingCopies,
    Symbol(String),
    RemoteSymbol {
        name: String,
        remote: String,
    },
    VisibleHeads,
    Root,
    Branches(StringPattern),
    RemoteBranches {
        branch_pattern: StringPattern,
        remote_pattern: StringPattern,
    },
    Tags,
    GitRefs,
    GitHead,
}

/// A custom revset filter expression, defined by an extension.
pub trait RevsetFilterExtension: std::fmt::Debug + Any {
    fn as_any(&self) -> &dyn Any;

    /// Returns true iff this filter matches the specified commit.
    fn matches_commit(&self, commit: &Commit) -> bool;
}

// TODO: Refactor tests to not need the Eq trait so we can remove this wrapper.
#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct RevsetFilterExtensionWrapper(pub Rc<dyn RevsetFilterExtension>);

impl PartialEq for RevsetFilterExtensionWrapper {
    fn eq(&self, other: &RevsetFilterExtensionWrapper) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RevsetFilterExtensionWrapper {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevsetFilterPredicate {
    /// Commits with number of parents in the range.
    ParentCount(Range<u32>),
    /// Commits with description containing the needle.
    Description(StringPattern),
    /// Commits with author's name or email containing the needle.
    Author(StringPattern),
    /// Commits with committer's name or email containing the needle.
    Committer(StringPattern),
    /// Commits modifying the paths specified by the fileset.
    File(FilesetExpression),
    /// Commits with conflicts
    HasConflict,
    /// Custom predicates provided by extensions
    Extension(RevsetFilterExtensionWrapper),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RevsetExpression {
    None,
    All,
    Commits(Vec<CommitId>),
    CommitRef(RevsetCommitRef),
    // TODO: This shouldn't be of RevsetExpression type. Maybe better to
    // introduce an intermediate AST tree where aliases will be substituted.
    StringPattern {
        kind: String,
        value: String,
    },
    Ancestors {
        heads: Rc<RevsetExpression>,
        generation: Range<u64>,
    },
    Descendants {
        roots: Rc<RevsetExpression>,
        generation: Range<u64>,
    },
    // Commits that are ancestors of "heads" but not ancestors of "roots"
    Range {
        roots: Rc<RevsetExpression>,
        heads: Rc<RevsetExpression>,
        generation: Range<u64>,
    },
    // Commits that are descendants of "roots" and ancestors of "heads"
    DagRange {
        roots: Rc<RevsetExpression>,
        heads: Rc<RevsetExpression>,
        // TODO: maybe add generation_from_roots/heads?
    },
    // Commits reachable from "sources" within "domain"
    Reachable {
        sources: Rc<RevsetExpression>,
        domain: Rc<RevsetExpression>,
    },
    Heads(Rc<RevsetExpression>),
    Roots(Rc<RevsetExpression>),
    Latest {
        candidates: Rc<RevsetExpression>,
        count: usize,
    },
    Filter(RevsetFilterPredicate),
    /// Marker for subtree that should be intersected as filter.
    AsFilter(Rc<RevsetExpression>),
    Present(Rc<RevsetExpression>),
    NotIn(Rc<RevsetExpression>),
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

    pub fn working_copy(workspace_id: WorkspaceId) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::WorkingCopy(
            workspace_id,
        )))
    }

    pub fn working_copies() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::WorkingCopies))
    }

    pub fn symbol(value: String) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::Symbol(value)))
    }

    pub fn remote_symbol(name: String, remote: String) -> Rc<RevsetExpression> {
        let commit_ref = RevsetCommitRef::RemoteSymbol { name, remote };
        Rc::new(RevsetExpression::CommitRef(commit_ref))
    }

    pub fn commit(commit_id: CommitId) -> Rc<RevsetExpression> {
        RevsetExpression::commits(vec![commit_id])
    }

    pub fn commits(commit_ids: Vec<CommitId>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Commits(commit_ids))
    }

    pub fn visible_heads() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::VisibleHeads))
    }

    pub fn root() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::Root))
    }

    pub fn branches(pattern: StringPattern) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::Branches(
            pattern,
        )))
    }

    pub fn remote_branches(
        branch_pattern: StringPattern,
        remote_pattern: StringPattern,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(
            RevsetCommitRef::RemoteBranches {
                branch_pattern,
                remote_pattern,
            },
        ))
    }

    pub fn tags() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::Tags))
    }

    pub fn git_refs() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::GitRefs))
    }

    pub fn git_head() -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::GitHead))
    }

    pub fn latest(self: &Rc<RevsetExpression>, count: usize) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Latest {
            candidates: self.clone(),
            count,
        })
    }

    pub fn filter(predicate: RevsetFilterPredicate) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Filter(predicate))
    }

    /// Find any empty commits.
    pub fn is_empty() -> Rc<RevsetExpression> {
        Self::filter(RevsetFilterPredicate::File(FilesetExpression::all())).negated()
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
        Rc::new(RevsetExpression::Ancestors {
            heads: self.clone(),
            generation: 1..2,
        })
    }

    /// Ancestors of `self`, including `self`.
    pub fn ancestors(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        self.ancestors_range(GENERATION_RANGE_FULL)
    }

    /// Ancestors of `self` at an offset of `generation` behind `self`.
    /// The `generation` offset is zero-based starting from `self`.
    pub fn ancestors_at(self: &Rc<RevsetExpression>, generation: u64) -> Rc<RevsetExpression> {
        self.ancestors_range(generation..(generation + 1))
    }

    /// Ancestors of `self` in the given range.
    pub fn ancestors_range(
        self: &Rc<RevsetExpression>,
        generation_range: Range<u64>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Ancestors {
            heads: self.clone(),
            generation: generation_range,
        })
    }

    /// Children of `self`.
    pub fn children(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Descendants {
            roots: self.clone(),
            generation: 1..2,
        })
    }

    /// Descendants of `self`, including `self`.
    pub fn descendants(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Descendants {
            roots: self.clone(),
            generation: GENERATION_RANGE_FULL,
        })
    }

    /// Descendants of `self` at an offset of `generation` ahead of `self`.
    /// The `generation` offset is zero-based starting from `self`.
    pub fn descendants_at(self: &Rc<RevsetExpression>, generation: u64) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Descendants {
            roots: self.clone(),
            generation: generation..(generation + 1),
        })
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

    /// All commits within `domain` reachable from this set of commits, by
    /// traversing either parent or child edges.
    pub fn reachable(
        self: &Rc<RevsetExpression>,
        domain: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Reachable {
            sources: self.clone(),
            domain: domain.clone(),
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
            generation: GENERATION_RANGE_FULL,
        })
    }

    /// Commits that are not in `self`, i.e. the complement of `self`.
    pub fn negated(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::NotIn(self.clone()))
    }

    /// Commits that are in `self` or in `other` (or both).
    pub fn union(
        self: &Rc<RevsetExpression>,
        other: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Union(self.clone(), other.clone()))
    }

    /// Commits that are in any of the `expressions`.
    pub fn union_all(expressions: &[Rc<RevsetExpression>]) -> Rc<RevsetExpression> {
        match expressions {
            [] => Self::none(),
            [expression] => expression.clone(),
            _ => {
                // Build balanced tree to minimize the recursion depth.
                let (left, right) = expressions.split_at(expressions.len() / 2);
                Self::union(&Self::union_all(left), &Self::union_all(right))
            }
        }
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

    /// Resolve a programmatically created revset expression. In particular, the
    /// expression must not contain any symbols (branches, tags, change/commit
    /// prefixes). Callers must not include `RevsetExpression::symbol()` in
    /// the expression, and should instead resolve symbols to `CommitId`s and
    /// pass them into `RevsetExpression::commits()`. Similarly, the expression
    /// must not contain any `RevsetExpression::remote_symbol()` or
    /// `RevsetExpression::working_copy()`, unless they're known to be valid.
    pub fn resolve_programmatic(self: Rc<Self>, repo: &dyn Repo) -> ResolvedExpression {
        let symbol_resolver = FailingSymbolResolver;
        resolve_symbols(repo, self, &symbol_resolver)
            .map(|expression| resolve_visibility(repo, &expression))
            .unwrap()
    }

    /// Resolve a user-provided expression. Symbols will be resolved using the
    /// provided `SymbolResolver`.
    pub fn resolve_user_expression(
        self: Rc<Self>,
        repo: &dyn Repo,
        symbol_resolver: &dyn SymbolResolver,
    ) -> Result<ResolvedExpression, RevsetResolutionError> {
        resolve_symbols(repo, self, symbol_resolver)
            .map(|expression| resolve_visibility(repo, &expression))
    }

    /// Resolve a programmatically created revset expression and evaluate it in
    /// the repo.
    pub fn evaluate_programmatic<'index>(
        self: Rc<Self>,
        repo: &'index dyn Repo,
    ) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError> {
        optimize(self).resolve_programmatic(repo).evaluate(repo)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedPredicateExpression {
    /// Pure filter predicate.
    Filter(RevsetFilterPredicate),
    /// Set expression to be evaluated as filter. This is typically a subtree
    /// node of `Union` with a pure filter predicate.
    Set(Box<ResolvedExpression>),
    NotIn(Box<ResolvedPredicateExpression>),
    Union(
        Box<ResolvedPredicateExpression>,
        Box<ResolvedPredicateExpression>,
    ),
}

/// Describes evaluation plan of revset expression.
///
/// Unlike `RevsetExpression`, this doesn't contain unresolved symbols or `View`
/// properties.
///
/// Use `RevsetExpression` API to build a query programmatically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedExpression {
    Commits(Vec<CommitId>),
    Ancestors {
        heads: Box<ResolvedExpression>,
        generation: Range<u64>,
    },
    /// Commits that are ancestors of `heads` but not ancestors of `roots`.
    Range {
        roots: Box<ResolvedExpression>,
        heads: Box<ResolvedExpression>,
        generation: Range<u64>,
    },
    /// Commits that are descendants of `roots` and ancestors of `heads`.
    DagRange {
        roots: Box<ResolvedExpression>,
        heads: Box<ResolvedExpression>,
        generation_from_roots: Range<u64>,
    },
    /// Commits reachable from `sources` within `domain`.
    Reachable {
        sources: Box<ResolvedExpression>,
        domain: Box<ResolvedExpression>,
    },
    Heads(Box<ResolvedExpression>),
    Roots(Box<ResolvedExpression>),
    Latest {
        candidates: Box<ResolvedExpression>,
        count: usize,
    },
    Union(Box<ResolvedExpression>, Box<ResolvedExpression>),
    /// Intersects `candidates` with `predicate` by filtering.
    FilterWithin {
        candidates: Box<ResolvedExpression>,
        predicate: ResolvedPredicateExpression,
    },
    /// Intersects expressions by merging.
    Intersection(Box<ResolvedExpression>, Box<ResolvedExpression>),
    Difference(Box<ResolvedExpression>, Box<ResolvedExpression>),
}

impl ResolvedExpression {
    pub fn evaluate<'index>(
        &self,
        repo: &'index dyn Repo,
    ) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError> {
        repo.index().evaluate_revset(self, repo.store())
    }
}

// TODO: maybe replace with RevsetParseContext?
#[derive(Clone, Copy, Debug)]
pub struct ParseState<'a> {
    function_map: &'a HashMap<&'static str, RevsetFunction>,
    user_email: &'a str,
    workspace_ctx: &'a Option<RevsetWorkspaceContext<'a>>,
}

impl<'a> ParseState<'a> {
    fn new(context: &'a RevsetParseContext) -> Self {
        ParseState {
            function_map: &context.extensions.function_map,
            user_email: &context.user_email,
            workspace_ctx: &context.workspace,
        }
    }
}

pub type RevsetFunction =
    fn(&FunctionCallNode, ParseState) -> Result<Rc<RevsetExpression>, RevsetParseError>;

static BUILTIN_FUNCTION_MAP: Lazy<HashMap<&'static str, RevsetFunction>> = Lazy::new(|| {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map: HashMap<&'static str, RevsetFunction> = HashMap::new();
    map.insert("parents", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = parse_expression_rule(arg, state)?;
        Ok(expression.parents())
    });
    map.insert("children", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = parse_expression_rule(arg, state)?;
        Ok(expression.children())
    });
    map.insert("ancestors", |function, state| {
        let ([heads_arg], [depth_opt_arg]) = function.expect_arguments()?;
        let heads = parse_expression_rule(heads_arg, state)?;
        let generation = if let Some(depth_arg) = depth_opt_arg {
            let depth = parse_function_argument_as_literal("integer", function.name, depth_arg)?;
            0..depth
        } else {
            GENERATION_RANGE_FULL
        };
        Ok(heads.ancestors_range(generation))
    });
    map.insert("descendants", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = parse_expression_rule(arg, state)?;
        Ok(expression.descendants())
    });
    map.insert("connected", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let candidates = parse_expression_rule(arg, state)?;
        Ok(candidates.connected())
    });
    map.insert("reachable", |function, state| {
        let [source_arg, domain_arg] = function.expect_exact_arguments()?;
        let sources = parse_expression_rule(source_arg, state)?;
        let domain = parse_expression_rule(domain_arg, state)?;
        Ok(sources.reachable(&domain))
    });
    map.insert("none", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::none())
    });
    map.insert("all", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::all())
    });
    map.insert("working_copies", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::working_copies())
    });
    map.insert("heads", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let candidates = parse_expression_rule(arg, state)?;
        Ok(candidates.heads())
    });
    map.insert("roots", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let candidates = parse_expression_rule(arg, state)?;
        Ok(candidates.roots())
    });
    map.insert("visible_heads", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::visible_heads())
    });
    map.insert("root", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::root())
    });
    map.insert("branches", |function, _state| {
        let ([], [opt_arg]) = function.expect_arguments()?;
        let pattern = if let Some(arg) = opt_arg {
            parse_function_argument_to_string_pattern(function.name, arg)?
        } else {
            StringPattern::everything()
        };
        Ok(RevsetExpression::branches(pattern))
    });
    map.insert("remote_branches", |function, _state| {
        let ([], [branch_opt_arg, remote_opt_arg]) =
            function.expect_named_arguments(&["", "remote"])?;
        let branch_pattern = if let Some(branch_arg) = branch_opt_arg {
            parse_function_argument_to_string_pattern(function.name, branch_arg)?
        } else {
            StringPattern::everything()
        };
        let remote_pattern = if let Some(remote_arg) = remote_opt_arg {
            parse_function_argument_to_string_pattern(function.name, remote_arg)?
        } else {
            StringPattern::everything()
        };
        Ok(RevsetExpression::remote_branches(
            branch_pattern,
            remote_pattern,
        ))
    });
    map.insert("tags", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::tags())
    });
    map.insert("git_refs", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::git_refs())
    });
    map.insert("git_head", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::git_head())
    });
    map.insert("latest", |function, state| {
        let ([candidates_arg], [count_opt_arg]) = function.expect_arguments()?;
        let candidates = parse_expression_rule(candidates_arg, state)?;
        let count = if let Some(count_arg) = count_opt_arg {
            parse_function_argument_as_literal("integer", function.name, count_arg)?
        } else {
            1
        };
        Ok(candidates.latest(count))
    });
    map.insert("merges", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::filter(
            RevsetFilterPredicate::ParentCount(2..u32::MAX),
        ))
    });
    map.insert("description", |function, _state| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = parse_function_argument_to_string_pattern(function.name, arg)?;
        Ok(RevsetExpression::filter(
            RevsetFilterPredicate::Description(pattern),
        ))
    });
    map.insert("author", |function, _state| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = parse_function_argument_to_string_pattern(function.name, arg)?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
            pattern,
        )))
    });
    map.insert("mine", |function, state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
            StringPattern::Exact(state.user_email.to_owned()),
        )))
    });
    map.insert("committer", |function, _state| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = parse_function_argument_to_string_pattern(function.name, arg)?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::Committer(
            pattern,
        )))
    });
    map.insert("empty", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::is_empty())
    });
    map.insert("file", |function, state| {
        if let Some(ctx) = state.workspace_ctx {
            let ([arg], args) = function.expect_some_arguments()?;
            let file_expressions = itertools::chain([arg], args)
                .map(|arg| {
                    parse_function_argument_to_file_pattern(function.name, arg, ctx.path_converter)
                })
                .map_ok(FilesetExpression::pattern)
                .try_collect()?;
            let expr = FilesetExpression::union_all(file_expressions);
            Ok(RevsetExpression::filter(RevsetFilterPredicate::File(expr)))
        } else {
            Err(RevsetParseError::with_span(
                RevsetParseErrorKind::FsPathWithoutWorkspace,
                function.args_span, // TODO: better to use name_span?
            ))
        }
    });
    map.insert("conflict", |function, _state| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::HasConflict))
    });
    map.insert("present", |function, state| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = parse_expression_rule(arg, state)?;
        Ok(Rc::new(RevsetExpression::Present(expression)))
    });
    map
});

// TODO: rename function to expect_file_pattern()?
pub fn parse_function_argument_to_file_pattern(
    name: &str,
    node: &ExpressionNode,
    path_converter: &RepoPathUiConverter,
) -> Result<FilePattern, RevsetParseError> {
    let parse_pattern = |value: &str, kind: Option<&str>| match kind {
        Some(kind) => FilePattern::from_str_kind(path_converter, value, kind),
        None => FilePattern::cwd_prefix_path(path_converter, value),
    };
    parse_function_argument_as_pattern("file pattern", name, node, parse_pattern)
}

// TODO: rename function to expect_string_pattern()?
pub fn parse_function_argument_to_string_pattern(
    name: &str,
    node: &ExpressionNode,
) -> Result<StringPattern, RevsetParseError> {
    let parse_pattern = |value: &str, kind: Option<&str>| match kind {
        Some(kind) => StringPattern::from_str_kind(value, kind),
        None => Ok(StringPattern::Substring(value.to_owned())),
    };
    parse_function_argument_as_pattern("string pattern", name, node, parse_pattern)
}

// TODO: move to revset_parser module?
// TODO: rename function to expect_pattern_with()?
fn parse_function_argument_as_pattern<T, E: Into<Box<dyn error::Error + Send + Sync>>>(
    type_name: &str,
    function_name: &str,
    node: &ExpressionNode,
    parse_pattern: impl FnOnce(&str, Option<&str>) -> Result<T, E>,
) -> Result<T, RevsetParseError> {
    let wrap_error = |err: E| {
        RevsetParseError::invalid_arguments(
            function_name,
            format!("Invalid {type_name}"),
            node.span,
        )
        .with_source(err)
    };
    revset_parser::expect_literal_with(node, |node| match &node.kind {
        ExpressionKind::Identifier(name) => parse_pattern(name, None).map_err(wrap_error),
        ExpressionKind::String(name) => parse_pattern(name, None).map_err(wrap_error),
        ExpressionKind::StringPattern { kind, value } => {
            parse_pattern(value, Some(kind)).map_err(wrap_error)
        }
        _ => Err(RevsetParseError::invalid_arguments(
            function_name,
            format!("Expected function argument of {type_name}"),
            node.span,
        )),
    })
}

// TODO: move to revset_parser module?
// TODO: rename function to something like expect_literal()?
pub fn parse_function_argument_as_literal<T: FromStr>(
    type_name: &str,
    name: &str,
    node: &ExpressionNode,
) -> Result<T, RevsetParseError> {
    let make_error = || {
        RevsetParseError::invalid_arguments(
            name,
            format!("Expected function argument of type {type_name}"),
            node.span,
        )
    };
    revset_parser::expect_literal_with(node, |node| match &node.kind {
        ExpressionKind::Identifier(name) => name.parse().map_err(|_| make_error()),
        ExpressionKind::String(name) => name.parse().map_err(|_| make_error()),
        _ => Err(make_error()),
    })
}

/// Resolves function call by using the given function map.
fn lower_function_call(
    function: &FunctionCallNode,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    if let Some(func) = state.function_map.get(function.name) {
        func(function, state)
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::NoSuchFunction {
                name: function.name.to_owned(),
                candidates: collect_similar(function.name, state.function_map.keys()),
            },
            function.name_span,
        ))
    }
}

// TODO: rename function to lower_expression()?:
pub fn parse_expression_rule(
    node: &ExpressionNode,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    match &node.kind {
        ExpressionKind::Identifier(name) => Ok(RevsetExpression::symbol((*name).to_owned())),
        ExpressionKind::String(name) => Ok(RevsetExpression::symbol(name.to_owned())),
        ExpressionKind::StringPattern { .. } => Err(RevsetParseError::with_span(
            RevsetParseErrorKind::NotInfixOperator {
                op: ":".to_owned(),
                similar_op: "::".to_owned(),
                description: "DAG range".to_owned(),
            },
            node.span,
        )),
        ExpressionKind::RemoteSymbol { name, remote } => Ok(RevsetExpression::remote_symbol(
            name.to_owned(),
            remote.to_owned(),
        )),
        ExpressionKind::AtWorkspace(name) => Ok(RevsetExpression::working_copy(WorkspaceId::new(
            name.to_owned(),
        ))),
        ExpressionKind::AtCurrentWorkspace => {
            let ctx = state.workspace_ctx.as_ref().ok_or_else(|| {
                RevsetParseError::with_span(
                    RevsetParseErrorKind::WorkingCopyWithoutWorkspace,
                    node.span,
                )
            })?;
            Ok(RevsetExpression::working_copy(ctx.workspace_id.clone()))
        }
        ExpressionKind::DagRangeAll => Ok(RevsetExpression::all()),
        ExpressionKind::RangeAll => {
            Ok(RevsetExpression::root().range(&RevsetExpression::visible_heads()))
        }
        ExpressionKind::Unary(op, arg_node) => {
            let arg = parse_expression_rule(arg_node, state)?;
            match op {
                UnaryOp::Negate => Ok(arg.negated()),
                UnaryOp::DagRangePre => Ok(arg.ancestors()),
                UnaryOp::DagRangePost => Ok(arg.descendants()),
                UnaryOp::RangePre => Ok(RevsetExpression::root().range(&arg)),
                UnaryOp::RangePost => Ok(arg.range(&RevsetExpression::visible_heads())),
                UnaryOp::Parents => Ok(arg.parents()),
                UnaryOp::Children => Ok(arg.children()),
            }
        }
        ExpressionKind::Binary(op, lhs_node, rhs_node) => {
            let lhs = parse_expression_rule(lhs_node, state)?;
            let rhs = parse_expression_rule(rhs_node, state)?;
            match op {
                BinaryOp::Union => Ok(lhs.union(&rhs)),
                BinaryOp::Intersection => Ok(lhs.intersection(&rhs)),
                BinaryOp::Difference => Ok(lhs.minus(&rhs)),
                BinaryOp::DagRange => Ok(lhs.dag_range_to(&rhs)),
                BinaryOp::Range => Ok(lhs.range(&rhs)),
            }
        }
        ExpressionKind::FunctionCall(function) => lower_function_call(function, state),
        ExpressionKind::AliasExpanded(id, subst) => parse_expression_rule(subst, state)
            .map_err(|e| e.within_alias_expansion(*id, node.span)),
    }
}

pub fn parse(
    revset_str: &str,
    context: &RevsetParseContext,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let node = revset_parser::parse_program(revset_str)?;
    let node = dsl_util::expand_aliases(node, context.aliases_map)?;
    let state = ParseState::new(context);
    parse_expression_rule(&node, state)
        .map_err(|err| err.extend_function_candidates(context.aliases_map.function_names()))
}

pub fn parse_with_modifier(
    revset_str: &str,
    context: &RevsetParseContext,
) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseError> {
    let (node, modifier) = revset_parser::parse_program_with_modifier(revset_str)?;
    let node = dsl_util::expand_aliases(node, context.aliases_map)?;
    let state = ParseState::new(context);
    let expression = parse_expression_rule(&node, state)
        .map_err(|err| err.extend_function_candidates(context.aliases_map.function_names()))?;
    Ok((expression, modifier))
}

/// `Some` for rewritten expression, or `None` to reuse the original expression.
type TransformedExpression = Option<Rc<RevsetExpression>>;

/// Walks `expression` tree and applies `f` recursively from leaf nodes.
fn transform_expression_bottom_up(
    expression: &Rc<RevsetExpression>,
    mut f: impl FnMut(&Rc<RevsetExpression>) -> TransformedExpression,
) -> TransformedExpression {
    try_transform_expression::<Infallible>(expression, |_| Ok(None), |expression| Ok(f(expression)))
        .unwrap()
}

/// Walks `expression` tree and applies transformation recursively.
///
/// `pre` is the callback to rewrite subtree including children. It is
/// invoked before visiting the child nodes. If returned `Some`, children
/// won't be visited.
///
/// `post` is the callback to rewrite from leaf nodes. If returned `None`,
/// the original expression node will be reused.
///
/// If no nodes rewritten, this function returns `None`.
/// `std::iter::successors()` could be used if the transformation needs to be
/// applied repeatedly until converged.
fn try_transform_expression<E>(
    expression: &Rc<RevsetExpression>,
    mut pre: impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
    mut post: impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
) -> Result<TransformedExpression, E> {
    fn transform_child_rec<E>(
        expression: &Rc<RevsetExpression>,
        pre: &mut impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
        post: &mut impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
    ) -> Result<TransformedExpression, E> {
        Ok(match expression.as_ref() {
            RevsetExpression::None => None,
            RevsetExpression::All => None,
            RevsetExpression::Commits(_) => None,
            RevsetExpression::CommitRef(_) => None,
            RevsetExpression::StringPattern { .. } => None,
            RevsetExpression::Ancestors { heads, generation } => transform_rec(heads, pre, post)?
                .map(|heads| RevsetExpression::Ancestors {
                    heads,
                    generation: generation.clone(),
                }),
            RevsetExpression::Descendants { roots, generation } => transform_rec(roots, pre, post)?
                .map(|roots| RevsetExpression::Descendants {
                    roots,
                    generation: generation.clone(),
                }),
            RevsetExpression::Range {
                roots,
                heads,
                generation,
            } => transform_rec_pair((roots, heads), pre, post)?.map(|(roots, heads)| {
                RevsetExpression::Range {
                    roots,
                    heads,
                    generation: generation.clone(),
                }
            }),
            RevsetExpression::DagRange { roots, heads } => {
                transform_rec_pair((roots, heads), pre, post)?
                    .map(|(roots, heads)| RevsetExpression::DagRange { roots, heads })
            }
            RevsetExpression::Reachable { sources, domain } => {
                transform_rec_pair((sources, domain), pre, post)?
                    .map(|(sources, domain)| RevsetExpression::Reachable { sources, domain })
            }
            RevsetExpression::Heads(candidates) => {
                transform_rec(candidates, pre, post)?.map(RevsetExpression::Heads)
            }
            RevsetExpression::Roots(candidates) => {
                transform_rec(candidates, pre, post)?.map(RevsetExpression::Roots)
            }
            RevsetExpression::Latest { candidates, count } => transform_rec(candidates, pre, post)?
                .map(|candidates| RevsetExpression::Latest {
                    candidates,
                    count: *count,
                }),
            RevsetExpression::Filter(_) => None,
            RevsetExpression::AsFilter(candidates) => {
                transform_rec(candidates, pre, post)?.map(RevsetExpression::AsFilter)
            }
            RevsetExpression::Present(candidates) => {
                transform_rec(candidates, pre, post)?.map(RevsetExpression::Present)
            }
            RevsetExpression::NotIn(complement) => {
                transform_rec(complement, pre, post)?.map(RevsetExpression::NotIn)
            }
            RevsetExpression::Union(expression1, expression2) => {
                transform_rec_pair((expression1, expression2), pre, post)?.map(
                    |(expression1, expression2)| RevsetExpression::Union(expression1, expression2),
                )
            }
            RevsetExpression::Intersection(expression1, expression2) => {
                transform_rec_pair((expression1, expression2), pre, post)?.map(
                    |(expression1, expression2)| {
                        RevsetExpression::Intersection(expression1, expression2)
                    },
                )
            }
            RevsetExpression::Difference(expression1, expression2) => {
                transform_rec_pair((expression1, expression2), pre, post)?.map(
                    |(expression1, expression2)| {
                        RevsetExpression::Difference(expression1, expression2)
                    },
                )
            }
        }
        .map(Rc::new))
    }

    #[allow(clippy::type_complexity)]
    fn transform_rec_pair<E>(
        (expression1, expression2): (&Rc<RevsetExpression>, &Rc<RevsetExpression>),
        pre: &mut impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
        post: &mut impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
    ) -> Result<Option<(Rc<RevsetExpression>, Rc<RevsetExpression>)>, E> {
        match (
            transform_rec(expression1, pre, post)?,
            transform_rec(expression2, pre, post)?,
        ) {
            (Some(new_expression1), Some(new_expression2)) => {
                Ok(Some((new_expression1, new_expression2)))
            }
            (Some(new_expression1), None) => Ok(Some((new_expression1, expression2.clone()))),
            (None, Some(new_expression2)) => Ok(Some((expression1.clone(), new_expression2))),
            (None, None) => Ok(None),
        }
    }

    fn transform_rec<E>(
        expression: &Rc<RevsetExpression>,
        pre: &mut impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
        post: &mut impl FnMut(&Rc<RevsetExpression>) -> Result<TransformedExpression, E>,
    ) -> Result<TransformedExpression, E> {
        if let Some(new_expression) = pre(expression)? {
            return Ok(Some(new_expression));
        }
        if let Some(new_expression) = transform_child_rec(expression, pre, post)? {
            // must propagate new expression tree
            Ok(Some(post(&new_expression)?.unwrap_or(new_expression)))
        } else {
            post(expression)
        }
    }

    transform_rec(expression, &mut pre, &mut post)
}

/// Transforms filter expressions, by applying the following rules.
///
/// a. Moves as many sets to left of filter intersection as possible, to
///    minimize the filter inputs.
/// b. TODO: Rewrites set operations to and/or/not of predicates, to
///    help further optimization (e.g. combine `file(_)` matchers.)
/// c. Wraps union of filter and set (e.g. `author(_) | heads()`), to
///    ensure inner filter wouldn't need to evaluate all the input sets.
fn internalize_filter(expression: &Rc<RevsetExpression>) -> TransformedExpression {
    fn is_filter(expression: &RevsetExpression) -> bool {
        matches!(
            expression,
            RevsetExpression::Filter(_) | RevsetExpression::AsFilter(_)
        )
    }

    fn is_filter_tree(expression: &RevsetExpression) -> bool {
        is_filter(expression) || as_filter_intersection(expression).is_some()
    }

    // Extracts 'c & f' from intersect_down()-ed node.
    fn as_filter_intersection(
        expression: &RevsetExpression,
    ) -> Option<(&Rc<RevsetExpression>, &Rc<RevsetExpression>)> {
        if let RevsetExpression::Intersection(expression1, expression2) = expression {
            is_filter(expression2).then_some((expression1, expression2))
        } else {
            None
        }
    }

    // Since both sides must have already been intersect_down()-ed, we don't need to
    // apply the whole bottom-up pass to new intersection node. Instead, just push
    // new 'c & (d & g)' down-left to '(c & d) & g' while either side is
    // an intersection of filter node.
    fn intersect_down(
        expression1: &Rc<RevsetExpression>,
        expression2: &Rc<RevsetExpression>,
    ) -> TransformedExpression {
        let recurse = |e1, e2| intersect_down(e1, e2).unwrap_or_else(|| e1.intersection(e2));
        match (expression1.as_ref(), expression2.as_ref()) {
            // Don't reorder 'f1 & f2'
            (_, e2) if is_filter(e2) => None,
            // f1 & e2 -> e2 & f1
            (e1, _) if is_filter(e1) => Some(expression2.intersection(expression1)),
            (e1, e2) => match (as_filter_intersection(e1), as_filter_intersection(e2)) {
                // e1 & (c2 & f2) -> (e1 & c2) & f2
                // (c1 & f1) & (c2 & f2) -> ((c1 & f1) & c2) & f2 -> ((c1 & c2) & f1) & f2
                (_, Some((c2, f2))) => Some(recurse(expression1, c2).intersection(f2)),
                // (c1 & f1) & e2 -> (c1 & e2) & f1
                // ((c1 & f1) & g1) & e2 -> ((c1 & f1) & e2) & g1 -> ((c1 & e2) & f1) & g1
                (Some((c1, f1)), _) => Some(recurse(c1, expression2).intersection(f1)),
                (None, None) => None,
            },
        }
    }

    // Bottom-up pass pulls up-right filter node from leaf '(c & f) & e' ->
    // '(c & e) & f', so that an intersection of filter node can be found as
    // a direct child of another intersection node. However, the rewritten
    // intersection node 'c & e' can also be a rewrite target if 'e' contains
    // a filter node. That's why intersect_down() is also recursive.
    transform_expression_bottom_up(expression, |expression| match expression.as_ref() {
        RevsetExpression::Present(e) => {
            is_filter_tree(e).then(|| Rc::new(RevsetExpression::AsFilter(expression.clone())))
        }
        RevsetExpression::NotIn(e) => {
            is_filter_tree(e).then(|| Rc::new(RevsetExpression::AsFilter(expression.clone())))
        }
        RevsetExpression::Union(e1, e2) => (is_filter_tree(e1) || is_filter_tree(e2))
            .then(|| Rc::new(RevsetExpression::AsFilter(expression.clone()))),
        RevsetExpression::Intersection(expression1, expression2) => {
            intersect_down(expression1, expression2)
        }
        // Difference(e1, e2) should have been unfolded to Intersection(e1, NotIn(e2)).
        _ => None,
    })
}

/// Eliminates redundant nodes like `x & all()`, `~~x`.
///
/// This does not rewrite 'x & none()' to 'none()' because 'x' may be an invalid
/// symbol.
fn fold_redundant_expression(expression: &Rc<RevsetExpression>) -> TransformedExpression {
    transform_expression_bottom_up(expression, |expression| match expression.as_ref() {
        RevsetExpression::NotIn(outer) => match outer.as_ref() {
            RevsetExpression::NotIn(inner) => Some(inner.clone()),
            _ => None,
        },
        RevsetExpression::Intersection(expression1, expression2) => {
            match (expression1.as_ref(), expression2.as_ref()) {
                (_, RevsetExpression::All) => Some(expression1.clone()),
                (RevsetExpression::All, _) => Some(expression2.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

fn to_difference_range(
    expression: &Rc<RevsetExpression>,
    complement: &Rc<RevsetExpression>,
) -> TransformedExpression {
    match (expression.as_ref(), complement.as_ref()) {
        // ::heads & ~(::roots) -> roots..heads
        (
            RevsetExpression::Ancestors { heads, generation },
            RevsetExpression::Ancestors {
                heads: roots,
                generation: GENERATION_RANGE_FULL,
            },
        ) => Some(Rc::new(RevsetExpression::Range {
            roots: roots.clone(),
            heads: heads.clone(),
            generation: generation.clone(),
        })),
        // ::heads & ~(::roots-) -> ::heads & ~ancestors(roots, 1..) -> roots-..heads
        (
            RevsetExpression::Ancestors { heads, generation },
            RevsetExpression::Ancestors {
                heads: roots,
                generation:
                    Range {
                        start: roots_start,
                        end: u64::MAX,
                    },
            },
        ) => Some(Rc::new(RevsetExpression::Range {
            roots: roots.ancestors_at(*roots_start),
            heads: heads.clone(),
            generation: generation.clone(),
        })),
        _ => None,
    }
}

/// Transforms negative intersection to difference. Redundant intersections like
/// `all() & e` should have been removed.
fn fold_difference(expression: &Rc<RevsetExpression>) -> TransformedExpression {
    fn to_difference(
        expression: &Rc<RevsetExpression>,
        complement: &Rc<RevsetExpression>,
    ) -> Rc<RevsetExpression> {
        to_difference_range(expression, complement).unwrap_or_else(|| expression.minus(complement))
    }

    transform_expression_bottom_up(expression, |expression| match expression.as_ref() {
        RevsetExpression::Intersection(expression1, expression2) => {
            match (expression1.as_ref(), expression2.as_ref()) {
                // For '~x & f', don't move filter node 'f' left
                (_, RevsetExpression::Filter(_) | RevsetExpression::AsFilter(_)) => None,
                (_, RevsetExpression::NotIn(complement)) => {
                    Some(to_difference(expression1, complement))
                }
                (RevsetExpression::NotIn(complement), _) => {
                    Some(to_difference(expression2, complement))
                }
                _ => None,
            }
        }
        _ => None,
    })
}

/// Transforms remaining negated ancestors `~(::h)` to range `h..`.
///
/// Since this rule inserts redundant `visible_heads()`, negative intersections
/// should have been transformed.
fn fold_not_in_ancestors(expression: &Rc<RevsetExpression>) -> TransformedExpression {
    transform_expression_bottom_up(expression, |expression| match expression.as_ref() {
        RevsetExpression::NotIn(complement)
            if matches!(complement.as_ref(), RevsetExpression::Ancestors { .. }) =>
        {
            // ~(::heads) -> heads..
            // ~(::heads-) -> ~ancestors(heads, 1..) -> heads-..
            to_difference_range(&RevsetExpression::visible_heads().ancestors(), complement)
        }
        _ => None,
    })
}

/// Transforms binary difference to more primitive negative intersection.
///
/// For example, `all() ~ e` will become `all() & ~e`, which can be simplified
/// further by `fold_redundant_expression()`.
fn unfold_difference(expression: &Rc<RevsetExpression>) -> TransformedExpression {
    transform_expression_bottom_up(expression, |expression| match expression.as_ref() {
        // roots..heads -> ::heads & ~(::roots)
        RevsetExpression::Range {
            roots,
            heads,
            generation,
        } => {
            let heads_ancestors = Rc::new(RevsetExpression::Ancestors {
                heads: heads.clone(),
                generation: generation.clone(),
            });
            Some(heads_ancestors.intersection(&roots.ancestors().negated()))
        }
        RevsetExpression::Difference(expression1, expression2) => {
            Some(expression1.intersection(&expression2.negated()))
        }
        _ => None,
    })
}

/// Transforms nested `ancestors()`/`parents()`/`descendants()`/`children()`
/// like `h---`/`r+++`.
fn fold_generation(expression: &Rc<RevsetExpression>) -> TransformedExpression {
    fn add_generation(generation1: &Range<u64>, generation2: &Range<u64>) -> Range<u64> {
        // For any (g1, g2) in (generation1, generation2), g1 + g2.
        if generation1.is_empty() || generation2.is_empty() {
            GENERATION_RANGE_EMPTY
        } else {
            let start = u64::saturating_add(generation1.start, generation2.start);
            let end = u64::saturating_add(generation1.end, generation2.end - 1);
            start..end
        }
    }

    transform_expression_bottom_up(expression, |expression| match expression.as_ref() {
        RevsetExpression::Ancestors {
            heads,
            generation: generation1,
        } => {
            match heads.as_ref() {
                // (h-)- -> ancestors(ancestors(h, 1), 1) -> ancestors(h, 2)
                // ::(h-) -> ancestors(ancestors(h, 1), ..) -> ancestors(h, 1..)
                // (::h)- -> ancestors(ancestors(h, ..), 1) -> ancestors(h, 1..)
                RevsetExpression::Ancestors {
                    heads,
                    generation: generation2,
                } => Some(Rc::new(RevsetExpression::Ancestors {
                    heads: heads.clone(),
                    generation: add_generation(generation1, generation2),
                })),
                _ => None,
            }
        }
        RevsetExpression::Descendants {
            roots,
            generation: generation1,
        } => {
            match roots.as_ref() {
                // (r+)+ -> descendants(descendants(r, 1), 1) -> descendants(r, 2)
                // (r+):: -> descendants(descendants(r, 1), ..) -> descendants(r, 1..)
                // (r::)+ -> descendants(descendants(r, ..), 1) -> descendants(r, 1..)
                RevsetExpression::Descendants {
                    roots,
                    generation: generation2,
                } => Some(Rc::new(RevsetExpression::Descendants {
                    roots: roots.clone(),
                    generation: add_generation(generation1, generation2),
                })),
                _ => None,
            }
        }
        // Range should have been unfolded to intersection of Ancestors.
        _ => None,
    })
}

/// Rewrites the given `expression` tree to reduce evaluation cost. Returns new
/// tree.
pub fn optimize(expression: Rc<RevsetExpression>) -> Rc<RevsetExpression> {
    let expression = unfold_difference(&expression).unwrap_or(expression);
    let expression = fold_redundant_expression(&expression).unwrap_or(expression);
    let expression = fold_generation(&expression).unwrap_or(expression);
    let expression = internalize_filter(&expression).unwrap_or(expression);
    let expression = fold_difference(&expression).unwrap_or(expression);
    fold_not_in_ancestors(&expression).unwrap_or(expression)
}

// TODO: find better place to host this function (or add compile-time revset
// parsing and resolution like
// `revset!("{unwanted}..{wanted}").evaluate(repo)`?)
pub fn walk_revs<'index>(
    repo: &'index dyn Repo,
    wanted: &[CommitId],
    unwanted: &[CommitId],
) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError> {
    RevsetExpression::commits(unwanted.to_vec())
        .range(&RevsetExpression::commits(wanted.to_vec()))
        .evaluate_programmatic(repo)
}

fn resolve_remote_branch(repo: &dyn Repo, name: &str, remote: &str) -> Option<Vec<CommitId>> {
    let view = repo.view();
    let target = match (name, remote) {
        ("HEAD", git::REMOTE_NAME_FOR_LOCAL_GIT_REPO) => view.git_head(),
        (name, remote) => &view.get_remote_branch(name, remote).target,
    };
    target
        .is_present()
        .then(|| target.added_ids().cloned().collect())
}

fn all_branch_symbols(
    repo: &dyn Repo,
    include_synced_remotes: bool,
) -> impl Iterator<Item = String> + '_ {
    let view = repo.view();
    view.branches()
        .flat_map(move |(name, branch_target)| {
            // Remote branch "x"@"y" may conflict with local "x@y" in unquoted form.
            let local_target = branch_target.local_target;
            let local_symbol = local_target.is_present().then(|| name.to_owned());
            let remote_symbols = branch_target
                .remote_refs
                .into_iter()
                .filter(move |&(_, remote_ref)| {
                    include_synced_remotes
                        || !remote_ref.is_tracking()
                        || remote_ref.target != *local_target
                })
                .map(move |(remote_name, _)| format!("{name}@{remote_name}"));
            local_symbol.into_iter().chain(remote_symbols)
        })
        .chain(view.git_head().is_present().then(|| "HEAD@git".to_owned()))
}

fn make_no_such_symbol_error(repo: &dyn Repo, name: impl Into<String>) -> RevsetResolutionError {
    let name = name.into();
    // TODO: include tags?
    let branch_names = all_branch_symbols(repo, name.contains('@'));
    let candidates = collect_similar(&name, branch_names);
    RevsetResolutionError::NoSuchRevision { name, candidates }
}

pub trait SymbolResolver {
    fn resolve_symbol(&self, symbol: &str) -> Result<Vec<CommitId>, RevsetResolutionError>;
}

/// Fails on any attempt to resolve a symbol.
pub struct FailingSymbolResolver;

impl SymbolResolver for FailingSymbolResolver {
    fn resolve_symbol(&self, symbol: &str) -> Result<Vec<CommitId>, RevsetResolutionError> {
        Err(RevsetResolutionError::NoSuchRevision {
            name: format!(
                "Won't resolve symbol {symbol:?}. When creating revsets programmatically, avoid \
                 using RevsetExpression::symbol(); use RevsetExpression::commits() instead."
            ),
            candidates: Default::default(),
        })
    }
}

/// A symbol resolver for a specific namespace of labels.
///
/// Returns None if it cannot handle the symbol.
pub trait PartialSymbolResolver {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError>;
}

struct TagResolver;

impl PartialSymbolResolver for TagResolver {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        let target = repo.view().get_tag(symbol);
        Ok(target
            .is_present()
            .then(|| target.added_ids().cloned().collect()))
    }
}

struct BranchResolver;

impl PartialSymbolResolver for BranchResolver {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        let target = repo.view().get_local_branch(symbol);
        Ok(target
            .is_present()
            .then(|| target.added_ids().cloned().collect()))
    }
}

struct GitRefResolver;

impl PartialSymbolResolver for GitRefResolver {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        let view = repo.view();
        for git_ref_prefix in &["", "refs/"] {
            let target = view.get_git_ref(&(git_ref_prefix.to_string() + symbol));
            if target.is_present() {
                return Ok(Some(target.added_ids().cloned().collect()));
            }
        }

        Ok(None)
    }
}

const DEFAULT_RESOLVERS: &[&'static dyn PartialSymbolResolver] =
    &[&TagResolver, &BranchResolver, &GitRefResolver];

#[derive(Default)]
struct CommitPrefixResolver<'a> {
    context: Option<&'a IdPrefixContext>,
}

impl PartialSymbolResolver for CommitPrefixResolver<'_> {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        if let Some(prefix) = HexPrefix::new(symbol) {
            let resolution = self
                .context
                .as_ref()
                .map(|ctx| ctx.resolve_commit_prefix(repo, &prefix))
                .unwrap_or_else(|| repo.index().resolve_commit_id_prefix(&prefix));
            match resolution {
                PrefixResolution::AmbiguousMatch => Err(
                    RevsetResolutionError::AmbiguousCommitIdPrefix(symbol.to_owned()),
                ),
                PrefixResolution::SingleMatch(id) => Ok(Some(vec![id])),
                PrefixResolution::NoMatch => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}

#[derive(Default)]
struct ChangePrefixResolver<'a> {
    context: Option<&'a IdPrefixContext>,
}

impl PartialSymbolResolver for ChangePrefixResolver<'_> {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        if let Some(prefix) = to_forward_hex(symbol).as_deref().and_then(HexPrefix::new) {
            let resolution = self
                .context
                .as_ref()
                .map(|ctx| ctx.resolve_change_prefix(repo, &prefix))
                .unwrap_or_else(|| repo.resolve_change_id_prefix(&prefix));
            match resolution {
                PrefixResolution::AmbiguousMatch => Err(
                    RevsetResolutionError::AmbiguousChangeIdPrefix(symbol.to_owned()),
                ),
                PrefixResolution::SingleMatch(ids) => Ok(Some(ids)),
                PrefixResolution::NoMatch => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}

/// An extension of the DefaultSymbolResolver.
///
/// Each PartialSymbolResolver will be invoked in order, its result used if one
/// is provided. Native resolvers are always invoked first. In the future, we
/// may provide a way for extensions to override native resolvers like tags and
/// branches.
pub trait SymbolResolverExtension {
    /// PartialSymbolResolvers can capture `repo` for caching purposes if
    /// desired, but they do not have to since `repo` is passed into
    /// `resolve_symbol()` as well.
    fn new_resolvers<'a>(&self, repo: &'a dyn Repo) -> Vec<Box<dyn PartialSymbolResolver + 'a>>;
}

/// Resolves branches, remote branches, tags, git refs, and full and abbreviated
/// commit and change ids.
pub struct DefaultSymbolResolver<'a> {
    repo: &'a dyn Repo,
    commit_id_resolver: CommitPrefixResolver<'a>,
    change_id_resolver: ChangePrefixResolver<'a>,
    extensions: Vec<Box<dyn PartialSymbolResolver + 'a>>,
}

impl<'a> DefaultSymbolResolver<'a> {
    pub fn new(repo: &'a dyn Repo, extensions: &[impl AsRef<dyn SymbolResolverExtension>]) -> Self {
        DefaultSymbolResolver {
            repo,
            commit_id_resolver: Default::default(),
            change_id_resolver: Default::default(),
            extensions: extensions
                .iter()
                .flat_map(|ext| ext.as_ref().new_resolvers(repo))
                .collect(),
        }
    }

    pub fn with_id_prefix_context(mut self, id_prefix_context: &'a IdPrefixContext) -> Self {
        self.commit_id_resolver.context = Some(id_prefix_context);
        self.change_id_resolver.context = Some(id_prefix_context);
        self
    }

    fn partial_resolvers(&self) -> impl Iterator<Item = &(dyn PartialSymbolResolver + 'a)> {
        let prefix_resolvers: [&dyn PartialSymbolResolver; 2] =
            [&self.commit_id_resolver, &self.change_id_resolver];
        itertools::chain!(
            DEFAULT_RESOLVERS.iter().copied(),
            prefix_resolvers,
            self.extensions.iter().map(|e| e.as_ref())
        )
    }
}

impl SymbolResolver for DefaultSymbolResolver<'_> {
    fn resolve_symbol(&self, symbol: &str) -> Result<Vec<CommitId>, RevsetResolutionError> {
        if symbol.is_empty() {
            return Err(RevsetResolutionError::EmptyString);
        }

        for partial_resolver in self.partial_resolvers() {
            if let Some(ids) = partial_resolver.resolve_symbol(self.repo, symbol)? {
                return Ok(ids);
            }
        }

        Err(make_no_such_symbol_error(self.repo, symbol))
    }
}

fn resolve_commit_ref(
    repo: &dyn Repo,
    commit_ref: &RevsetCommitRef,
    symbol_resolver: &dyn SymbolResolver,
) -> Result<Vec<CommitId>, RevsetResolutionError> {
    match commit_ref {
        RevsetCommitRef::Symbol(symbol) => symbol_resolver.resolve_symbol(symbol),
        RevsetCommitRef::RemoteSymbol { name, remote } => resolve_remote_branch(repo, name, remote)
            .ok_or_else(|| make_no_such_symbol_error(repo, format!("{name}@{remote}"))),
        RevsetCommitRef::WorkingCopy(workspace_id) => {
            if let Some(commit_id) = repo.view().get_wc_commit_id(workspace_id) {
                Ok(vec![commit_id.clone()])
            } else {
                Err(RevsetResolutionError::WorkspaceMissingWorkingCopy {
                    name: workspace_id.as_str().to_string(),
                })
            }
        }
        RevsetCommitRef::WorkingCopies => {
            let wc_commits = repo.view().wc_commit_ids().values().cloned().collect_vec();
            Ok(wc_commits)
        }
        RevsetCommitRef::VisibleHeads => Ok(repo.view().heads().iter().cloned().collect_vec()),
        RevsetCommitRef::Root => Ok(vec![repo.store().root_commit_id().clone()]),
        RevsetCommitRef::Branches(pattern) => {
            let commit_ids = repo
                .view()
                .local_branches_matching(pattern)
                .flat_map(|(_, target)| target.added_ids())
                .cloned()
                .collect();
            Ok(commit_ids)
        }
        RevsetCommitRef::RemoteBranches {
            branch_pattern,
            remote_pattern,
        } => {
            // TODO: should we allow to select @git branches explicitly?
            let commit_ids = repo
                .view()
                .remote_branches_matching(branch_pattern, remote_pattern)
                .filter(|&((_, remote_name), _)| remote_name != git::REMOTE_NAME_FOR_LOCAL_GIT_REPO)
                .flat_map(|(_, remote_ref)| remote_ref.target.added_ids())
                .cloned()
                .collect();
            Ok(commit_ids)
        }
        RevsetCommitRef::Tags => {
            let mut commit_ids = vec![];
            for ref_target in repo.view().tags().values() {
                commit_ids.extend(ref_target.added_ids().cloned());
            }
            Ok(commit_ids)
        }
        RevsetCommitRef::GitRefs => {
            let mut commit_ids = vec![];
            for ref_target in repo.view().git_refs().values() {
                commit_ids.extend(ref_target.added_ids().cloned());
            }
            Ok(commit_ids)
        }
        RevsetCommitRef::GitHead => Ok(repo.view().git_head().added_ids().cloned().collect()),
    }
}

fn resolve_symbols(
    repo: &dyn Repo,
    expression: Rc<RevsetExpression>,
    symbol_resolver: &dyn SymbolResolver,
) -> Result<Rc<RevsetExpression>, RevsetResolutionError> {
    Ok(try_transform_expression(
        &expression,
        |expression| match expression.as_ref() {
            // 'present(x)' opens new symbol resolution scope to map error to 'none()'.
            RevsetExpression::Present(candidates) => {
                resolve_symbols(repo, candidates.clone(), symbol_resolver)
                    .or_else(|err| match err {
                        RevsetResolutionError::NoSuchRevision { .. } => {
                            Ok(RevsetExpression::none())
                        }
                        RevsetResolutionError::WorkspaceMissingWorkingCopy { .. }
                        | RevsetResolutionError::EmptyString
                        | RevsetResolutionError::AmbiguousCommitIdPrefix(_)
                        | RevsetResolutionError::AmbiguousChangeIdPrefix(_)
                        | RevsetResolutionError::StoreError(_)
                        | RevsetResolutionError::Other(_) => Err(err),
                    })
                    .map(Some) // Always rewrite subtree
            }
            // Otherwise resolve symbols recursively.
            _ => Ok(None),
        },
        |expression| match expression.as_ref() {
            RevsetExpression::CommitRef(commit_ref) => {
                let commit_ids = resolve_commit_ref(repo, commit_ref, symbol_resolver)?;
                Ok(Some(RevsetExpression::commits(commit_ids)))
            }
            _ => Ok(None),
        },
    )?
    .unwrap_or(expression))
}

/// Inserts implicit `all()` and `visible_heads()` nodes to the `expression`.
///
/// Symbols and commit refs in the `expression` should have been resolved.
///
/// This is a separate step because a symbol-resolved `expression` could be
/// transformed further to e.g. combine OR-ed `Commits(_)`, or to collect
/// commit ids to make `all()` include hidden-but-specified commits. The
/// return type `ResolvedExpression` is stricter than `RevsetExpression`,
/// and isn't designed for such transformation.
fn resolve_visibility(repo: &dyn Repo, expression: &RevsetExpression) -> ResolvedExpression {
    // If we add "operation" scope (#1283), visible_heads might be translated to
    // `RevsetExpression::WithinOperation(visible_heads, expression)` node to
    // evaluate filter predicates and "all()" against that scope.
    let context = VisibilityResolutionContext {
        visible_heads: &repo.view().heads().iter().cloned().collect_vec(),
    };
    context.resolve(expression)
}

#[derive(Clone, Debug)]
struct VisibilityResolutionContext<'a> {
    visible_heads: &'a [CommitId],
}

impl VisibilityResolutionContext<'_> {
    /// Resolves expression tree as set.
    fn resolve(&self, expression: &RevsetExpression) -> ResolvedExpression {
        match expression {
            RevsetExpression::None => ResolvedExpression::Commits(vec![]),
            RevsetExpression::All => self.resolve_all(),
            RevsetExpression::Commits(commit_ids) => {
                ResolvedExpression::Commits(commit_ids.clone())
            }
            RevsetExpression::StringPattern { .. } => {
                panic!("Expression '{expression:?}' should be rejected by parser");
            }
            RevsetExpression::CommitRef(_) => {
                panic!("Expression '{expression:?}' should have been resolved by caller");
            }
            RevsetExpression::Ancestors { heads, generation } => ResolvedExpression::Ancestors {
                heads: self.resolve(heads).into(),
                generation: generation.clone(),
            },
            RevsetExpression::Descendants { roots, generation } => ResolvedExpression::DagRange {
                roots: self.resolve(roots).into(),
                heads: self.resolve_visible_heads().into(),
                generation_from_roots: generation.clone(),
            },
            RevsetExpression::Range {
                roots,
                heads,
                generation,
            } => ResolvedExpression::Range {
                roots: self.resolve(roots).into(),
                heads: self.resolve(heads).into(),
                generation: generation.clone(),
            },
            RevsetExpression::DagRange { roots, heads } => ResolvedExpression::DagRange {
                roots: self.resolve(roots).into(),
                heads: self.resolve(heads).into(),
                generation_from_roots: GENERATION_RANGE_FULL,
            },
            RevsetExpression::Reachable { sources, domain } => ResolvedExpression::Reachable {
                sources: self.resolve(sources).into(),
                domain: self.resolve(domain).into(),
            },
            RevsetExpression::Heads(candidates) => {
                ResolvedExpression::Heads(self.resolve(candidates).into())
            }
            RevsetExpression::Roots(candidates) => {
                ResolvedExpression::Roots(self.resolve(candidates).into())
            }
            RevsetExpression::Latest { candidates, count } => ResolvedExpression::Latest {
                candidates: self.resolve(candidates).into(),
                count: *count,
            },
            RevsetExpression::Filter(_) | RevsetExpression::AsFilter(_) => {
                // Top-level filter without intersection: e.g. "~author(_)" is represented as
                // `AsFilter(NotIn(Filter(Author(_))))`.
                ResolvedExpression::FilterWithin {
                    candidates: self.resolve_all().into(),
                    predicate: self.resolve_predicate(expression),
                }
            }
            RevsetExpression::Present(_) => {
                panic!("Expression '{expression:?}' should have been resolved by caller");
            }
            RevsetExpression::NotIn(complement) => ResolvedExpression::Difference(
                self.resolve_all().into(),
                self.resolve(complement).into(),
            ),
            RevsetExpression::Union(expression1, expression2) => ResolvedExpression::Union(
                self.resolve(expression1).into(),
                self.resolve(expression2).into(),
            ),
            RevsetExpression::Intersection(expression1, expression2) => {
                match expression2.as_ref() {
                    RevsetExpression::Filter(_) | RevsetExpression::AsFilter(_) => {
                        ResolvedExpression::FilterWithin {
                            candidates: self.resolve(expression1).into(),
                            predicate: self.resolve_predicate(expression2),
                        }
                    }
                    _ => ResolvedExpression::Intersection(
                        self.resolve(expression1).into(),
                        self.resolve(expression2).into(),
                    ),
                }
            }
            RevsetExpression::Difference(expression1, expression2) => {
                ResolvedExpression::Difference(
                    self.resolve(expression1).into(),
                    self.resolve(expression2).into(),
                )
            }
        }
    }

    fn resolve_all(&self) -> ResolvedExpression {
        // Since `all()` does not include hidden commits, some of the logical
        // transformation rules may subtly change the evaluated set. For example,
        // `all() & x` is not `x` if `x` is hidden. This wouldn't matter in practice,
        // but if it does, the heads set could be extended to include the commits
        // (and `remote_branches()`) specified in the revset expression. Alternatively,
        // some optimization rules could be removed, but that means `author(_) & x`
        // would have to test `::visible_heads() & x`.
        ResolvedExpression::Ancestors {
            heads: self.resolve_visible_heads().into(),
            generation: GENERATION_RANGE_FULL,
        }
    }

    fn resolve_visible_heads(&self) -> ResolvedExpression {
        ResolvedExpression::Commits(self.visible_heads.to_owned())
    }

    /// Resolves expression tree as filter predicate.
    ///
    /// For filter expression, this never inserts a hidden `all()` since a
    /// filter predicate doesn't need to produce revisions to walk.
    fn resolve_predicate(&self, expression: &RevsetExpression) -> ResolvedPredicateExpression {
        match expression {
            RevsetExpression::None
            | RevsetExpression::All
            | RevsetExpression::Commits(_)
            | RevsetExpression::CommitRef(_)
            | RevsetExpression::StringPattern { .. }
            | RevsetExpression::Ancestors { .. }
            | RevsetExpression::Descendants { .. }
            | RevsetExpression::Range { .. }
            | RevsetExpression::DagRange { .. }
            | RevsetExpression::Reachable { .. }
            | RevsetExpression::Heads(_)
            | RevsetExpression::Roots(_)
            | RevsetExpression::Latest { .. } => {
                ResolvedPredicateExpression::Set(self.resolve(expression).into())
            }
            RevsetExpression::Filter(predicate) => {
                ResolvedPredicateExpression::Filter(predicate.clone())
            }
            RevsetExpression::AsFilter(candidates) => self.resolve_predicate(candidates),
            RevsetExpression::Present(_) => {
                panic!("Expression '{expression:?}' should have been resolved by caller")
            }
            RevsetExpression::NotIn(complement) => {
                ResolvedPredicateExpression::NotIn(self.resolve_predicate(complement).into())
            }
            RevsetExpression::Union(expression1, expression2) => {
                let predicate1 = self.resolve_predicate(expression1);
                let predicate2 = self.resolve_predicate(expression2);
                ResolvedPredicateExpression::Union(predicate1.into(), predicate2.into())
            }
            // Intersection of filters should have been substituted by optimize().
            // If it weren't, just fall back to the set evaluation path.
            RevsetExpression::Intersection(..) | RevsetExpression::Difference(..) => {
                ResolvedPredicateExpression::Set(self.resolve(expression).into())
            }
        }
    }
}

pub trait Revset: fmt::Debug {
    /// Iterate in topological order with children before parents.
    fn iter<'a>(&self) -> Box<dyn Iterator<Item = CommitId> + 'a>
    where
        Self: 'a;

    /// Iterates commit/change id pairs in topological order.
    fn commit_change_ids<'a>(&self) -> Box<dyn Iterator<Item = (CommitId, ChangeId)> + 'a>
    where
        Self: 'a;

    fn iter_graph<'a>(&self) -> Box<dyn Iterator<Item = (CommitId, Vec<GraphEdge<CommitId>>)> + 'a>
    where
        Self: 'a;

    fn is_empty(&self) -> bool;

    /// Inclusive lower bound and, optionally, inclusive upper bound of how many
    /// commits are in the revset. The implementation can use its discretion as
    /// to how much effort should be put into the estimation, and how accurate
    /// the resulting estimate should be.
    fn count_estimate(&self) -> (usize, Option<usize>);

    /// Returns a closure that checks if a commit is contained within the
    /// revset.
    ///
    /// The implementation may construct and maintain any necessary internal
    /// context to optimize the performance of the check.
    fn containing_fn<'a>(&self) -> Box<dyn Fn(&CommitId) -> bool + 'a>
    where
        Self: 'a;
}

pub trait RevsetIteratorExt<'index, I> {
    fn commits(self, store: &Arc<Store>) -> RevsetCommitIterator<I>;
    fn reversed(self) -> ReverseRevsetIterator;
}

impl<'index, I: Iterator<Item = CommitId>> RevsetIteratorExt<'index, I> for I {
    fn commits(self, store: &Arc<Store>) -> RevsetCommitIterator<I> {
        RevsetCommitIterator {
            iter: self,
            store: store.clone(),
        }
    }

    fn reversed(self) -> ReverseRevsetIterator {
        ReverseRevsetIterator {
            entries: self.into_iter().collect_vec(),
        }
    }
}

pub struct RevsetCommitIterator<I> {
    store: Arc<Store>,
    iter: I,
}

impl<I: Iterator<Item = CommitId>> Iterator for RevsetCommitIterator<I> {
    type Item = BackendResult<Commit>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|commit_id| self.store.get_commit(&commit_id))
    }
}

pub struct ReverseRevsetIterator {
    entries: Vec<CommitId>,
}

impl Iterator for ReverseRevsetIterator {
    type Item = CommitId;

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.pop()
    }
}

/// A set of extensions for revset evaluation.
pub struct RevsetExtensions {
    symbol_resolvers: Vec<Box<dyn SymbolResolverExtension>>,
    function_map: HashMap<&'static str, RevsetFunction>,
}

impl Default for RevsetExtensions {
    fn default() -> Self {
        Self::new()
    }
}

impl RevsetExtensions {
    pub fn new() -> Self {
        Self {
            symbol_resolvers: vec![],
            function_map: BUILTIN_FUNCTION_MAP.clone(),
        }
    }

    pub fn symbol_resolvers(&self) -> &[impl AsRef<dyn SymbolResolverExtension>] {
        &self.symbol_resolvers
    }

    pub fn add_symbol_resolver(&mut self, symbol_resolver: Box<dyn SymbolResolverExtension>) {
        self.symbol_resolvers.push(symbol_resolver);
    }

    pub fn add_custom_function(&mut self, name: &'static str, func: RevsetFunction) {
        match self.function_map.entry(name) {
            hash_map::Entry::Occupied(_) => {
                panic!("Conflict registering revset function '{name}'")
            }
            hash_map::Entry::Vacant(v) => v.insert(func),
        };
    }
}

/// Information needed to parse revset expression.
#[derive(Clone)]
pub struct RevsetParseContext<'a> {
    aliases_map: &'a RevsetAliasesMap,
    user_email: String,
    extensions: &'a RevsetExtensions,
    workspace: Option<RevsetWorkspaceContext<'a>>,
}

impl<'a> RevsetParseContext<'a> {
    pub fn new(
        aliases_map: &'a RevsetAliasesMap,
        user_email: String,
        extensions: &'a RevsetExtensions,
        workspace: Option<RevsetWorkspaceContext<'a>>,
    ) -> Self {
        Self {
            aliases_map,
            user_email,
            extensions,
            workspace,
        }
    }

    pub fn aliases_map(&self) -> &'a RevsetAliasesMap {
        self.aliases_map
    }

    pub fn user_email(&self) -> &str {
        &self.user_email
    }

    pub fn symbol_resolvers(&self) -> &[impl AsRef<dyn SymbolResolverExtension>] {
        self.extensions.symbol_resolvers()
    }
}

/// Workspace information needed to parse revset expression.
#[derive(Clone, Debug)]
pub struct RevsetWorkspaceContext<'a> {
    pub path_converter: &'a RepoPathUiConverter,
    pub workspace_id: &'a WorkspaceId,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use assert_matches::assert_matches;

    use super::*;
    use crate::repo_path::RepoPathBuf;

    fn parse(revset_str: &str) -> Result<Rc<RevsetExpression>, RevsetParseErrorKind> {
        parse_with_aliases(revset_str, [] as [(&str, &str); 0])
    }

    fn parse_with_workspace(
        revset_str: &str,
        workspace_id: &WorkspaceId,
    ) -> Result<Rc<RevsetExpression>, RevsetParseErrorKind> {
        parse_with_aliases_and_workspace(revset_str, [] as [(&str, &str); 0], workspace_id)
    }

    fn parse_with_aliases(
        revset_str: &str,
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
    ) -> Result<Rc<RevsetExpression>, RevsetParseErrorKind> {
        let mut aliases_map = RevsetAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        let extensions = RevsetExtensions::default();
        let context = RevsetParseContext::new(
            &aliases_map,
            "test.user@example.com".to_string(),
            &extensions,
            None,
        );
        // Map error to comparable object
        super::parse(revset_str, &context).map_err(|e| e.kind)
    }

    fn parse_with_aliases_and_workspace(
        revset_str: &str,
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
        workspace_id: &WorkspaceId,
    ) -> Result<Rc<RevsetExpression>, RevsetParseErrorKind> {
        // Set up pseudo context to resolve `workspace_id@` and `file(path)`
        let path_converter = RepoPathUiConverter::Fs {
            cwd: PathBuf::from("/"),
            base: PathBuf::from("/"),
        };
        let workspace_ctx = RevsetWorkspaceContext {
            path_converter: &path_converter,
            workspace_id,
        };
        let mut aliases_map = RevsetAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        let extensions = RevsetExtensions::default();
        let context = RevsetParseContext::new(
            &aliases_map,
            "test.user@example.com".to_string(),
            &extensions,
            Some(workspace_ctx),
        );
        // Map error to comparable object
        super::parse(revset_str, &context).map_err(|e| e.kind)
    }

    fn parse_with_modifier(
        revset_str: &str,
    ) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseErrorKind> {
        parse_with_aliases_and_modifier(revset_str, [] as [(&str, &str); 0])
    }

    fn parse_with_aliases_and_modifier(
        revset_str: &str,
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
    ) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseErrorKind> {
        let mut aliases_map = RevsetAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        let extensions = RevsetExtensions::default();
        let context = RevsetParseContext::new(
            &aliases_map,
            "test.user@example.com".to_string(),
            &extensions,
            None,
        );
        // Map error to comparable object
        super::parse_with_modifier(revset_str, &context).map_err(|e| e.kind)
    }

    #[test]
    #[allow(clippy::redundant_clone)] // allow symbol.clone()
    fn test_revset_expression_building() {
        let current_wc = RevsetExpression::working_copy(WorkspaceId::default());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        let bar_symbol = RevsetExpression::symbol("bar".to_string());
        let baz_symbol = RevsetExpression::symbol("baz".to_string());
        assert_eq!(
            current_wc,
            Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::WorkingCopy(
                WorkspaceId::default()
            ))),
        );
        assert_eq!(
            current_wc.heads(),
            Rc::new(RevsetExpression::Heads(current_wc.clone()))
        );
        assert_eq!(
            current_wc.roots(),
            Rc::new(RevsetExpression::Roots(current_wc.clone()))
        );
        assert_eq!(
            current_wc.parents(),
            Rc::new(RevsetExpression::Ancestors {
                heads: current_wc.clone(),
                generation: 1..2,
            })
        );
        assert_eq!(
            current_wc.ancestors(),
            Rc::new(RevsetExpression::Ancestors {
                heads: current_wc.clone(),
                generation: GENERATION_RANGE_FULL,
            })
        );
        assert_eq!(
            foo_symbol.children(),
            Rc::new(RevsetExpression::Descendants {
                roots: foo_symbol.clone(),
                generation: 1..2,
            }),
        );
        assert_eq!(
            foo_symbol.descendants(),
            Rc::new(RevsetExpression::Descendants {
                roots: foo_symbol.clone(),
                generation: GENERATION_RANGE_FULL,
            })
        );
        assert_eq!(
            foo_symbol.dag_range_to(&current_wc),
            Rc::new(RevsetExpression::DagRange {
                roots: foo_symbol.clone(),
                heads: current_wc.clone(),
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
            foo_symbol.range(&current_wc),
            Rc::new(RevsetExpression::Range {
                roots: foo_symbol.clone(),
                heads: current_wc.clone(),
                generation: GENERATION_RANGE_FULL,
            })
        );
        assert_eq!(
            foo_symbol.negated(),
            Rc::new(RevsetExpression::NotIn(foo_symbol.clone()))
        );
        assert_eq!(
            foo_symbol.union(&current_wc),
            Rc::new(RevsetExpression::Union(
                foo_symbol.clone(),
                current_wc.clone()
            ))
        );
        assert_eq!(
            RevsetExpression::union_all(&[]),
            Rc::new(RevsetExpression::None)
        );
        assert_eq!(
            RevsetExpression::union_all(&[current_wc.clone()]),
            current_wc
        );
        assert_eq!(
            RevsetExpression::union_all(&[current_wc.clone(), foo_symbol.clone()]),
            Rc::new(RevsetExpression::Union(
                current_wc.clone(),
                foo_symbol.clone(),
            ))
        );
        assert_eq!(
            RevsetExpression::union_all(&[
                current_wc.clone(),
                foo_symbol.clone(),
                bar_symbol.clone(),
            ]),
            Rc::new(RevsetExpression::Union(
                current_wc.clone(),
                Rc::new(RevsetExpression::Union(
                    foo_symbol.clone(),
                    bar_symbol.clone(),
                ))
            ))
        );
        assert_eq!(
            RevsetExpression::union_all(&[
                current_wc.clone(),
                foo_symbol.clone(),
                bar_symbol.clone(),
                baz_symbol.clone(),
            ]),
            Rc::new(RevsetExpression::Union(
                Rc::new(RevsetExpression::Union(
                    current_wc.clone(),
                    foo_symbol.clone(),
                )),
                Rc::new(RevsetExpression::Union(
                    bar_symbol.clone(),
                    baz_symbol.clone(),
                ))
            ))
        );
        assert_eq!(
            foo_symbol.intersection(&current_wc),
            Rc::new(RevsetExpression::Intersection(
                foo_symbol.clone(),
                current_wc.clone()
            ))
        );
        assert_eq!(
            foo_symbol.minus(&current_wc),
            Rc::new(RevsetExpression::Difference(foo_symbol, current_wc.clone()))
        );
    }

    #[test]
    fn test_parse_revset() {
        let main_workspace_id = WorkspaceId::new("main".to_string());
        let other_workspace_id = WorkspaceId::new("other".to_string());
        let main_wc = RevsetExpression::working_copy(main_workspace_id.clone());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        let bar_symbol = RevsetExpression::symbol("bar".to_string());
        // Parse "@" (the current working copy)
        assert_eq!(
            parse("@"),
            Err(RevsetParseErrorKind::WorkingCopyWithoutWorkspace)
        );
        assert_eq!(parse("main@"), Ok(main_wc.clone()));
        assert_eq!(
            parse_with_workspace("@", &main_workspace_id),
            Ok(main_wc.clone())
        );
        assert_eq!(
            parse_with_workspace("main@", &other_workspace_id),
            Ok(main_wc)
        );
        assert_eq!(
            parse("main@origin"),
            Ok(RevsetExpression::remote_symbol(
                "main".to_string(),
                "origin".to_string()
            ))
        );
        // Quoted component in @ expression
        assert_eq!(
            parse(r#""foo bar"@"#),
            Ok(RevsetExpression::working_copy(WorkspaceId::new(
                "foo bar".to_string()
            )))
        );
        assert_eq!(
            parse(r#""foo bar"@origin"#),
            Ok(RevsetExpression::remote_symbol(
                "foo bar".to_string(),
                "origin".to_string()
            ))
        );
        assert_eq!(
            parse(r#"main@"foo bar""#),
            Ok(RevsetExpression::remote_symbol(
                "main".to_string(),
                "foo bar".to_string()
            ))
        );
        assert_eq!(
            parse(r#"'foo bar'@'bar baz'"#),
            Ok(RevsetExpression::remote_symbol(
                "foo bar".to_string(),
                "bar baz".to_string()
            ))
        );
        // Quoted "@" is not interpreted as a working copy or remote symbol
        assert_eq!(
            parse(r#""@""#),
            Ok(RevsetExpression::symbol("@".to_string()))
        );
        assert_eq!(
            parse(r#""main@""#),
            Ok(RevsetExpression::symbol("main@".to_string()))
        );
        assert_eq!(
            parse(r#""main@origin""#),
            Ok(RevsetExpression::symbol("main@origin".to_string()))
        );
        // "@" in function argument must be quoted
        assert_eq!(
            parse("author(foo@)"),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "author".to_string(),
                message: "Expected function argument of string pattern".to_string(),
            })
        );
        assert_eq!(
            parse(r#"author("foo@")"#),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
                StringPattern::Substring("foo@".to_string()),
            )))
        );
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
        // Default arguments for *branches() are all ""
        assert_eq!(parse("branches()"), parse(r#"branches("")"#));
        assert_eq!(parse("remote_branches()"), parse(r#"remote_branches("")"#));
        assert_eq!(
            parse("remote_branches()"),
            parse(r#"remote_branches("", "")"#)
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
        assert_eq!(parse("'foo'"), Ok(foo_symbol.clone()));
        // Parse the "parents" operator
        assert_eq!(parse("foo-"), Ok(foo_symbol.parents()));
        // Parse the "children" operator
        assert_eq!(parse("foo+"), Ok(foo_symbol.children()));
        // Parse the "ancestors" operator
        assert_eq!(parse("::foo"), Ok(foo_symbol.ancestors()));
        // Parse the "descendants" operator
        assert_eq!(parse("foo::"), Ok(foo_symbol.descendants()));
        // Parse the "dag range" operator
        assert_eq!(parse("foo::bar"), Ok(foo_symbol.dag_range_to(&bar_symbol)));
        // Parse the nullary "dag range" operator
        assert_eq!(parse("::"), Ok(RevsetExpression::all()));
        // Parse the "range" prefix operator
        assert_eq!(
            parse("..foo"),
            Ok(RevsetExpression::root().range(&foo_symbol))
        );
        assert_eq!(
            parse("foo.."),
            Ok(foo_symbol.range(&RevsetExpression::visible_heads()))
        );
        assert_eq!(parse("foo..bar"), Ok(foo_symbol.range(&bar_symbol)));
        // Parse the nullary "range" operator
        assert_eq!(
            parse(".."),
            Ok(RevsetExpression::root().range(&RevsetExpression::visible_heads()))
        );
        // Parse the "negate" operator
        assert_eq!(parse("~ foo"), Ok(foo_symbol.negated()));
        assert_eq!(
            parse("~ ~~ foo"),
            Ok(foo_symbol.negated().negated().negated())
        );
        // Parse the "intersection" operator
        assert_eq!(parse("foo & bar"), Ok(foo_symbol.intersection(&bar_symbol)));
        // Parse the "union" operator
        assert_eq!(parse("foo | bar"), Ok(foo_symbol.union(&bar_symbol)));
        // Parse the "difference" operator
        assert_eq!(parse("foo ~ bar"), Ok(foo_symbol.minus(&bar_symbol)));
        // Parentheses are allowed before suffix operators
        assert_eq!(parse("(foo)-"), Ok(foo_symbol.parents()));
        // Space is allowed around expressions
        assert_eq!(parse(" ::foo "), Ok(foo_symbol.ancestors()));
        assert_eq!(parse("( ::foo )"), Ok(foo_symbol.ancestors()));
        // Space is not allowed around prefix operators
        assert_eq!(parse(" :: foo "), Err(RevsetParseErrorKind::SyntaxError));
        // Incomplete parse
        assert_eq!(parse("foo | -"), Err(RevsetParseErrorKind::SyntaxError));
        // Space is allowed around infix operators and function arguments
        assert_eq!(
            parse_with_workspace(
                "   description(  arg1 ) ~    file(  arg1 ,   arg2 )  ~ visible_heads(  )  ",
                &main_workspace_id
            ),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Description(
                StringPattern::Substring("arg1".to_string())
            ))
            .minus(&RevsetExpression::filter(RevsetFilterPredicate::File(
                FilesetExpression::union_all(vec![
                    FilesetExpression::prefix_path(RepoPathBuf::from_internal_string("arg1")),
                    FilesetExpression::prefix_path(RepoPathBuf::from_internal_string("arg2")),
                ])
            )))
            .minus(&RevsetExpression::visible_heads()))
        );
        // Space is allowed around keyword arguments
        assert_eq!(
            parse("remote_branches( remote  =   foo  )").unwrap(),
            parse("remote_branches(remote=foo)").unwrap(),
        );

        // Trailing comma isn't allowed for empty argument
        assert!(parse("branches(,)").is_err());
        // Trailing comma is allowed for the last argument
        assert!(parse("branches(a,)").is_ok());
        assert!(parse("branches(a ,  )").is_ok());
        assert!(parse("branches(,a)").is_err());
        assert!(parse("branches(a,,)").is_err());
        assert!(parse("branches(a  , , )").is_err());
        assert!(parse_with_workspace("file(a,b,)", &main_workspace_id).is_ok());
        assert!(parse_with_workspace("file(a,,b)", &main_workspace_id).is_err());
        assert!(parse("remote_branches(a,remote=b  , )").is_ok());
        assert!(parse("remote_branches(a,,remote=b)").is_err());
    }

    #[test]
    fn test_parse_revset_with_modifier() {
        let all_symbol = RevsetExpression::symbol("all".to_owned());
        let foo_symbol = RevsetExpression::symbol("foo".to_owned());

        // all: is a program modifier, but all:: isn't
        assert_eq!(
            parse_with_modifier("all:"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_with_modifier("all:foo"),
            Ok((foo_symbol.clone(), Some(RevsetModifier::All)))
        );
        assert_eq!(
            parse_with_modifier("all::"),
            Ok((all_symbol.descendants(), None))
        );
        assert_eq!(
            parse_with_modifier("all::foo"),
            Ok((all_symbol.dag_range_to(&foo_symbol), None))
        );

        // all::: could be parsed as all:(::), but rejected for simplicity
        assert_eq!(
            parse_with_modifier("all:::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_with_modifier("all:::foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );

        assert_eq!(
            parse_with_modifier("all:(foo)"),
            Ok((foo_symbol.clone(), Some(RevsetModifier::All)))
        );
        assert_eq!(
            parse_with_modifier("all:all::foo"),
            Ok((
                all_symbol.dag_range_to(&foo_symbol),
                Some(RevsetModifier::All)
            ))
        );
        assert_eq!(
            parse_with_modifier("all:all | foo"),
            Ok((all_symbol.union(&foo_symbol), Some(RevsetModifier::All)))
        );

        assert_eq!(
            parse_with_modifier("all: ::foo"),
            Ok((foo_symbol.ancestors(), Some(RevsetModifier::All)))
        );
        assert_eq!(
            parse_with_modifier(" all: foo"),
            Ok((foo_symbol.clone(), Some(RevsetModifier::All)))
        );
        assert_matches!(
            parse_with_modifier("(all:foo)"),
            Err(RevsetParseErrorKind::NotInfixOperator { .. })
        );
        assert_matches!(
            parse_with_modifier("all :foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_matches!(
            parse_with_modifier("all:all:all"),
            Err(RevsetParseErrorKind::NotInfixOperator { .. })
        );

        // Top-level string pattern can't be parsed, which is an error anyway
        assert_eq!(
            parse_with_modifier(r#"exact:"foo""#),
            Err(RevsetParseErrorKind::NoSuchModifier("exact".to_owned()))
        );
    }

    #[test]
    fn test_parse_whitespace() {
        let ascii_whitespaces: String = ('\x00'..='\x7f')
            .filter(char::is_ascii_whitespace)
            .collect();
        assert_eq!(
            parse(&format!("{ascii_whitespaces}all()")).unwrap(),
            parse("all()").unwrap(),
        );
    }

    #[test]
    fn test_parse_string_literal() {
        let branches_expr =
            |s: &str| RevsetExpression::branches(StringPattern::Substring(s.to_owned()));

        // "\<char>" escapes
        assert_eq!(
            parse(r#"branches("\t\r\n\"\\\0")"#),
            Ok(branches_expr("\t\r\n\"\\\0"))
        );

        // Invalid "\<char>" escape
        assert_eq!(
            parse(r#"branches("\y")"#),
            Err(RevsetParseErrorKind::SyntaxError)
        );

        // Single-quoted raw string
        assert_eq!(parse(r#"branches('')"#), Ok(branches_expr("")));
        assert_eq!(parse(r#"branches('a\n')"#), Ok(branches_expr(r"a\n")));
        assert_eq!(parse(r#"branches('\')"#), Ok(branches_expr(r"\")));
        assert_eq!(parse(r#"branches('"')"#), Ok(branches_expr(r#"""#)));
    }

    #[test]
    fn test_parse_string_pattern() {
        assert_eq!(
            parse(r#"branches("foo")"#),
            Ok(RevsetExpression::branches(StringPattern::Substring(
                "foo".to_owned()
            )))
        );
        assert_eq!(
            parse(r#"branches(exact:"foo")"#),
            Ok(RevsetExpression::branches(StringPattern::Exact(
                "foo".to_owned()
            )))
        );
        assert_eq!(
            parse(r#"branches(substring:"foo")"#),
            Ok(RevsetExpression::branches(StringPattern::Substring(
                "foo".to_owned()
            )))
        );
        assert_eq!(
            parse(r#"branches("exact:foo")"#),
            Ok(RevsetExpression::branches(StringPattern::Substring(
                "exact:foo".to_owned()
            )))
        );
        assert_eq!(
            parse(r#"branches((exact:"foo"))"#),
            Ok(RevsetExpression::branches(StringPattern::Exact(
                "foo".to_owned()
            )))
        );
        assert_eq!(
            parse(r#"branches(exact:'\')"#),
            Ok(RevsetExpression::branches(StringPattern::Exact(
                r"\".to_owned()
            )))
        );
        assert_eq!(
            parse(r#"branches(bad:"foo")"#),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "branches".to_owned(),
                message: "Invalid string pattern".to_owned()
            })
        );
        assert_eq!(
            parse(r#"branches(exact::"foo")"#),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "branches".to_owned(),
                message: "Expected function argument of string pattern".to_owned()
            })
        );
        assert_eq!(
            parse(r#"branches(exact:"foo"+)"#),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "branches".to_owned(),
                message: "Expected function argument of string pattern".to_owned()
            })
        );
        assert_matches!(
            parse(r#"branches(exact:("foo"))"#),
            Err(RevsetParseErrorKind::NotInfixOperator { .. })
        );

        // String pattern isn't allowed at top level.
        assert_matches!(
            parse(r#"exact:"foo""#),
            Err(RevsetParseErrorKind::NotInfixOperator { .. })
        );
        assert_matches!(
            parse(r#"(exact:"foo")"#),
            Err(RevsetParseErrorKind::NotInfixOperator { .. })
        );
    }

    #[test]
    fn test_parse_revset_alias_symbol_decl() {
        let mut aliases_map = RevsetAliasesMap::new();
        // Working copy or remote symbol cannot be used as an alias name.
        assert!(aliases_map.insert("@", "none()").is_err());
        assert!(aliases_map.insert("a@", "none()").is_err());
        assert!(aliases_map.insert("a@b", "none()").is_err());
    }

    #[test]
    fn test_parse_revset_alias_formal_parameter() {
        let mut aliases_map = RevsetAliasesMap::new();
        // Working copy or remote symbol cannot be used as an parameter name.
        assert!(aliases_map.insert("f(@)", "none()").is_err());
        assert!(aliases_map.insert("f(a@)", "none()").is_err());
        assert!(aliases_map.insert("f(a@b)", "none()").is_err());
        // Trailing comma isn't allowed for empty parameter
        assert!(aliases_map.insert("f(,)", "none()").is_err());
        // Trailing comma is allowed for the last parameter
        assert!(aliases_map.insert("g(a,)", "none()").is_ok());
        assert!(aliases_map.insert("h(a ,  )", "none()").is_ok());
        assert!(aliases_map.insert("i(,a)", "none()").is_err());
        assert!(aliases_map.insert("j(a,,)", "none()").is_err());
        assert!(aliases_map.insert("k(a  , , )", "none()").is_err());
        assert!(aliases_map.insert("l(a,b,)", "none()").is_ok());
        assert!(aliases_map.insert("m(a,,b)", "none()").is_err());
    }

    #[test]
    fn test_parse_revset_compat_operator() {
        assert_eq!(
            parse(":foo"),
            Err(RevsetParseErrorKind::NotPrefixOperator {
                op: ":".to_owned(),
                similar_op: "::".to_owned(),
                description: "ancestors".to_owned(),
            })
        );
        assert_eq!(
            parse("foo^"),
            Err(RevsetParseErrorKind::NotPostfixOperator {
                op: "^".to_owned(),
                similar_op: "-".to_owned(),
                description: "parents".to_owned(),
            })
        );
        assert_eq!(
            parse("foo + bar"),
            Err(RevsetParseErrorKind::NotInfixOperator {
                op: "+".to_owned(),
                similar_op: "|".to_owned(),
                description: "union".to_owned(),
            })
        );
        assert_eq!(
            parse("foo - bar"),
            Err(RevsetParseErrorKind::NotInfixOperator {
                op: "-".to_owned(),
                similar_op: "~".to_owned(),
                description: "difference".to_owned(),
            })
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
        assert_eq!(parse("~x|y").unwrap(), parse("(~x)|y").unwrap());
        assert_eq!(parse("x&~y").unwrap(), parse("x&(~y)").unwrap());
        assert_eq!(parse("x~~y").unwrap(), parse("x~(~y)").unwrap());
        assert_eq!(parse("x~~~y").unwrap(), parse("x~(~(~y))").unwrap());
        assert_eq!(parse("~x::y").unwrap(), parse("~(x::y)").unwrap());
        assert_eq!(parse("x|y|z").unwrap(), parse("(x|y)|z").unwrap());
        assert_eq!(parse("x&y|z").unwrap(), parse("(x&y)|z").unwrap());
        assert_eq!(parse("x|y&z").unwrap(), parse("x|(y&z)").unwrap());
        assert_eq!(parse("x|y~z").unwrap(), parse("x|(y~z)").unwrap());
        assert_eq!(parse("::&..").unwrap(), parse("(::)&(..)").unwrap());
        // Parse repeated "ancestors"/"descendants"/"dag range"/"range" operators
        assert_eq!(parse("::foo::"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse(":::foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("::::foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo:::"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo::::"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo:::bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo::::bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("::foo::bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo::bar::"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("::::"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("....foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo...."), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo.....bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("..foo..bar"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("foo..bar.."), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("...."), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("::.."), Err(RevsetParseErrorKind::SyntaxError));
        // Parse combinations of "parents"/"children" operators and the range operators.
        // The former bind more strongly.
        assert_eq!(parse("foo-+"), Ok(foo_symbol.parents().children()));
        assert_eq!(parse("foo-::"), Ok(foo_symbol.parents().descendants()));
        assert_eq!(parse("::foo+"), Ok(foo_symbol.children().ancestors()));
        assert_eq!(parse("::-"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(parse("..+"), Err(RevsetParseErrorKind::SyntaxError));
    }

    #[test]
    fn test_parse_revset_function() {
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        assert_eq!(parse("parents(foo)"), Ok(foo_symbol.parents()));
        assert_eq!(parse("parents((foo))"), Ok(foo_symbol.parents()));
        assert_eq!(parse("parents(\"foo\")"), Ok(foo_symbol.parents()));
        assert_eq!(
            parse("ancestors(parents(foo))"),
            Ok(foo_symbol.parents().ancestors())
        );
        assert_eq!(parse("parents(foo"), Err(RevsetParseErrorKind::SyntaxError));
        assert_eq!(
            parse("parents(foo,foo)"),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "parents".to_string(),
                message: "Expected 1 arguments".to_string()
            })
        );
        assert_eq!(
            parse("root()"),
            Ok(Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::Root)))
        );
        assert!(parse("root(a)").is_err());
        assert_eq!(
            parse(r#"description("")"#),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description(StringPattern::Substring("".to_string()))
            ))
        );
        assert_eq!(
            parse("description(foo)"),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description(StringPattern::Substring("foo".to_string()))
            ))
        );
        assert_eq!(
            parse("description(visible_heads())"),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "description".to_string(),
                message: "Expected function argument of string pattern".to_string()
            })
        );
        assert_eq!(
            parse("description((foo))"),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description(StringPattern::Substring("foo".to_string()))
            ))
        );
        assert_eq!(
            parse("description(\"(foo)\")"),
            Ok(RevsetExpression::filter(
                RevsetFilterPredicate::Description(StringPattern::Substring("(foo)".to_string()))
            ))
        );
        assert!(parse("mine(foo)").is_err());
        assert_eq!(
            parse("mine()"),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
                StringPattern::Exact("test.user@example.com".to_string())
            )))
        );
        assert_eq!(
            parse_with_workspace("empty()", &WorkspaceId::default()),
            Ok(
                RevsetExpression::filter(RevsetFilterPredicate::File(FilesetExpression::all()))
                    .negated()
            )
        );
        assert!(parse_with_workspace("empty(foo)", &WorkspaceId::default()).is_err());
        assert!(parse_with_workspace("file()", &WorkspaceId::default()).is_err());
        assert_eq!(
            parse_with_workspace("file(foo)", &WorkspaceId::default()),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::File(
                FilesetExpression::prefix_path(RepoPathBuf::from_internal_string("foo"))
            )))
        );
        assert_eq!(
            parse_with_workspace(r#"file(file:"foo")"#, &WorkspaceId::default()),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::File(
                FilesetExpression::file_path(RepoPathBuf::from_internal_string("foo"))
            )))
        );
        assert_eq!(
            parse_with_workspace("file(foo, bar, baz)", &WorkspaceId::default()),
            Ok(RevsetExpression::filter(RevsetFilterPredicate::File(
                FilesetExpression::union_all(vec![
                    FilesetExpression::prefix_path(RepoPathBuf::from_internal_string("foo")),
                    FilesetExpression::prefix_path(RepoPathBuf::from_internal_string("bar")),
                    FilesetExpression::prefix_path(RepoPathBuf::from_internal_string("baz")),
                ])
            )))
        );
    }

    #[test]
    fn test_parse_revset_keyword_arguments() {
        assert_eq!(
            parse("remote_branches(remote=foo)").unwrap(),
            parse(r#"remote_branches("", foo)"#).unwrap(),
        );
        assert_eq!(
            parse("remote_branches(foo, remote=bar)").unwrap(),
            parse(r#"remote_branches(foo, bar)"#).unwrap(),
        );
        insta::assert_debug_snapshot!(
            parse(r#"remote_branches(remote=foo, bar)"#).unwrap_err(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_branches",
            message: "Positional argument follows keyword argument",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_branches("", foo, remote=bar)"#).unwrap_err(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_branches",
            message: "Got multiple values for keyword \"remote\"",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_branches(remote=bar, remote=bar)"#).unwrap_err(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_branches",
            message: "Got multiple values for keyword \"remote\"",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_branches(unknown=bar)"#).unwrap_err(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_branches",
            message: "Unexpected keyword argument \"unknown\"",
        }
        "###);
    }

    #[test]
    fn test_expand_symbol_alias() {
        assert_eq!(
            parse_with_aliases("AB|c", [("AB", "a|b")]).unwrap(),
            parse("(a|b)|c").unwrap()
        );
        assert_eq!(
            parse_with_aliases("AB::heads(AB)", [("AB", "a|b")]).unwrap(),
            parse("(a|b)::heads(a|b)").unwrap()
        );

        // Not string substitution 'a&b|c', but tree substitution.
        assert_eq!(
            parse_with_aliases("a&BC", [("BC", "b|c")]).unwrap(),
            parse("a&(b|c)").unwrap()
        );

        // String literal should not be substituted with alias.
        assert_eq!(
            parse_with_aliases(r#"A|"A"|'A'"#, [("A", "a")]).unwrap(),
            parse("a|A|A").unwrap()
        );

        // Alias can be substituted to string literal.
        assert_eq!(
            parse_with_aliases_and_workspace("file(A)", [("A", "a")], &WorkspaceId::default())
                .unwrap(),
            parse_with_workspace("file(a)", &WorkspaceId::default()).unwrap()
        );

        // Alias can be substituted to string pattern.
        assert_eq!(
            parse_with_aliases("author(A)", [("A", "a")]).unwrap(),
            parse("author(a)").unwrap()
        );
        assert_eq!(
            parse_with_aliases("author(A)", [("A", "exact:a")]).unwrap(),
            parse("author(exact:a)").unwrap()
        );

        // Part of string pattern cannot be substituted.
        assert_eq!(
            parse_with_aliases("author(exact:A)", [("A", "a")]).unwrap(),
            parse("author(exact:A)").unwrap()
        );

        // Part of @ symbol cannot be substituted.
        assert_eq!(
            parse_with_aliases("A@", [("A", "a")]).unwrap(),
            parse("A@").unwrap()
        );
        assert_eq!(
            parse_with_aliases("A@b", [("A", "a")]).unwrap(),
            parse("A@b").unwrap()
        );
        assert_eq!(
            parse_with_aliases("a@B", [("B", "b")]).unwrap(),
            parse("a@B").unwrap()
        );

        // Modifier cannot be substituted.
        assert_eq!(
            parse_with_aliases_and_modifier("all:all", [("all", "ALL")]).unwrap(),
            parse_with_modifier("all:ALL").unwrap()
        );

        // Multi-level substitution.
        assert_eq!(
            parse_with_aliases("A", [("A", "BC"), ("BC", "b|C"), ("C", "c")]).unwrap(),
            parse("b|c").unwrap()
        );

        // Infinite recursion, where the top-level error isn't of RecursiveAlias kind.
        assert_eq!(
            parse_with_aliases("A", [("A", "A")]),
            Err(RevsetParseErrorKind::BadAliasExpansion("A".to_owned()))
        );
        assert_eq!(
            parse_with_aliases("A", [("A", "B"), ("B", "b|C"), ("C", "c|B")]),
            Err(RevsetParseErrorKind::BadAliasExpansion("A".to_owned()))
        );

        // Error in alias definition.
        assert_eq!(
            parse_with_aliases("A", [("A", "a(")]),
            Err(RevsetParseErrorKind::BadAliasExpansion("A".to_owned()))
        );
        // Modifier isn't allowed in alias definition.
        assert_eq!(
            parse_with_aliases_and_modifier("A", [("A", "all:a")]),
            Err(RevsetParseErrorKind::BadAliasExpansion("A".to_owned()))
        );
    }

    #[test]
    fn test_expand_function_alias() {
        assert_eq!(
            parse_with_aliases("F()", [("F(  )", "a")]).unwrap(),
            parse("a").unwrap()
        );
        assert_eq!(
            parse_with_aliases("F(a)", [("F( x  )", "x")]).unwrap(),
            parse("a").unwrap()
        );
        assert_eq!(
            parse_with_aliases("F(a, b)", [("F( x,  y )", "x|y")]).unwrap(),
            parse("a|b").unwrap()
        );

        // Arguments should be resolved in the current scope.
        assert_eq!(
            parse_with_aliases("F(a::y,b::x)", [("F(x,y)", "x|y")]).unwrap(),
            parse("(a::y)|(b::x)").unwrap()
        );
        // F(a) -> G(a)&y -> (x|a)&y
        assert_eq!(
            parse_with_aliases("F(a)", [("F(x)", "G(x)&y"), ("G(y)", "x|y")]).unwrap(),
            parse("(x|a)&y").unwrap()
        );
        // F(G(a)) -> F(x|a) -> G(x|a)&y -> (x|(x|a))&y
        assert_eq!(
            parse_with_aliases("F(G(a))", [("F(x)", "G(x)&y"), ("G(y)", "x|y")]).unwrap(),
            parse("(x|(x|a))&y").unwrap()
        );

        // Function parameter should precede the symbol alias.
        assert_eq!(
            parse_with_aliases("F(a)|X", [("F(X)", "X"), ("X", "x")]).unwrap(),
            parse("a|x").unwrap()
        );

        // Function parameter shouldn't be expanded in symbol alias.
        assert_eq!(
            parse_with_aliases("F(a)", [("F(x)", "x|A"), ("A", "x")]).unwrap(),
            parse("a|x").unwrap()
        );

        // String literal should not be substituted with function parameter.
        assert_eq!(
            parse_with_aliases("F(a)", [("F(x)", r#"x|"x""#)]).unwrap(),
            parse("a|x").unwrap()
        );

        // Pass string literal as parameter.
        assert_eq!(
            parse_with_aliases("F(a)", [("F(x)", "author(x)|committer(x)")]).unwrap(),
            parse("author(a)|committer(a)").unwrap()
        );

        // Function and symbol aliases reside in separate namespaces.
        assert_eq!(
            parse_with_aliases("A()", [("A()", "A"), ("A", "a")]).unwrap(),
            parse("a").unwrap()
        );

        // Invalid number of arguments.
        assert_eq!(
            parse_with_aliases("F(a)", [("F()", "x")]),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 0 arguments".to_owned()
            })
        );
        assert_eq!(
            parse_with_aliases("F()", [("F(x)", "x")]),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 1 arguments".to_owned()
            })
        );
        assert_eq!(
            parse_with_aliases("F(a,b,c)", [("F(x,y)", "x|y")]),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 2 arguments".to_owned()
            })
        );

        // Keyword argument isn't supported for now.
        assert_eq!(
            parse_with_aliases("F(x=y)", [("F(x)", "x")]),
            Err(RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Unexpected keyword arguments".to_owned()
            })
        );

        // Infinite recursion, where the top-level error isn't of RecursiveAlias kind.
        assert_eq!(
            parse_with_aliases(
                "F(a)",
                [("F(x)", "G(x)"), ("G(x)", "H(x)"), ("H(x)", "F(x)")]
            ),
            Err(RevsetParseErrorKind::BadAliasExpansion("F()".to_owned()))
        );
    }

    #[test]
    fn test_optimize_subtree() {
        // Check that transform_expression_bottom_up() never rewrites enum variant
        // (e.g. Range -> DagRange) nor reorders arguments unintentionally.

        assert_eq!(
            optimize(parse("parents(branches() & all())").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).parents()
        );
        assert_eq!(
            optimize(parse("children(branches() & all())").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).children()
        );
        assert_eq!(
            optimize(parse("ancestors(branches() & all())").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).ancestors()
        );
        assert_eq!(
            optimize(parse("descendants(branches() & all())").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).descendants()
        );

        assert_eq!(
            optimize(parse("(branches() & all())..(all() & tags())").unwrap()),
            RevsetExpression::branches(StringPattern::everything())
                .range(&RevsetExpression::tags())
        );
        assert_eq!(
            optimize(parse("(branches() & all())::(all() & tags())").unwrap()),
            RevsetExpression::branches(StringPattern::everything())
                .dag_range_to(&RevsetExpression::tags())
        );

        assert_eq!(
            optimize(parse("heads(branches() & all())").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).heads()
        );
        assert_eq!(
            optimize(parse("roots(branches() & all())").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).roots()
        );

        assert_eq!(
            optimize(parse("latest(branches() & all(), 2)").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).latest(2)
        );

        assert_eq!(
            optimize(parse("present(foo ~ bar)").unwrap()),
            Rc::new(RevsetExpression::Present(
                RevsetExpression::symbol("foo".to_owned())
                    .minus(&RevsetExpression::symbol("bar".to_owned()))
            ))
        );
        assert_eq!(
            optimize(parse("present(branches() & all())").unwrap()),
            Rc::new(RevsetExpression::Present(RevsetExpression::branches(
                StringPattern::everything()
            )))
        );

        assert_eq!(
            optimize(parse("~branches() & all()").unwrap()),
            RevsetExpression::branches(StringPattern::everything()).negated()
        );
        assert_eq!(
            optimize(parse("(branches() & all()) | (all() & tags())").unwrap()),
            RevsetExpression::branches(StringPattern::everything())
                .union(&RevsetExpression::tags())
        );
        assert_eq!(
            optimize(parse("(branches() & all()) & (all() & tags())").unwrap()),
            RevsetExpression::branches(StringPattern::everything())
                .intersection(&RevsetExpression::tags())
        );
        assert_eq!(
            optimize(parse("(branches() & all()) ~ (all() & tags())").unwrap()),
            RevsetExpression::branches(StringPattern::everything())
                .minus(&RevsetExpression::tags())
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

        // Only left subtree should be rewritten.
        let parsed = parse("(branches() & all()) | tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert_eq!(
            unwrap_union(&optimized).0.as_ref(),
            &RevsetExpression::CommitRef(RevsetCommitRef::Branches(StringPattern::everything()))
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
        assert_eq!(
            unwrap_union(&optimized).1.as_ref(),
            &RevsetExpression::CommitRef(RevsetCommitRef::Tags),
        );
    }

    #[test]
    fn test_optimize_difference() {
        insta::assert_debug_snapshot!(optimize(parse("foo & ~bar").unwrap()), @r###"
        Difference(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~foo & bar").unwrap()), @r###"
        Difference(
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~foo & bar & ~baz").unwrap()), @r###"
        Difference(
            Difference(
                CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
            ),
            CommitRef(
                Symbol(
                    "baz",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(all() & ~foo) & bar").unwrap()), @r###"
        Difference(
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
        )
        "###);

        // Binary difference operation should go through the same optimization passes.
        insta::assert_debug_snapshot!(optimize(parse("all() ~ foo").unwrap()), @r###"
        NotIn(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo ~ bar").unwrap()), @r###"
        Difference(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(all() ~ foo) & bar").unwrap()), @r###"
        Difference(
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
        )
        "###);

        // Range expression.
        insta::assert_debug_snapshot!(optimize(parse("::foo & ~::bar").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            heads: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~::foo & ::bar").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            heads: CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo..").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            heads: CommitRef(
                VisibleHeads,
            ),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo..bar").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            heads: CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            generation: 0..18446744073709551615,
        }
        "###);

        // Double/triple negates.
        insta::assert_debug_snapshot!(optimize(parse("foo & ~~bar").unwrap()), @r###"
        Intersection(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo & ~~~bar").unwrap()), @r###"
        Difference(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~(all() & ~foo) & bar").unwrap()), @r###"
        Intersection(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        )
        "###);

        // Should be better than '(all() & ~foo) & (all() & ~bar)'.
        insta::assert_debug_snapshot!(optimize(parse("~foo & ~bar").unwrap()), @r###"
        Difference(
            NotIn(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
            ),
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        )
        "###);
    }

    #[test]
    fn test_optimize_not_in_ancestors() {
        // '~(::foo)' is equivalent to 'foo..'.
        insta::assert_debug_snapshot!(optimize(parse("~(::foo)").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            heads: CommitRef(
                VisibleHeads,
            ),
            generation: 0..18446744073709551615,
        }
        "###);

        // '~(::foo-)' is equivalent to 'foo-..'.
        insta::assert_debug_snapshot!(optimize(parse("~(::foo-)").unwrap()), @r###"
        Range {
            roots: Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 1..2,
            },
            heads: CommitRef(
                VisibleHeads,
            ),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~(::foo--)").unwrap()), @r###"
        Range {
            roots: Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 2..3,
            },
            heads: CommitRef(
                VisibleHeads,
            ),
            generation: 0..18446744073709551615,
        }
        "###);

        // Bounded ancestors shouldn't be substituted.
        insta::assert_debug_snapshot!(optimize(parse("~ancestors(foo, 1)").unwrap()), @r###"
        NotIn(
            Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 0..1,
            },
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~ancestors(foo-, 1)").unwrap()), @r###"
        NotIn(
            Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 1..2,
            },
        )
        "###);
    }

    #[test]
    fn test_optimize_filter_difference() {
        // '~empty()' -> '~~file(*)' -> 'file(*)'
        insta::assert_debug_snapshot!(optimize(parse("~empty()").unwrap()), @r###"
        Filter(
            File(
                All,
            ),
        )
        "###);

        // '& baz' can be moved into the filter node, and form a difference node.
        insta::assert_debug_snapshot!(
            optimize(parse("(author(foo) & ~bar) & baz").unwrap()), @r###"
        Intersection(
            Difference(
                CommitRef(
                    Symbol(
                        "baz",
                    ),
                ),
                CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "foo",
                    ),
                ),
            ),
        )
        "###);

        // '~set & filter()' shouldn't be substituted.
        insta::assert_debug_snapshot!(
            optimize(parse("~foo & author(bar)").unwrap()), @r###"
        Intersection(
            NotIn(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "bar",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("~foo & (author(bar) | baz)").unwrap()), @r###"
        Intersection(
            NotIn(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
            ),
            AsFilter(
                Union(
                    Filter(
                        Author(
                            Substring(
                                "bar",
                            ),
                        ),
                    ),
                    CommitRef(
                        Symbol(
                            "baz",
                        ),
                    ),
                ),
            ),
        )
        "###);

        // Filter should be moved right of the intersection.
        insta::assert_debug_snapshot!(
            optimize(parse("author(foo) ~ bar").unwrap()), @r###"
        Intersection(
            NotIn(
                CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "foo",
                    ),
                ),
            ),
        )
        "###);
    }

    #[test]
    fn test_optimize_filter_intersection() {
        insta::assert_debug_snapshot!(optimize(parse("author(foo)").unwrap()), @r###"
        Filter(
            Author(
                Substring(
                    "foo",
                ),
            ),
        )
        "###);

        insta::assert_debug_snapshot!(optimize(parse("foo & description(bar)").unwrap()), @r###"
        Intersection(
            CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            Filter(
                Description(
                    Substring(
                        "bar",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("author(foo) & bar").unwrap()), @r###"
        Intersection(
            CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "foo",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("author(foo) & committer(bar)").unwrap()), @r###"
        Intersection(
            Filter(
                Author(
                    Substring(
                        "foo",
                    ),
                ),
            ),
            Filter(
                Committer(
                    Substring(
                        "bar",
                    ),
                ),
            ),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & author(baz)").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                Filter(
                    Description(
                        Substring(
                            "bar",
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "baz",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("committer(foo) & bar & author(baz)").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
                Filter(
                    Committer(
                        Substring(
                            "foo",
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "baz",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse_with_workspace("committer(foo) & file(bar) & baz", &WorkspaceId::default()).unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "baz",
                    ),
                ),
                Filter(
                    Committer(
                        Substring(
                            "foo",
                        ),
                    ),
                ),
            ),
            Filter(
                File(
                    Pattern(
                        PrefixPath(
                            "bar",
                        ),
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse_with_workspace("committer(foo) & file(bar) & author(baz)", &WorkspaceId::default()).unwrap()), @r###"
        Intersection(
            Intersection(
                Filter(
                    Committer(
                        Substring(
                            "foo",
                        ),
                    ),
                ),
                Filter(
                    File(
                        Pattern(
                            PrefixPath(
                                "bar",
                            ),
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "baz",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse_with_workspace("foo & file(bar) & baz", &WorkspaceId::default()).unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                CommitRef(
                    Symbol(
                        "baz",
                    ),
                ),
            ),
            Filter(
                File(
                    Pattern(
                        PrefixPath(
                            "bar",
                        ),
                    ),
                ),
            ),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & author(baz) & qux").unwrap()), @r###"
        Intersection(
            Intersection(
                Intersection(
                    CommitRef(
                        Symbol(
                            "foo",
                        ),
                    ),
                    CommitRef(
                        Symbol(
                            "qux",
                        ),
                    ),
                ),
                Filter(
                    Description(
                        Substring(
                            "bar",
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "baz",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & parents(author(baz)) & qux").unwrap()), @r###"
        Intersection(
            Intersection(
                Intersection(
                    CommitRef(
                        Symbol(
                            "foo",
                        ),
                    ),
                    Ancestors {
                        heads: Filter(
                            Author(
                                Substring(
                                    "baz",
                                ),
                            ),
                        ),
                        generation: 1..2,
                    },
                ),
                CommitRef(
                    Symbol(
                        "qux",
                    ),
                ),
            ),
            Filter(
                Description(
                    Substring(
                        "bar",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & parents(author(baz) & qux)").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                Ancestors {
                    heads: Intersection(
                        CommitRef(
                            Symbol(
                                "qux",
                            ),
                        ),
                        Filter(
                            Author(
                                Substring(
                                    "baz",
                                ),
                            ),
                        ),
                    ),
                    generation: 1..2,
                },
            ),
            Filter(
                Description(
                    Substring(
                        "bar",
                    ),
                ),
            ),
        )
        "###);

        // Symbols have to be pushed down to the innermost filter node.
        insta::assert_debug_snapshot!(
            optimize(parse("(a & author(A)) & (b & author(B)) & (c & author(C))").unwrap()), @r###"
        Intersection(
            Intersection(
                Intersection(
                    Intersection(
                        Intersection(
                            CommitRef(
                                Symbol(
                                    "a",
                                ),
                            ),
                            CommitRef(
                                Symbol(
                                    "b",
                                ),
                            ),
                        ),
                        CommitRef(
                            Symbol(
                                "c",
                            ),
                        ),
                    ),
                    Filter(
                        Author(
                            Substring(
                                "A",
                            ),
                        ),
                    ),
                ),
                Filter(
                    Author(
                        Substring(
                            "B",
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "C",
                    ),
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("(a & author(A)) & ((b & author(B)) & (c & author(C))) & d").unwrap()),
            @r###"
        Intersection(
            Intersection(
                Intersection(
                    Intersection(
                        Intersection(
                            CommitRef(
                                Symbol(
                                    "a",
                                ),
                            ),
                            Intersection(
                                CommitRef(
                                    Symbol(
                                        "b",
                                    ),
                                ),
                                CommitRef(
                                    Symbol(
                                        "c",
                                    ),
                                ),
                            ),
                        ),
                        CommitRef(
                            Symbol(
                                "d",
                            ),
                        ),
                    ),
                    Filter(
                        Author(
                            Substring(
                                "A",
                            ),
                        ),
                    ),
                ),
                Filter(
                    Author(
                        Substring(
                            "B",
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "C",
                    ),
                ),
            ),
        )
        "###);

        // 'all()' moves in to 'filter()' first, so 'A & filter()' can be found.
        insta::assert_debug_snapshot!(
            optimize(parse("foo & (all() & description(bar)) & (author(baz) & all())").unwrap()),
            @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                Filter(
                    Description(
                        Substring(
                            "bar",
                        ),
                    ),
                ),
            ),
            Filter(
                Author(
                    Substring(
                        "baz",
                    ),
                ),
            ),
        )
        "###);
    }

    #[test]
    fn test_optimize_filter_subtree() {
        insta::assert_debug_snapshot!(
            optimize(parse("(author(foo) | bar) & baz").unwrap()), @r###"
        Intersection(
            CommitRef(
                Symbol(
                    "baz",
                ),
            ),
            AsFilter(
                Union(
                    Filter(
                        Author(
                            Substring(
                                "foo",
                            ),
                        ),
                    ),
                    CommitRef(
                        Symbol(
                            "bar",
                        ),
                    ),
                ),
            ),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("(foo | committer(bar)) & description(baz) & qux").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(
                    Symbol(
                        "qux",
                    ),
                ),
                AsFilter(
                    Union(
                        CommitRef(
                            Symbol(
                                "foo",
                            ),
                        ),
                        Filter(
                            Committer(
                                Substring(
                                    "bar",
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            Filter(
                Description(
                    Substring(
                        "baz",
                    ),
                ),
            ),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("(~present(author(foo) & bar) | baz) & qux").unwrap()), @r###"
        Intersection(
            CommitRef(
                Symbol(
                    "qux",
                ),
            ),
            AsFilter(
                Union(
                    AsFilter(
                        NotIn(
                            AsFilter(
                                Present(
                                    Intersection(
                                        CommitRef(
                                            Symbol(
                                                "bar",
                                            ),
                                        ),
                                        Filter(
                                            Author(
                                                Substring(
                                                    "foo",
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                    CommitRef(
                        Symbol(
                            "baz",
                        ),
                    ),
                ),
            ),
        )
        "###);

        // Symbols have to be pushed down to the innermost filter node.
        insta::assert_debug_snapshot!(
            optimize(parse(
                "(a & (author(A) | 0)) & (b & (author(B) | 1)) & (c & (author(C) | 2))").unwrap()),
            @r###"
        Intersection(
            Intersection(
                Intersection(
                    Intersection(
                        Intersection(
                            CommitRef(
                                Symbol(
                                    "a",
                                ),
                            ),
                            CommitRef(
                                Symbol(
                                    "b",
                                ),
                            ),
                        ),
                        CommitRef(
                            Symbol(
                                "c",
                            ),
                        ),
                    ),
                    AsFilter(
                        Union(
                            Filter(
                                Author(
                                    Substring(
                                        "A",
                                    ),
                                ),
                            ),
                            CommitRef(
                                Symbol(
                                    "0",
                                ),
                            ),
                        ),
                    ),
                ),
                AsFilter(
                    Union(
                        Filter(
                            Author(
                                Substring(
                                    "B",
                                ),
                            ),
                        ),
                        CommitRef(
                            Symbol(
                                "1",
                            ),
                        ),
                    ),
                ),
            ),
            AsFilter(
                Union(
                    Filter(
                        Author(
                            Substring(
                                "C",
                            ),
                        ),
                    ),
                    CommitRef(
                        Symbol(
                            "2",
                        ),
                    ),
                ),
            ),
        )
        "###);
    }

    #[test]
    fn test_optimize_ancestors() {
        // Typical scenario: fold nested parents()
        insta::assert_debug_snapshot!(optimize(parse("foo--").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 2..3,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("::(foo---)").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 3..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(::foo)---").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 3..18446744073709551615,
        }
        "###);

        // 'foo-+' is not 'foo'.
        insta::assert_debug_snapshot!(optimize(parse("foo---+").unwrap()), @r###"
        Descendants {
            roots: Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 3..4,
            },
            generation: 1..2,
        }
        "###);

        // For 'roots..heads', heads can be folded.
        insta::assert_debug_snapshot!(optimize(parse("foo..(bar--)").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            heads: CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            generation: 2..18446744073709551615,
        }
        "###);
        // roots can also be folded, and the range expression is reconstructed.
        insta::assert_debug_snapshot!(optimize(parse("(foo--)..(bar---)").unwrap()), @r###"
        Range {
            roots: Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 2..3,
            },
            heads: CommitRef(
                Symbol(
                    "bar",
                ),
            ),
            generation: 3..18446744073709551615,
        }
        "###);
        // Bounded ancestors shouldn't be substituted to range.
        insta::assert_debug_snapshot!(
            optimize(parse("~ancestors(foo, 2) & ::bar").unwrap()), @r###"
        Difference(
            Ancestors {
                heads: CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
                generation: 0..18446744073709551615,
            },
            Ancestors {
                heads: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 0..2,
            },
        )
        "###);

        // If inner range is bounded by roots, it cannot be merged.
        // e.g. '..(foo..foo)' is equivalent to '..none()', not to '..foo'
        insta::assert_debug_snapshot!(optimize(parse("(foo..bar)--").unwrap()), @r###"
        Ancestors {
            heads: Range {
                roots: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                heads: CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
                generation: 0..18446744073709551615,
            },
            generation: 2..3,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo..(bar..baz)").unwrap()), @r###"
        Range {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            heads: Range {
                roots: CommitRef(
                    Symbol(
                        "bar",
                    ),
                ),
                heads: CommitRef(
                    Symbol(
                        "baz",
                    ),
                ),
                generation: 0..18446744073709551615,
            },
            generation: 0..18446744073709551615,
        }
        "###);

        // Ancestors of empty generation range should be empty.
        insta::assert_debug_snapshot!(
            optimize(parse("ancestors(ancestors(foo), 0)").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 0..0,
        }
        "###
        );
        insta::assert_debug_snapshot!(
            optimize(parse("ancestors(ancestors(foo, 0))").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 0..0,
        }
        "###
        );
    }

    #[test]
    fn test_optimize_descendants() {
        // Typical scenario: fold nested children()
        insta::assert_debug_snapshot!(optimize(parse("foo++").unwrap()), @r###"
        Descendants {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 2..3,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(foo+++)::").unwrap()), @r###"
        Descendants {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 3..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(foo::)+++").unwrap()), @r###"
        Descendants {
            roots: CommitRef(
                Symbol(
                    "foo",
                ),
            ),
            generation: 3..18446744073709551615,
        }
        "###);

        // 'foo+-' is not 'foo'.
        insta::assert_debug_snapshot!(optimize(parse("foo+++-").unwrap()), @r###"
        Ancestors {
            heads: Descendants {
                roots: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 3..4,
            },
            generation: 1..2,
        }
        "###);

        // TODO: Inner Descendants can be folded into DagRange. Perhaps, we can rewrite
        // 'x::y' to 'x:: & ::y' first, so the common substitution rule can handle both
        // 'x+::y' and 'x+ & ::y'.
        insta::assert_debug_snapshot!(optimize(parse("(foo++)::bar").unwrap()), @r###"
        DagRange {
            roots: Descendants {
                roots: CommitRef(
                    Symbol(
                        "foo",
                    ),
                ),
                generation: 2..3,
            },
            heads: CommitRef(
                Symbol(
                    "bar",
                ),
            ),
        }
        "###);
    }
}
