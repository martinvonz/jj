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
use std::collections::hash_map;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use itertools::Itertools;
use once_cell::sync::Lazy;
use thiserror::Error;

use crate::backend::BackendError;
use crate::backend::BackendResult;
use crate::backend::ChangeId;
use crate::backend::CommitId;
use crate::commit::Commit;
use crate::dsl_util;
use crate::dsl_util::collect_similar;
use crate::dsl_util::AliasExpandError as _;
use crate::fileset;
use crate::fileset::FilesetExpression;
use crate::graph::GraphEdge;
use crate::hex_util::to_forward_hex;
use crate::id_prefix::IdPrefixContext;
use crate::object_id::HexPrefix;
use crate::object_id::PrefixResolution;
use crate::op_store::RemoteRefState;
use crate::op_store::WorkspaceId;
use crate::repo::Repo;
use crate::repo_path::RepoPathUiConverter;
use crate::revset_parser;
pub use crate::revset_parser::expect_literal;
pub use crate::revset_parser::BinaryOp;
pub use crate::revset_parser::ExpressionKind;
pub use crate::revset_parser::ExpressionNode;
pub use crate::revset_parser::FunctionCallNode;
pub use crate::revset_parser::RevsetAliasesMap;
pub use crate::revset_parser::RevsetParseError;
pub use crate::revset_parser::RevsetParseErrorKind;
pub use crate::revset_parser::UnaryOp;
use crate::store::Store;
use crate::str_util::StringPattern;
use crate::time_util::DatePattern;
use crate::time_util::DatePatternContext;

/// Error occurred during symbol resolution.
#[derive(Debug, Error)]
pub enum RevsetResolutionError {
    #[error("Revision \"{name}\" doesn't exist")]
    NoSuchRevision {
        name: String,
        candidates: Vec<String>,
    },
    #[error("Workspace \"{name}\" doesn't have a working-copy commit")]
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
#[derive(Clone, Debug)]
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
    Bookmarks(StringPattern),
    RemoteBookmarks {
        bookmark_pattern: StringPattern,
        remote_pattern: StringPattern,
        remote_ref_state: Option<RemoteRefState>,
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

#[derive(Clone, Debug)]
pub enum RevsetFilterPredicate {
    /// Commits with number of parents in the range.
    ParentCount(Range<u32>),
    /// Commits with description matching the pattern.
    Description(StringPattern),
    /// Commits with author name or email matching the pattern.
    Author(StringPattern),
    /// Commits with committer name or email matching the pattern.
    Committer(StringPattern),
    /// Commits with author dates matching the given date pattern.
    AuthorDate(DatePattern),
    /// Commits with committer dates matching the given date pattern.
    CommitterDate(DatePattern),
    /// Commits modifying the paths specified by the fileset.
    File(FilesetExpression),
    /// Commits containing diffs matching the `text` pattern within the `files`.
    DiffContains {
        text: StringPattern,
        files: FilesetExpression,
    },
    /// Commits with conflicts
    HasConflict,
    /// Custom predicates provided by extensions
    Extension(Rc<dyn RevsetFilterExtension>),
}

#[derive(Clone, Debug)]
pub enum RevsetExpression {
    None,
    All,
    Commits(Vec<CommitId>),
    CommitRef(RevsetCommitRef),
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

    pub fn bookmarks(pattern: StringPattern) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(RevsetCommitRef::Bookmarks(
            pattern,
        )))
    }

    pub fn remote_bookmarks(
        bookmark_pattern: StringPattern,
        remote_pattern: StringPattern,
        remote_ref_state: Option<RemoteRefState>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::CommitRef(
            RevsetCommitRef::RemoteBookmarks {
                bookmark_pattern,
                remote_pattern,
                remote_ref_state,
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
        self.ancestors_at(1)
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
        self.descendants_at(1)
    }

    /// Descendants of `self`, including `self`.
    pub fn descendants(self: &Rc<RevsetExpression>) -> Rc<RevsetExpression> {
        self.descendants_range(GENERATION_RANGE_FULL)
    }

    /// Descendants of `self` at an offset of `generation` ahead of `self`.
    /// The `generation` offset is zero-based starting from `self`.
    pub fn descendants_at(self: &Rc<RevsetExpression>, generation: u64) -> Rc<RevsetExpression> {
        self.descendants_range(generation..(generation + 1))
    }

    /// Descendants of `self` in the given range.
    pub fn descendants_range(
        self: &Rc<RevsetExpression>,
        generation_range: Range<u64>,
    ) -> Rc<RevsetExpression> {
        Rc::new(RevsetExpression::Descendants {
            roots: self.clone(),
            generation: generation_range,
        })
    }

    /// Filter all commits by `predicate` in `self`.
    pub fn filtered(
        self: &Rc<RevsetExpression>,
        predicate: RevsetFilterPredicate,
    ) -> Rc<RevsetExpression> {
        self.intersection(&RevsetExpression::filter(predicate))
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
    /// expression must not contain any symbols (bookmarks, tags, change/commit
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

#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
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

pub type RevsetFunction =
    fn(&FunctionCallNode, &RevsetParseContext) -> Result<Rc<RevsetExpression>, RevsetParseError>;

static BUILTIN_FUNCTION_MAP: Lazy<HashMap<&'static str, RevsetFunction>> = Lazy::new(|| {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map: HashMap<&'static str, RevsetFunction> = HashMap::new();
    map.insert("parents", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = lower_expression(arg, context)?;
        Ok(expression.parents())
    });
    map.insert("children", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = lower_expression(arg, context)?;
        Ok(expression.children())
    });
    map.insert("ancestors", |function, context| {
        let ([heads_arg], [depth_opt_arg]) = function.expect_arguments()?;
        let heads = lower_expression(heads_arg, context)?;
        let generation = if let Some(depth_arg) = depth_opt_arg {
            let depth = expect_literal("integer", depth_arg)?;
            0..depth
        } else {
            GENERATION_RANGE_FULL
        };
        Ok(heads.ancestors_range(generation))
    });
    map.insert("descendants", |function, context| {
        let ([roots_arg], [depth_opt_arg]) = function.expect_arguments()?;
        let roots = lower_expression(roots_arg, context)?;
        let generation = if let Some(depth_arg) = depth_opt_arg {
            let depth = expect_literal("integer", depth_arg)?;
            0..depth
        } else {
            GENERATION_RANGE_FULL
        };
        Ok(roots.descendants_range(generation))
    });
    map.insert("connected", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let candidates = lower_expression(arg, context)?;
        Ok(candidates.connected())
    });
    map.insert("reachable", |function, context| {
        let [source_arg, domain_arg] = function.expect_exact_arguments()?;
        let sources = lower_expression(source_arg, context)?;
        let domain = lower_expression(domain_arg, context)?;
        Ok(sources.reachable(&domain))
    });
    map.insert("none", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::none())
    });
    map.insert("all", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::all())
    });
    map.insert("working_copies", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::working_copies())
    });
    map.insert("heads", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let candidates = lower_expression(arg, context)?;
        Ok(candidates.heads())
    });
    map.insert("roots", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let candidates = lower_expression(arg, context)?;
        Ok(candidates.roots())
    });
    map.insert("visible_heads", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::visible_heads())
    });
    map.insert("root", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::root())
    });
    map.insert("bookmarks", |function, _context| {
        let ([], [opt_arg]) = function.expect_arguments()?;
        let pattern = if let Some(arg) = opt_arg {
            expect_string_pattern(arg)?
        } else {
            StringPattern::everything()
        };
        Ok(RevsetExpression::bookmarks(pattern))
    });
    map.insert("remote_bookmarks", |function, _context| {
        parse_remote_bookmarks_arguments(function, None)
    });
    map.insert("tracked_remote_bookmarks", |function, _context| {
        parse_remote_bookmarks_arguments(function, Some(RemoteRefState::Tracking))
    });
    map.insert("untracked_remote_bookmarks", |function, _context| {
        parse_remote_bookmarks_arguments(function, Some(RemoteRefState::New))
    });

    // TODO: Remove in jj 0.28+
    map.insert("branches", map["bookmarks"]);
    map.insert("remote_branches", map["remote_bookmarks"]);
    map.insert("tracked_remote_branches", map["tracked_remote_bookmarks"]);
    map.insert(
        "untracked_remote_branches",
        map["untracked_remote_bookmarks"],
    );

    map.insert("tags", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::tags())
    });
    map.insert("git_refs", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::git_refs())
    });
    map.insert("git_head", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::git_head())
    });
    map.insert("latest", |function, context| {
        let ([candidates_arg], [count_opt_arg]) = function.expect_arguments()?;
        let candidates = lower_expression(candidates_arg, context)?;
        let count = if let Some(count_arg) = count_opt_arg {
            expect_literal("integer", count_arg)?
        } else {
            1
        };
        Ok(candidates.latest(count))
    });
    map.insert("merges", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::filter(
            RevsetFilterPredicate::ParentCount(2..u32::MAX),
        ))
    });
    map.insert("description", |function, _context| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = expect_string_pattern(arg)?;
        Ok(RevsetExpression::filter(
            RevsetFilterPredicate::Description(pattern),
        ))
    });
    map.insert("author", |function, _context| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = expect_string_pattern(arg)?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
            pattern,
        )))
    });
    map.insert("author_date", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = expect_date_pattern(arg, context.date_pattern_context())?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::AuthorDate(
            pattern,
        )))
    });
    map.insert("mine", |function, context| {
        function.expect_no_arguments()?;
        // Email address domains are inherently case‐insensitive, and the local‐parts
        // are generally (although not universally) treated as case‐insensitive too, so
        // we use a case‐insensitive match here.
        Ok(RevsetExpression::filter(RevsetFilterPredicate::Author(
            StringPattern::exact_i(&context.user_email),
        )))
    });
    map.insert("committer", |function, _context| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = expect_string_pattern(arg)?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::Committer(
            pattern,
        )))
    });
    map.insert("committer_date", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let pattern = expect_date_pattern(arg, context.date_pattern_context())?;
        Ok(RevsetExpression::filter(
            RevsetFilterPredicate::CommitterDate(pattern),
        ))
    });
    map.insert("empty", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::is_empty())
    });
    map.insert("file", |function, context| {
        let ctx = context.workspace.as_ref().ok_or_else(|| {
            RevsetParseError::with_span(
                RevsetParseErrorKind::FsPathWithoutWorkspace,
                function.args_span, // TODO: better to use name_span?
            )
        })?;
        // TODO: emit deprecation warning if multiple arguments are passed
        let ([arg], args) = function.expect_some_arguments()?;
        let file_expressions = itertools::chain([arg], args)
            .map(|arg| expect_fileset_expression(arg, ctx.path_converter))
            .try_collect()?;
        let expr = FilesetExpression::union_all(file_expressions);
        Ok(RevsetExpression::filter(RevsetFilterPredicate::File(expr)))
    });
    map.insert("diff_contains", |function, context| {
        let ([text_arg], [files_opt_arg]) = function.expect_arguments()?;
        let text = expect_string_pattern(text_arg)?;
        let files = if let Some(files_arg) = files_opt_arg {
            let ctx = context.workspace.as_ref().ok_or_else(|| {
                RevsetParseError::with_span(
                    RevsetParseErrorKind::FsPathWithoutWorkspace,
                    files_arg.span,
                )
            })?;
            expect_fileset_expression(files_arg, ctx.path_converter)?
        } else {
            // TODO: defaults to CLI path arguments?
            // https://github.com/martinvonz/jj/issues/2933#issuecomment-1925870731
            FilesetExpression::all()
        };
        Ok(RevsetExpression::filter(
            RevsetFilterPredicate::DiffContains { text, files },
        ))
    });
    map.insert("conflict", |function, _context| {
        function.expect_no_arguments()?;
        Ok(RevsetExpression::filter(RevsetFilterPredicate::HasConflict))
    });
    map.insert("present", |function, context| {
        let [arg] = function.expect_exact_arguments()?;
        let expression = lower_expression(arg, context)?;
        Ok(Rc::new(RevsetExpression::Present(expression)))
    });
    map
});

/// Parses the given `node` as a fileset expression.
pub fn expect_fileset_expression(
    node: &ExpressionNode,
    path_converter: &RepoPathUiConverter,
) -> Result<FilesetExpression, RevsetParseError> {
    // Alias handling is a bit tricky. The outermost expression `alias` is
    // substituted, but inner expressions `x & alias` aren't. If this seemed
    // weird, we can either transform AST or turn off revset aliases completely.
    revset_parser::expect_expression_with(node, |node| {
        fileset::parse(node.span.as_str(), path_converter).map_err(|err| {
            RevsetParseError::expression("Invalid fileset expression", node.span).with_source(err)
        })
    })
}

pub fn expect_string_pattern(node: &ExpressionNode) -> Result<StringPattern, RevsetParseError> {
    let parse_pattern = |value: &str, kind: Option<&str>| match kind {
        Some(kind) => StringPattern::from_str_kind(value, kind),
        None => Ok(StringPattern::Substring(value.to_owned())),
    };
    revset_parser::expect_pattern_with("string pattern", node, parse_pattern)
}

pub fn expect_date_pattern(
    node: &ExpressionNode,
    context: &DatePatternContext,
) -> Result<DatePattern, RevsetParseError> {
    let parse_pattern =
        |value: &str, kind: Option<&str>| -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
            match kind {
                None => Err("Date pattern must specify 'after' or 'before'".into()),
                Some(kind) => Ok(context.parse_relative(value, kind)?),
            }
        };
    revset_parser::expect_pattern_with("date pattern", node, parse_pattern)
}

fn parse_remote_bookmarks_arguments(
    function: &FunctionCallNode,
    remote_ref_state: Option<RemoteRefState>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let ([], [bookmark_opt_arg, remote_opt_arg]) =
        function.expect_named_arguments(&["", "remote"])?;
    let bookmark_pattern = if let Some(bookmark_arg) = bookmark_opt_arg {
        expect_string_pattern(bookmark_arg)?
    } else {
        StringPattern::everything()
    };
    let remote_pattern = if let Some(remote_arg) = remote_opt_arg {
        expect_string_pattern(remote_arg)?
    } else {
        StringPattern::everything()
    };
    Ok(RevsetExpression::remote_bookmarks(
        bookmark_pattern,
        remote_pattern,
        remote_ref_state,
    ))
}

/// Resolves function call by using the given function map.
fn lower_function_call(
    function: &FunctionCallNode,
    context: &RevsetParseContext,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let function_map = &context.extensions.function_map;
    if let Some(func) = function_map.get(function.name) {
        func(function, context)
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::NoSuchFunction {
                name: function.name.to_owned(),
                candidates: collect_similar(function.name, function_map.keys()),
            },
            function.name_span,
        ))
    }
}

/// Transforms the given AST `node` into expression that describes DAG
/// operation. Function calls will be resolved at this stage.
pub fn lower_expression(
    node: &ExpressionNode,
    context: &RevsetParseContext,
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
            let ctx = context.workspace.as_ref().ok_or_else(|| {
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
            let arg = lower_expression(arg_node, context)?;
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
            let lhs = lower_expression(lhs_node, context)?;
            let rhs = lower_expression(rhs_node, context)?;
            match op {
                BinaryOp::Intersection => Ok(lhs.intersection(&rhs)),
                BinaryOp::Difference => Ok(lhs.minus(&rhs)),
                BinaryOp::DagRange => Ok(lhs.dag_range_to(&rhs)),
                BinaryOp::Range => Ok(lhs.range(&rhs)),
            }
        }
        ExpressionKind::UnionAll(nodes) => {
            let expressions: Vec<_> = nodes
                .iter()
                .map(|node| lower_expression(node, context))
                .try_collect()?;
            Ok(RevsetExpression::union_all(&expressions))
        }
        ExpressionKind::FunctionCall(function) => lower_function_call(function, context),
        ExpressionKind::Modifier(modifier) => {
            let name = modifier.name;
            Err(RevsetParseError::expression(
                format!(r#"Modifier "{name}:" is not allowed in sub expression"#),
                modifier.name_span,
            ))
        }
        ExpressionKind::AliasExpanded(id, subst) => {
            lower_expression(subst, context).map_err(|e| e.within_alias_expansion(*id, node.span))
        }
    }
}

pub fn parse(
    revset_str: &str,
    context: &RevsetParseContext,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let node = revset_parser::parse_program(revset_str)?;
    let node = dsl_util::expand_aliases(node, context.aliases_map)?;
    lower_expression(&node, context)
        .map_err(|err| err.extend_function_candidates(context.aliases_map.function_names()))
}

pub fn parse_with_modifier(
    revset_str: &str,
    context: &RevsetParseContext,
) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseError> {
    let node = revset_parser::parse_program(revset_str)?;
    let node = dsl_util::expand_aliases(node, context.aliases_map)?;
    revset_parser::expect_program_with(
        &node,
        |node| lower_expression(node, context),
        |name, span| match name {
            "all" => Ok(RevsetModifier::All),
            _ => Err(RevsetParseError::with_span(
                RevsetParseErrorKind::NoSuchModifier(name.to_owned()),
                span,
            )),
        },
    )
    .map_err(|err| err.extend_function_candidates(context.aliases_map.function_names()))
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

fn resolve_remote_bookmark(repo: &dyn Repo, name: &str, remote: &str) -> Option<Vec<CommitId>> {
    let view = repo.view();
    let target = match (name, remote) {
        #[cfg(feature = "git")]
        ("HEAD", crate::git::REMOTE_NAME_FOR_LOCAL_GIT_REPO) => view.git_head(),
        (name, remote) => &view.get_remote_bookmark(name, remote).target,
    };
    target
        .is_present()
        .then(|| target.added_ids().cloned().collect())
}

fn all_bookmark_symbols(
    repo: &dyn Repo,
    include_synced_remotes: bool,
) -> impl Iterator<Item = String> + '_ {
    let view = repo.view();
    view.bookmarks()
        .flat_map(move |(name, bookmark_target)| {
            // Remote bookmark "x"@"y" may conflict with local "x@y" in unquoted form.
            let local_target = bookmark_target.local_target;
            let local_symbol = local_target.is_present().then(|| name.to_owned());
            let remote_symbols = bookmark_target
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
    let bookmark_names = all_bookmark_symbols(repo, name.contains('@'));
    let candidates = collect_similar(&name, bookmark_names);
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

struct BookmarkResolver;

impl PartialSymbolResolver for BookmarkResolver {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        let target = repo.view().get_local_bookmark(symbol);
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
    &[&TagResolver, &BookmarkResolver, &GitRefResolver];

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
/// bookmarks.
pub trait SymbolResolverExtension {
    /// PartialSymbolResolvers can capture `repo` for caching purposes if
    /// desired, but they do not have to since `repo` is passed into
    /// `resolve_symbol()` as well.
    fn new_resolvers<'a>(&self, repo: &'a dyn Repo) -> Vec<Box<dyn PartialSymbolResolver + 'a>>;
}

/// Resolves bookmarks, remote bookmarks, tags, git refs, and full and
/// abbreviated commit and change ids.
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
        RevsetCommitRef::RemoteSymbol { name, remote } => {
            resolve_remote_bookmark(repo, name, remote)
                .ok_or_else(|| make_no_such_symbol_error(repo, format!("{name}@{remote}")))
        }
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
        RevsetCommitRef::Bookmarks(pattern) => {
            let commit_ids = repo
                .view()
                .local_bookmarks_matching(pattern)
                .flat_map(|(_, target)| target.added_ids())
                .cloned()
                .collect();
            Ok(commit_ids)
        }
        RevsetCommitRef::RemoteBookmarks {
            bookmark_pattern,
            remote_pattern,
            remote_ref_state,
        } => {
            // TODO: should we allow to select @git bookmarks explicitly?
            let commit_ids = repo
                .view()
                .remote_bookmarks_matching(bookmark_pattern, remote_pattern)
                .filter(|(_, remote_ref)| {
                    remote_ref_state.map_or(true, |state| remote_ref.state == state)
                })
                .filter(|&((_, remote_name), _)| {
                    #[cfg(feature = "git")]
                    {
                        remote_name != crate::git::REMOTE_NAME_FOR_LOCAL_GIT_REPO
                    }
                    #[cfg(not(feature = "git"))]
                    {
                        let _ = remote_name;
                        true
                    }
                })
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
        // (and `remote_bookmarks()`) specified in the revset expression. Alternatively,
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
    date_pattern_context: DatePatternContext,
    extensions: &'a RevsetExtensions,
    workspace: Option<RevsetWorkspaceContext<'a>>,
}

impl<'a> RevsetParseContext<'a> {
    pub fn new(
        aliases_map: &'a RevsetAliasesMap,
        user_email: String,
        date_pattern_context: DatePatternContext,
        extensions: &'a RevsetExtensions,
        workspace: Option<RevsetWorkspaceContext<'a>>,
    ) -> Self {
        Self {
            aliases_map,
            user_email,
            date_pattern_context,
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

    pub fn date_pattern_context(&self) -> &DatePatternContext {
        &self.date_pattern_context
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

    fn parse(revset_str: &str) -> Result<Rc<RevsetExpression>, RevsetParseError> {
        parse_with_aliases(revset_str, [] as [(&str, &str); 0])
    }

    fn parse_with_workspace(
        revset_str: &str,
        workspace_id: &WorkspaceId,
    ) -> Result<Rc<RevsetExpression>, RevsetParseError> {
        parse_with_aliases_and_workspace(revset_str, [] as [(&str, &str); 0], workspace_id)
    }

    fn parse_with_aliases(
        revset_str: &str,
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
    ) -> Result<Rc<RevsetExpression>, RevsetParseError> {
        let mut aliases_map = RevsetAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        let extensions = RevsetExtensions::default();
        let context = RevsetParseContext::new(
            &aliases_map,
            "test.user@example.com".to_string(),
            chrono::Utc::now().fixed_offset().into(),
            &extensions,
            None,
        );
        super::parse(revset_str, &context)
    }

    fn parse_with_aliases_and_workspace(
        revset_str: &str,
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
        workspace_id: &WorkspaceId,
    ) -> Result<Rc<RevsetExpression>, RevsetParseError> {
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
            chrono::Utc::now().fixed_offset().into(),
            &extensions,
            Some(workspace_ctx),
        );
        super::parse(revset_str, &context)
    }

    fn parse_with_modifier(
        revset_str: &str,
    ) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseError> {
        parse_with_aliases_and_modifier(revset_str, [] as [(&str, &str); 0])
    }

    fn parse_with_aliases_and_modifier(
        revset_str: &str,
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
    ) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseError> {
        let mut aliases_map = RevsetAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        let extensions = RevsetExtensions::default();
        let context = RevsetParseContext::new(
            &aliases_map,
            "test.user@example.com".to_string(),
            chrono::Utc::now().fixed_offset().into(),
            &extensions,
            None,
        );
        super::parse_with_modifier(revset_str, &context)
    }

    fn insta_settings() -> insta::Settings {
        let mut settings = insta::Settings::clone_current();
        // Collapse short "Thing(_,)" repeatedly to save vertical space and make
        // the output more readable.
        for _ in 0..4 {
            settings.add_filter(
                r"(?x)
                \b([A-Z]\w*)\(\n
                    \s*(.{1,60}),\n
                \s*\)",
                "$1($2)",
            );
        }
        settings
    }

    #[test]
    #[allow(clippy::redundant_clone)] // allow symbol.clone()
    fn test_revset_expression_building() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let current_wc = RevsetExpression::working_copy(WorkspaceId::default());
        let foo_symbol = RevsetExpression::symbol("foo".to_string());
        let bar_symbol = RevsetExpression::symbol("bar".to_string());
        let baz_symbol = RevsetExpression::symbol("baz".to_string());

        insta::assert_debug_snapshot!(
            current_wc,
            @r###"CommitRef(WorkingCopy(WorkspaceId("default")))"###);
        insta::assert_debug_snapshot!(
            current_wc.heads(),
            @r###"Heads(CommitRef(WorkingCopy(WorkspaceId("default"))))"###);
        insta::assert_debug_snapshot!(
            current_wc.roots(),
            @r###"Roots(CommitRef(WorkingCopy(WorkspaceId("default"))))"###);
        insta::assert_debug_snapshot!(
            current_wc.parents(), @r###"
        Ancestors {
            heads: CommitRef(WorkingCopy(WorkspaceId("default"))),
            generation: 1..2,
        }
        "###);
        insta::assert_debug_snapshot!(
            current_wc.ancestors(), @r###"
        Ancestors {
            heads: CommitRef(WorkingCopy(WorkspaceId("default"))),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.children(), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 1..2,
        }
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.descendants(), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.dag_range_to(&current_wc), @r###"
        DagRange {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(WorkingCopy(WorkspaceId("default"))),
        }
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.connected(), @r###"
        DagRange {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(Symbol("foo")),
        }
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.range(&current_wc), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(WorkingCopy(WorkspaceId("default"))),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.negated(),
            @r###"NotIn(CommitRef(Symbol("foo")))"###);
        insta::assert_debug_snapshot!(
            foo_symbol.union(&current_wc), @r###"
        Union(
            CommitRef(Symbol("foo")),
            CommitRef(WorkingCopy(WorkspaceId("default"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            RevsetExpression::union_all(&[]),
            @"None");
        insta::assert_debug_snapshot!(
            RevsetExpression::union_all(&[current_wc.clone()]),
            @r###"CommitRef(WorkingCopy(WorkspaceId("default")))"###);
        insta::assert_debug_snapshot!(
            RevsetExpression::union_all(&[current_wc.clone(), foo_symbol.clone()]),
            @r###"
        Union(
            CommitRef(WorkingCopy(WorkspaceId("default"))),
            CommitRef(Symbol("foo")),
        )
        "###);
        insta::assert_debug_snapshot!(
            RevsetExpression::union_all(&[
                current_wc.clone(),
                foo_symbol.clone(),
                bar_symbol.clone(),
            ]),
            @r###"
        Union(
            CommitRef(WorkingCopy(WorkspaceId("default"))),
            Union(
                CommitRef(Symbol("foo")),
                CommitRef(Symbol("bar")),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            RevsetExpression::union_all(&[
                current_wc.clone(),
                foo_symbol.clone(),
                bar_symbol.clone(),
                baz_symbol.clone(),
            ]),
            @r###"
        Union(
            Union(
                CommitRef(WorkingCopy(WorkspaceId("default"))),
                CommitRef(Symbol("foo")),
            ),
            Union(
                CommitRef(Symbol("bar")),
                CommitRef(Symbol("baz")),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.intersection(&current_wc), @r###"
        Intersection(
            CommitRef(Symbol("foo")),
            CommitRef(WorkingCopy(WorkspaceId("default"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            foo_symbol.minus(&current_wc), @r###"
        Difference(
            CommitRef(Symbol("foo")),
            CommitRef(WorkingCopy(WorkspaceId("default"))),
        )
        "###);
    }

    #[test]
    fn test_parse_revset() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let main_workspace_id = WorkspaceId::new("main".to_string());
        let other_workspace_id = WorkspaceId::new("other".to_string());

        // Parse "@" (the current working copy)
        insta::assert_debug_snapshot!(
            parse("@").unwrap_err().kind(),
            @"WorkingCopyWithoutWorkspace");
        insta::assert_debug_snapshot!(
            parse("main@").unwrap(),
            @r###"CommitRef(WorkingCopy(WorkspaceId("main")))"###);
        insta::assert_debug_snapshot!(
            parse_with_workspace("@", &main_workspace_id).unwrap(),
            @r###"CommitRef(WorkingCopy(WorkspaceId("main")))"###);
        insta::assert_debug_snapshot!(
            parse_with_workspace("main@", &other_workspace_id).unwrap(),
            @r###"CommitRef(WorkingCopy(WorkspaceId("main")))"###);
        // "@" in function argument must be quoted
        insta::assert_debug_snapshot!(
            parse("author(foo@)").unwrap_err().kind(),
            @r###"Expression("Expected expression of string pattern")"###);
        insta::assert_debug_snapshot!(
            parse(r#"author("foo@")"#).unwrap(),
            @r###"Filter(Author(Substring("foo@")))"###);
        // Parse a single symbol
        insta::assert_debug_snapshot!(
            parse("foo").unwrap(),
            @r###"CommitRef(Symbol("foo"))"###);
        // Default arguments for *bookmarks() are all ""
        insta::assert_debug_snapshot!(
            parse("bookmarks()").unwrap(),
            @r###"CommitRef(Bookmarks(Substring("")))"###);
        insta::assert_debug_snapshot!(parse("remote_bookmarks()").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring(""),
                remote_pattern: Substring(""),
                remote_ref_state: None,
            },
        )
        "###);
        insta::assert_debug_snapshot!(parse("tracked_remote_bookmarks()").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring(""),
                remote_pattern: Substring(""),
                remote_ref_state: Some(Tracking),
            },
        )
        "###);
        insta::assert_debug_snapshot!(parse("untracked_remote_bookmarks()").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring(""),
                remote_pattern: Substring(""),
                remote_ref_state: Some(New),
            },
        )
        "###);
        // Parse a quoted symbol
        insta::assert_debug_snapshot!(
            parse("'foo'").unwrap(),
            @r###"CommitRef(Symbol("foo"))"###);
        // Parse the "parents" operator
        insta::assert_debug_snapshot!(parse("foo-").unwrap(), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 1..2,
        }
        "###);
        // Parse the "children" operator
        insta::assert_debug_snapshot!(parse("foo+").unwrap(), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 1..2,
        }
        "###);
        // Parse the "ancestors" operator
        insta::assert_debug_snapshot!(parse("::foo").unwrap(), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 0..18446744073709551615,
        }
        "###);
        // Parse the "descendants" operator
        insta::assert_debug_snapshot!(parse("foo::").unwrap(), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 0..18446744073709551615,
        }
        "###);
        // Parse the "dag range" operator
        insta::assert_debug_snapshot!(parse("foo::bar").unwrap(), @r###"
        DagRange {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(Symbol("bar")),
        }
        "###);
        // Parse the nullary "dag range" operator
        insta::assert_debug_snapshot!(parse("::").unwrap(), @"All");
        // Parse the "range" prefix operator
        insta::assert_debug_snapshot!(parse("..foo").unwrap(), @r###"
        Range {
            roots: CommitRef(Root),
            heads: CommitRef(Symbol("foo")),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(parse("foo..").unwrap(), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(VisibleHeads),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(parse("foo..bar").unwrap(), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(Symbol("bar")),
            generation: 0..18446744073709551615,
        }
        "###);
        // Parse the nullary "range" operator
        insta::assert_debug_snapshot!(parse("..").unwrap(), @r###"
        Range {
            roots: CommitRef(Root),
            heads: CommitRef(VisibleHeads),
            generation: 0..18446744073709551615,
        }
        "###);
        // Parse the "negate" operator
        insta::assert_debug_snapshot!(
            parse("~ foo").unwrap(),
            @r###"NotIn(CommitRef(Symbol("foo")))"###);
        // Parse the "intersection" operator
        insta::assert_debug_snapshot!(parse("foo & bar").unwrap(), @r###"
        Intersection(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
        // Parse the "union" operator
        insta::assert_debug_snapshot!(parse("foo | bar").unwrap(), @r###"
        Union(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
        // Parse the "difference" operator
        insta::assert_debug_snapshot!(parse("foo ~ bar").unwrap(), @r###"
        Difference(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
    }

    #[test]
    fn test_parse_revset_with_modifier() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            parse_with_modifier("all:foo").unwrap(), @r###"
        (
            CommitRef(Symbol("foo")),
            Some(All),
        )
        "###);

        // Top-level string pattern can't be parsed, which is an error anyway
        insta::assert_debug_snapshot!(
            parse_with_modifier(r#"exact:"foo""#).unwrap_err().kind(),
            @r###"NoSuchModifier("exact")"###);
    }

    #[test]
    fn test_parse_string_pattern() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            parse(r#"bookmarks("foo")"#).unwrap(),
            @r###"CommitRef(Bookmarks(Substring("foo")))"###);
        insta::assert_debug_snapshot!(
            parse(r#"bookmarks(exact:"foo")"#).unwrap(),
            @r###"CommitRef(Bookmarks(Exact("foo")))"###);
        insta::assert_debug_snapshot!(
            parse(r#"bookmarks(substring:"foo")"#).unwrap(),
            @r###"CommitRef(Bookmarks(Substring("foo")))"###);
        insta::assert_debug_snapshot!(
            parse(r#"bookmarks(bad:"foo")"#).unwrap_err().kind(),
            @r###"Expression("Invalid string pattern")"###);
        insta::assert_debug_snapshot!(
            parse(r#"bookmarks(exact::"foo")"#).unwrap_err().kind(),
            @r###"Expression("Expected expression of string pattern")"###);
        insta::assert_debug_snapshot!(
            parse(r#"bookmarks(exact:"foo"+)"#).unwrap_err().kind(),
            @r###"Expression("Expected expression of string pattern")"###);

        // String pattern isn't allowed at top level.
        assert_matches!(
            parse(r#"(exact:"foo")"#).unwrap_err().kind(),
            RevsetParseErrorKind::NotInfixOperator { .. }
        );
    }

    #[test]
    fn test_parse_revset_function() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            parse("parents(foo)").unwrap(), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 1..2,
        }
        "###);
        insta::assert_debug_snapshot!(
            parse("parents(\"foo\")").unwrap(), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 1..2,
        }
        "###);
        insta::assert_debug_snapshot!(
            parse("ancestors(parents(foo))").unwrap(), @r###"
        Ancestors {
            heads: Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 1..2,
            },
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(
            parse("parents(foo,foo)").unwrap_err().kind(), @r###"
        InvalidFunctionArguments {
            name: "parents",
            message: "Expected 1 arguments",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse("root()").unwrap(),
            @"CommitRef(Root)");
        assert!(parse("root(a)").is_err());
        insta::assert_debug_snapshot!(
            parse(r#"description("")"#).unwrap(),
            @r###"Filter(Description(Substring("")))"###);
        insta::assert_debug_snapshot!(
            parse("description(foo)").unwrap(),
            @r###"Filter(Description(Substring("foo")))"###);
        insta::assert_debug_snapshot!(
            parse("description(visible_heads())").unwrap_err().kind(),
            @r###"Expression("Expected expression of string pattern")"###);
        insta::assert_debug_snapshot!(
            parse("description(\"(foo)\")").unwrap(),
            @r###"Filter(Description(Substring("(foo)")))"###);
        assert!(parse("mine(foo)").is_err());
        insta::assert_debug_snapshot!(
            parse("mine()").unwrap(),
            @r###"Filter(Author(ExactI("test.user@example.com")))"###);
        insta::assert_debug_snapshot!(
            parse_with_workspace("empty()", &WorkspaceId::default()).unwrap(),
            @"NotIn(Filter(File(All)))");
        assert!(parse_with_workspace("empty(foo)", &WorkspaceId::default()).is_err());
        assert!(parse_with_workspace("file()", &WorkspaceId::default()).is_err());
        insta::assert_debug_snapshot!(
            parse_with_workspace("file(foo)", &WorkspaceId::default()).unwrap(),
            @r###"Filter(File(Pattern(PrefixPath("foo"))))"###);
        insta::assert_debug_snapshot!(
            parse_with_workspace("file(all())", &WorkspaceId::default()).unwrap(),
            @"Filter(File(All))");
        insta::assert_debug_snapshot!(
            parse_with_workspace(r#"file(file:"foo")"#, &WorkspaceId::default()).unwrap(),
            @r###"Filter(File(Pattern(FilePath("foo"))))"###);
        insta::assert_debug_snapshot!(
            parse_with_workspace("file(foo|bar&baz)", &WorkspaceId::default()).unwrap(), @r###"
        Filter(
            File(
                UnionAll(
                    [
                        Pattern(PrefixPath("foo")),
                        Intersection(
                            Pattern(PrefixPath("bar")),
                            Pattern(PrefixPath("baz")),
                        ),
                    ],
                ),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            parse_with_workspace("file(foo, bar, baz)", &WorkspaceId::default()).unwrap(), @r###"
        Filter(
            File(
                UnionAll(
                    [
                        Pattern(PrefixPath("foo")),
                        Pattern(PrefixPath("bar")),
                        Pattern(PrefixPath("baz")),
                    ],
                ),
            ),
        )
        "###);
    }

    #[test]
    fn test_parse_revset_keyword_arguments() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            parse("remote_bookmarks(remote=foo)").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring(""),
                remote_pattern: Substring("foo"),
                remote_ref_state: None,
            },
        )
        "###);
        insta::assert_debug_snapshot!(
            parse("remote_bookmarks(foo, remote=bar)").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring("foo"),
                remote_pattern: Substring("bar"),
                remote_ref_state: None,
            },
        )
        "###);
        insta::assert_debug_snapshot!(
            parse("tracked_remote_bookmarks(foo, remote=bar)").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring("foo"),
                remote_pattern: Substring("bar"),
                remote_ref_state: Some(Tracking),
            },
        )
        "###);
        insta::assert_debug_snapshot!(
            parse("untracked_remote_bookmarks(foo, remote=bar)").unwrap(), @r###"
        CommitRef(
            RemoteBookmarks {
                bookmark_pattern: Substring("foo"),
                remote_pattern: Substring("bar"),
                remote_ref_state: Some(New),
            },
        )
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_bookmarks(remote=foo, bar)"#).unwrap_err().kind(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_bookmarks",
            message: "Positional argument follows keyword argument",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_bookmarks("", foo, remote=bar)"#).unwrap_err().kind(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_bookmarks",
            message: "Got multiple values for keyword \"remote\"",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_bookmarks(remote=bar, remote=bar)"#).unwrap_err().kind(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_bookmarks",
            message: "Got multiple values for keyword \"remote\"",
        }
        "###);
        insta::assert_debug_snapshot!(
            parse(r#"remote_bookmarks(unknown=bar)"#).unwrap_err().kind(),
            @r###"
        InvalidFunctionArguments {
            name: "remote_bookmarks",
            message: "Unexpected keyword argument \"unknown\"",
        }
        "###);
    }

    #[test]
    fn test_expand_symbol_alias() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            parse_with_aliases("AB|c", [("AB", "a|b")]).unwrap(), @r###"
        Union(
            Union(
                CommitRef(Symbol("a")),
                CommitRef(Symbol("b")),
            ),
            CommitRef(Symbol("c")),
        )
        "###);

        // Alias can be substituted to string literal.
        insta::assert_debug_snapshot!(
            parse_with_aliases_and_workspace("file(A)", [("A", "a")], &WorkspaceId::default())
                .unwrap(),
            @r###"Filter(File(Pattern(PrefixPath("a"))))"###);

        // Alias can be substituted to string pattern.
        insta::assert_debug_snapshot!(
            parse_with_aliases("author(A)", [("A", "a")]).unwrap(),
            @r###"Filter(Author(Substring("a")))"###);
        // However, parentheses are required because top-level x:y is parsed as
        // program modifier.
        insta::assert_debug_snapshot!(
            parse_with_aliases("author(A)", [("A", "(exact:a)")]).unwrap(),
            @r###"Filter(Author(Exact("a")))"###);

        // Sub-expression alias cannot be substituted to modifier expression.
        insta::assert_debug_snapshot!(
            parse_with_aliases_and_modifier("A-", [("A", "all:a")]).unwrap_err().kind(),
            @r###"BadAliasExpansion("A")"###);
    }

    #[test]
    fn test_expand_function_alias() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        // Pass string literal as parameter.
        insta::assert_debug_snapshot!(
            parse_with_aliases("F(a)", [("F(x)", "author(x)|committer(x)")]).unwrap(), @r###"
        Union(
            Filter(Author(Substring("a"))),
            Filter(Committer(Substring("a"))),
        )
        "###);
    }

    #[test]
    fn test_optimize_subtree() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        // Check that transform_expression_bottom_up() never rewrites enum variant
        // (e.g. Range -> DagRange) nor reorders arguments unintentionally.

        insta::assert_debug_snapshot!(
            optimize(parse("parents(bookmarks() & all())").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Bookmarks(Substring(""))),
            generation: 1..2,
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("children(bookmarks() & all())").unwrap()), @r###"
        Descendants {
            roots: CommitRef(Bookmarks(Substring(""))),
            generation: 1..2,
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("ancestors(bookmarks() & all())").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Bookmarks(Substring(""))),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("descendants(bookmarks() & all())").unwrap()), @r###"
        Descendants {
            roots: CommitRef(Bookmarks(Substring(""))),
            generation: 0..18446744073709551615,
        }
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("(bookmarks() & all())..(all() & tags())").unwrap()), @r###"
        Range {
            roots: CommitRef(Bookmarks(Substring(""))),
            heads: CommitRef(Tags),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("(bookmarks() & all())::(all() & tags())").unwrap()), @r###"
        DagRange {
            roots: CommitRef(Bookmarks(Substring(""))),
            heads: CommitRef(Tags),
        }
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("heads(bookmarks() & all())").unwrap()),
            @r###"Heads(CommitRef(Bookmarks(Substring(""))))"###);
        insta::assert_debug_snapshot!(
            optimize(parse("roots(bookmarks() & all())").unwrap()),
            @r###"Roots(CommitRef(Bookmarks(Substring(""))))"###);

        insta::assert_debug_snapshot!(
            optimize(parse("latest(bookmarks() & all(), 2)").unwrap()), @r###"
        Latest {
            candidates: CommitRef(Bookmarks(Substring(""))),
            count: 2,
        }
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("present(foo ~ bar)").unwrap()), @r###"
        Present(
            Difference(
                CommitRef(Symbol("foo")),
                CommitRef(Symbol("bar")),
            ),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("present(bookmarks() & all())").unwrap()),
            @r###"Present(CommitRef(Bookmarks(Substring(""))))"###);

        insta::assert_debug_snapshot!(
            optimize(parse("~bookmarks() & all()").unwrap()),
            @r###"NotIn(CommitRef(Bookmarks(Substring(""))))"###);
        insta::assert_debug_snapshot!(
            optimize(parse("(bookmarks() & all()) | (all() & tags())").unwrap()), @r###"
        Union(
            CommitRef(Bookmarks(Substring(""))),
            CommitRef(Tags),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("(bookmarks() & all()) & (all() & tags())").unwrap()), @r###"
        Intersection(
            CommitRef(Bookmarks(Substring(""))),
            CommitRef(Tags),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("(bookmarks() & all()) ~ (all() & tags())").unwrap()), @r###"
        Difference(
            CommitRef(Bookmarks(Substring(""))),
            CommitRef(Tags),
        )
        "###);
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

        let parsed = parse("bookmarks() | tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(&parsed, &optimized));

        let parsed = parse("bookmarks() & tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(&parsed, &optimized));

        // Only left subtree should be rewritten.
        let parsed = parse("(bookmarks() & all()) | tags()").unwrap();
        let optimized = optimize(parsed.clone());
        assert_matches!(
            unwrap_union(&optimized).0.as_ref(),
            RevsetExpression::CommitRef(RevsetCommitRef::Bookmarks(_))
        );
        assert!(Rc::ptr_eq(
            unwrap_union(&parsed).1,
            unwrap_union(&optimized).1
        ));

        // Only right subtree should be rewritten.
        let parsed = parse("bookmarks() | (all() & tags())").unwrap();
        let optimized = optimize(parsed.clone());
        assert!(Rc::ptr_eq(
            unwrap_union(&parsed).0,
            unwrap_union(&optimized).0
        ));
        assert_matches!(
            unwrap_union(&optimized).1.as_ref(),
            RevsetExpression::CommitRef(RevsetCommitRef::Tags)
        );
    }

    #[test]
    fn test_optimize_difference() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(optimize(parse("foo & ~bar").unwrap()), @r###"
        Difference(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~foo & bar").unwrap()), @r###"
        Difference(
            CommitRef(Symbol("bar")),
            CommitRef(Symbol("foo")),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~foo & bar & ~baz").unwrap()), @r###"
        Difference(
            Difference(
                CommitRef(Symbol("bar")),
                CommitRef(Symbol("foo")),
            ),
            CommitRef(Symbol("baz")),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(all() & ~foo) & bar").unwrap()), @r###"
        Difference(
            CommitRef(Symbol("bar")),
            CommitRef(Symbol("foo")),
        )
        "###);

        // Binary difference operation should go through the same optimization passes.
        insta::assert_debug_snapshot!(
            optimize(parse("all() ~ foo").unwrap()),
            @r###"NotIn(CommitRef(Symbol("foo")))"###);
        insta::assert_debug_snapshot!(optimize(parse("foo ~ bar").unwrap()), @r###"
        Difference(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(all() ~ foo) & bar").unwrap()), @r###"
        Difference(
            CommitRef(Symbol("bar")),
            CommitRef(Symbol("foo")),
        )
        "###);

        // Range expression.
        insta::assert_debug_snapshot!(optimize(parse("::foo & ~::bar").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("bar")),
            heads: CommitRef(Symbol("foo")),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~::foo & ::bar").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(Symbol("bar")),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo..").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(VisibleHeads),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo..bar").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(Symbol("bar")),
            generation: 0..18446744073709551615,
        }
        "###);

        // Double/triple negates.
        insta::assert_debug_snapshot!(optimize(parse("foo & ~~bar").unwrap()), @r###"
        Intersection(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo & ~~~bar").unwrap()), @r###"
        Difference(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~(all() & ~foo) & bar").unwrap()), @r###"
        Intersection(
            CommitRef(Symbol("foo")),
            CommitRef(Symbol("bar")),
        )
        "###);

        // Should be better than '(all() & ~foo) & (all() & ~bar)'.
        insta::assert_debug_snapshot!(optimize(parse("~foo & ~bar").unwrap()), @r###"
        Difference(
            NotIn(CommitRef(Symbol("foo"))),
            CommitRef(Symbol("bar")),
        )
        "###);
    }

    #[test]
    fn test_optimize_not_in_ancestors() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        // '~(::foo)' is equivalent to 'foo..'.
        insta::assert_debug_snapshot!(optimize(parse("~(::foo)").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(VisibleHeads),
            generation: 0..18446744073709551615,
        }
        "###);

        // '~(::foo-)' is equivalent to 'foo-..'.
        insta::assert_debug_snapshot!(optimize(parse("~(::foo-)").unwrap()), @r###"
        Range {
            roots: Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 1..2,
            },
            heads: CommitRef(VisibleHeads),
            generation: 0..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~(::foo--)").unwrap()), @r###"
        Range {
            roots: Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 2..3,
            },
            heads: CommitRef(VisibleHeads),
            generation: 0..18446744073709551615,
        }
        "###);

        // Bounded ancestors shouldn't be substituted.
        insta::assert_debug_snapshot!(optimize(parse("~ancestors(foo, 1)").unwrap()), @r###"
        NotIn(
            Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 0..1,
            },
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("~ancestors(foo-, 1)").unwrap()), @r###"
        NotIn(
            Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 1..2,
            },
        )
        "###);
    }

    #[test]
    fn test_optimize_filter_difference() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        // '~empty()' -> '~~file(*)' -> 'file(*)'
        insta::assert_debug_snapshot!(optimize(parse("~empty()").unwrap()), @"Filter(File(All))");

        // '& baz' can be moved into the filter node, and form a difference node.
        insta::assert_debug_snapshot!(
            optimize(parse("(author(foo) & ~bar) & baz").unwrap()), @r###"
        Intersection(
            Difference(
                CommitRef(Symbol("baz")),
                CommitRef(Symbol("bar")),
            ),
            Filter(Author(Substring("foo"))),
        )
        "###);

        // '~set & filter()' shouldn't be substituted.
        insta::assert_debug_snapshot!(
            optimize(parse("~foo & author(bar)").unwrap()), @r###"
        Intersection(
            NotIn(CommitRef(Symbol("foo"))),
            Filter(Author(Substring("bar"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("~foo & (author(bar) | baz)").unwrap()), @r###"
        Intersection(
            NotIn(CommitRef(Symbol("foo"))),
            AsFilter(
                Union(
                    Filter(Author(Substring("bar"))),
                    CommitRef(Symbol("baz")),
                ),
            ),
        )
        "###);

        // Filter should be moved right of the intersection.
        insta::assert_debug_snapshot!(
            optimize(parse("author(foo) ~ bar").unwrap()), @r###"
        Intersection(
            NotIn(CommitRef(Symbol("bar"))),
            Filter(Author(Substring("foo"))),
        )
        "###);
    }

    #[test]
    fn test_optimize_filter_intersection() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            optimize(parse("author(foo)").unwrap()), @r###"Filter(Author(Substring("foo")))"###);

        insta::assert_debug_snapshot!(optimize(parse("foo & description(bar)").unwrap()), @r###"
        Intersection(
            CommitRef(Symbol("foo")),
            Filter(Description(Substring("bar"))),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse("author(foo) & bar").unwrap()), @r###"
        Intersection(
            CommitRef(Symbol("bar")),
            Filter(Author(Substring("foo"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("author(foo) & committer(bar)").unwrap()), @r###"
        Intersection(
            Filter(Author(Substring("foo"))),
            Filter(Committer(Substring("bar"))),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & author(baz)").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("foo")),
                Filter(Description(Substring("bar"))),
            ),
            Filter(Author(Substring("baz"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("committer(foo) & bar & author(baz)").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("bar")),
                Filter(Committer(Substring("foo"))),
            ),
            Filter(Author(Substring("baz"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse_with_workspace("committer(foo) & file(bar) & baz", &WorkspaceId::default()).unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("baz")),
                Filter(Committer(Substring("foo"))),
            ),
            Filter(File(Pattern(PrefixPath("bar")))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse_with_workspace("committer(foo) & file(bar) & author(baz)", &WorkspaceId::default()).unwrap()), @r###"
        Intersection(
            Intersection(
                Filter(Committer(Substring("foo"))),
                Filter(File(Pattern(PrefixPath("bar")))),
            ),
            Filter(Author(Substring("baz"))),
        )
        "###);
        insta::assert_debug_snapshot!(optimize(parse_with_workspace("foo & file(bar) & baz", &WorkspaceId::default()).unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("foo")),
                CommitRef(Symbol("baz")),
            ),
            Filter(File(Pattern(PrefixPath("bar")))),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & author(baz) & qux").unwrap()), @r###"
        Intersection(
            Intersection(
                Intersection(
                    CommitRef(Symbol("foo")),
                    CommitRef(Symbol("qux")),
                ),
                Filter(Description(Substring("bar"))),
            ),
            Filter(Author(Substring("baz"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & parents(author(baz)) & qux").unwrap()), @r###"
        Intersection(
            Intersection(
                Intersection(
                    CommitRef(Symbol("foo")),
                    Ancestors {
                        heads: Filter(Author(Substring("baz"))),
                        generation: 1..2,
                    },
                ),
                CommitRef(Symbol("qux")),
            ),
            Filter(Description(Substring("bar"))),
        )
        "###);
        insta::assert_debug_snapshot!(
            optimize(parse("foo & description(bar) & parents(author(baz) & qux)").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("foo")),
                Ancestors {
                    heads: Intersection(
                        CommitRef(Symbol("qux")),
                        Filter(Author(Substring("baz"))),
                    ),
                    generation: 1..2,
                },
            ),
            Filter(Description(Substring("bar"))),
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
                            CommitRef(Symbol("a")),
                            CommitRef(Symbol("b")),
                        ),
                        CommitRef(Symbol("c")),
                    ),
                    Filter(Author(Substring("A"))),
                ),
                Filter(Author(Substring("B"))),
            ),
            Filter(Author(Substring("C"))),
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
                            CommitRef(Symbol("a")),
                            Intersection(
                                CommitRef(Symbol("b")),
                                CommitRef(Symbol("c")),
                            ),
                        ),
                        CommitRef(Symbol("d")),
                    ),
                    Filter(Author(Substring("A"))),
                ),
                Filter(Author(Substring("B"))),
            ),
            Filter(Author(Substring("C"))),
        )
        "###);

        // 'all()' moves in to 'filter()' first, so 'A & filter()' can be found.
        insta::assert_debug_snapshot!(
            optimize(parse("foo & (all() & description(bar)) & (author(baz) & all())").unwrap()),
            @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("foo")),
                Filter(Description(Substring("bar"))),
            ),
            Filter(Author(Substring("baz"))),
        )
        "###);
    }

    #[test]
    fn test_optimize_filter_subtree() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(
            optimize(parse("(author(foo) | bar) & baz").unwrap()), @r###"
        Intersection(
            CommitRef(Symbol("baz")),
            AsFilter(
                Union(
                    Filter(Author(Substring("foo"))),
                    CommitRef(Symbol("bar")),
                ),
            ),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("(foo | committer(bar)) & description(baz) & qux").unwrap()), @r###"
        Intersection(
            Intersection(
                CommitRef(Symbol("qux")),
                AsFilter(
                    Union(
                        CommitRef(Symbol("foo")),
                        Filter(Committer(Substring("bar"))),
                    ),
                ),
            ),
            Filter(Description(Substring("baz"))),
        )
        "###);

        insta::assert_debug_snapshot!(
            optimize(parse("(~present(author(foo) & bar) | baz) & qux").unwrap()), @r###"
        Intersection(
            CommitRef(Symbol("qux")),
            AsFilter(
                Union(
                    AsFilter(
                        NotIn(
                            AsFilter(
                                Present(
                                    Intersection(
                                        CommitRef(Symbol("bar")),
                                        Filter(Author(Substring("foo"))),
                                    ),
                                ),
                            ),
                        ),
                    ),
                    CommitRef(Symbol("baz")),
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
                            CommitRef(Symbol("a")),
                            CommitRef(Symbol("b")),
                        ),
                        CommitRef(Symbol("c")),
                    ),
                    AsFilter(
                        Union(
                            Filter(Author(Substring("A"))),
                            CommitRef(Symbol("0")),
                        ),
                    ),
                ),
                AsFilter(
                    Union(
                        Filter(Author(Substring("B"))),
                        CommitRef(Symbol("1")),
                    ),
                ),
            ),
            AsFilter(
                Union(
                    Filter(Author(Substring("C"))),
                    CommitRef(Symbol("2")),
                ),
            ),
        )
        "###);
    }

    #[test]
    fn test_optimize_ancestors() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        // Typical scenario: fold nested parents()
        insta::assert_debug_snapshot!(optimize(parse("foo--").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 2..3,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("::(foo---)").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 3..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(::foo)---").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 3..18446744073709551615,
        }
        "###);

        // 'foo-+' is not 'foo'.
        insta::assert_debug_snapshot!(optimize(parse("foo---+").unwrap()), @r###"
        Descendants {
            roots: Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 3..4,
            },
            generation: 1..2,
        }
        "###);

        // For 'roots..heads', heads can be folded.
        insta::assert_debug_snapshot!(optimize(parse("foo..(bar--)").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: CommitRef(Symbol("bar")),
            generation: 2..18446744073709551615,
        }
        "###);
        // roots can also be folded, and the range expression is reconstructed.
        insta::assert_debug_snapshot!(optimize(parse("(foo--)..(bar---)").unwrap()), @r###"
        Range {
            roots: Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 2..3,
            },
            heads: CommitRef(Symbol("bar")),
            generation: 3..18446744073709551615,
        }
        "###);
        // Bounded ancestors shouldn't be substituted to range.
        insta::assert_debug_snapshot!(
            optimize(parse("~ancestors(foo, 2) & ::bar").unwrap()), @r###"
        Difference(
            Ancestors {
                heads: CommitRef(Symbol("bar")),
                generation: 0..18446744073709551615,
            },
            Ancestors {
                heads: CommitRef(Symbol("foo")),
                generation: 0..2,
            },
        )
        "###);

        // If inner range is bounded by roots, it cannot be merged.
        // e.g. '..(foo..foo)' is equivalent to '..none()', not to '..foo'
        insta::assert_debug_snapshot!(optimize(parse("(foo..bar)--").unwrap()), @r###"
        Ancestors {
            heads: Range {
                roots: CommitRef(Symbol("foo")),
                heads: CommitRef(Symbol("bar")),
                generation: 0..18446744073709551615,
            },
            generation: 2..3,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("foo..(bar..baz)").unwrap()), @r###"
        Range {
            roots: CommitRef(Symbol("foo")),
            heads: Range {
                roots: CommitRef(Symbol("bar")),
                heads: CommitRef(Symbol("baz")),
                generation: 0..18446744073709551615,
            },
            generation: 0..18446744073709551615,
        }
        "###);

        // Ancestors of empty generation range should be empty.
        insta::assert_debug_snapshot!(
            optimize(parse("ancestors(ancestors(foo), 0)").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 0..0,
        }
        "###
        );
        insta::assert_debug_snapshot!(
            optimize(parse("ancestors(ancestors(foo, 0))").unwrap()), @r###"
        Ancestors {
            heads: CommitRef(Symbol("foo")),
            generation: 0..0,
        }
        "###
        );
    }

    #[test]
    fn test_optimize_descendants() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        // Typical scenario: fold nested children()
        insta::assert_debug_snapshot!(optimize(parse("foo++").unwrap()), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 2..3,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(foo+++)::").unwrap()), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 3..18446744073709551615,
        }
        "###);
        insta::assert_debug_snapshot!(optimize(parse("(foo::)+++").unwrap()), @r###"
        Descendants {
            roots: CommitRef(Symbol("foo")),
            generation: 3..18446744073709551615,
        }
        "###);

        // 'foo+-' is not 'foo'.
        insta::assert_debug_snapshot!(optimize(parse("foo+++-").unwrap()), @r###"
        Ancestors {
            heads: Descendants {
                roots: CommitRef(Symbol("foo")),
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
                roots: CommitRef(Symbol("foo")),
                generation: 2..3,
            },
            heads: CommitRef(Symbol("bar")),
        }
        "###);
    }
}
