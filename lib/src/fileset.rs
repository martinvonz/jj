// Copyright 2024 The Jujutsu Authors
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

//! Functional language for selecting a set of paths.

use std::collections::HashMap;
use std::{iter, path, slice};

use once_cell::sync::Lazy;
use thiserror::Error;

use crate::dsl_util::collect_similar;
use crate::fileset_parser::{
    self, BinaryOp, ExpressionKind, ExpressionNode, FunctionCallNode, UnaryOp,
};
pub use crate::fileset_parser::{FilesetParseError, FilesetParseErrorKind, FilesetParseResult};
use crate::matchers::{
    DifferenceMatcher, EverythingMatcher, FileGlobsMatcher, FilesMatcher, IntersectionMatcher,
    Matcher, NothingMatcher, PrefixMatcher, UnionMatcher,
};
use crate::repo_path::{
    RelativePathParseError, RepoPath, RepoPathBuf, RepoPathUiConverter, UiPathParseError,
};

/// Error occurred during file pattern parsing.
#[derive(Debug, Error)]
pub enum FilePatternParseError {
    /// Unknown pattern kind is specified.
    #[error(r#"Invalid file pattern kind "{0}:""#)]
    InvalidKind(String),
    /// Failed to parse input UI path.
    #[error(transparent)]
    UiPath(#[from] UiPathParseError),
    /// Failed to parse input workspace-relative path.
    #[error(transparent)]
    RelativePath(#[from] RelativePathParseError),
    /// Failed to parse glob pattern.
    #[error(transparent)]
    GlobPattern(#[from] glob::PatternError),
}

/// Basic pattern to match `RepoPath`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilePattern {
    /// Matches file (or exact) path.
    FilePath(RepoPathBuf),
    /// Matches path prefix.
    PrefixPath(RepoPathBuf),
    /// Matches file (or exact) path with glob pattern.
    FileGlob {
        /// Prefix directory path where the `pattern` will be evaluated.
        dir: RepoPathBuf,
        /// Glob pattern relative to `dir`.
        pattern: glob::Pattern,
    },
    // TODO: add more patterns:
    // - FilesInPath: files in directory, non-recursively?
    // - NameGlob or SuffixGlob: file name with glob?
}

impl FilePattern {
    /// Parses the given `input` string as pattern of the specified `kind`.
    pub fn from_str_kind(
        path_converter: &RepoPathUiConverter,
        input: &str,
        kind: &str,
    ) -> Result<Self, FilePatternParseError> {
        // Naming convention:
        // * path normalization
        //   * cwd: cwd-relative path (default)
        //   * root: workspace-relative path
        // * where to anchor
        //   * file: exact file path
        //   * prefix: path prefix (files under directory recursively)
        //   * files-in: files in directory non-recursively
        //   * name: file name component (or suffix match?)
        //   * substring: substring match?
        // * string pattern syntax (+ case sensitivity?)
        //   * path: literal path (default) (default anchor: prefix)
        //   * glob: glob pattern (default anchor: file)
        //   * regex?
        match kind {
            "cwd" => Self::cwd_prefix_path(path_converter, input),
            "cwd-file" | "file" => Self::cwd_file_path(path_converter, input),
            "cwd-glob" | "glob" => Self::cwd_file_glob(path_converter, input),
            "root" => Self::root_prefix_path(input),
            "root-file" => Self::root_file_path(input),
            "root-glob" => Self::root_file_glob(input),
            _ => Err(FilePatternParseError::InvalidKind(kind.to_owned())),
        }
    }

    /// Pattern that matches cwd-relative file (or exact) path.
    pub fn cwd_file_path(
        path_converter: &RepoPathUiConverter,
        input: impl AsRef<str>,
    ) -> Result<Self, FilePatternParseError> {
        let path = path_converter.parse_file_path(input.as_ref())?;
        Ok(FilePattern::FilePath(path))
    }

    /// Pattern that matches cwd-relative path prefix.
    pub fn cwd_prefix_path(
        path_converter: &RepoPathUiConverter,
        input: impl AsRef<str>,
    ) -> Result<Self, FilePatternParseError> {
        let path = path_converter.parse_file_path(input.as_ref())?;
        Ok(FilePattern::PrefixPath(path))
    }

    /// Pattern that matches cwd-relative file path glob.
    pub fn cwd_file_glob(
        path_converter: &RepoPathUiConverter,
        input: impl AsRef<str>,
    ) -> Result<Self, FilePatternParseError> {
        let (dir, pattern) = split_glob_path(input.as_ref());
        let dir = path_converter.parse_file_path(dir)?;
        Self::file_glob_at(dir, pattern)
    }

    /// Pattern that matches workspace-relative file (or exact) path.
    pub fn root_file_path(input: impl AsRef<str>) -> Result<Self, FilePatternParseError> {
        // TODO: Let caller pass in converter for root-relative paths too
        let path = RepoPathBuf::from_relative_path(input.as_ref())?;
        Ok(FilePattern::FilePath(path))
    }

    /// Pattern that matches workspace-relative path prefix.
    pub fn root_prefix_path(input: impl AsRef<str>) -> Result<Self, FilePatternParseError> {
        let path = RepoPathBuf::from_relative_path(input.as_ref())?;
        Ok(FilePattern::PrefixPath(path))
    }

    /// Pattern that matches workspace-relative file path glob.
    pub fn root_file_glob(input: impl AsRef<str>) -> Result<Self, FilePatternParseError> {
        let (dir, pattern) = split_glob_path(input.as_ref());
        let dir = RepoPathBuf::from_relative_path(dir)?;
        Self::file_glob_at(dir, pattern)
    }

    fn file_glob_at(dir: RepoPathBuf, input: &str) -> Result<Self, FilePatternParseError> {
        if input.is_empty() {
            return Ok(FilePattern::FilePath(dir));
        }
        // Normalize separator to '/', reject ".." which will never match
        let normalized = RepoPathBuf::from_relative_path(input)?;
        let pattern = glob::Pattern::new(normalized.as_internal_file_string())?;
        Ok(FilePattern::FileGlob { dir, pattern })
    }

    /// Returns path if this pattern represents a literal path in a workspace.
    /// Returns `None` if this is a glob pattern for example.
    pub fn as_path(&self) -> Option<&RepoPath> {
        match self {
            FilePattern::FilePath(path) => Some(path),
            FilePattern::PrefixPath(path) => Some(path),
            FilePattern::FileGlob { .. } => None,
        }
    }
}

/// Splits `input` path into literal directory path and glob pattern.
fn split_glob_path(input: &str) -> (&str, &str) {
    const GLOB_CHARS: &[char] = &['?', '*', '[', ']']; // see glob::Pattern::escape()
    let prefix_len = input
        .split_inclusive(path::is_separator)
        .take_while(|component| !component.contains(GLOB_CHARS))
        .map(|component| component.len())
        .sum();
    input.split_at(prefix_len)
}

/// AST-level representation of the fileset expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilesetExpression {
    /// Matches nothing.
    None,
    /// Matches everything.
    All,
    /// Matches basic pattern.
    Pattern(FilePattern),
    /// Matches any of the expressions.
    ///
    /// Use `FilesetExpression::union_all()` to construct a union expression.
    /// It will normalize 0-ary or 1-ary union.
    UnionAll(Vec<FilesetExpression>),
    /// Matches both expressions.
    Intersection(Box<FilesetExpression>, Box<FilesetExpression>),
    /// Matches the first expression, but not the second expression.
    Difference(Box<FilesetExpression>, Box<FilesetExpression>),
}

impl FilesetExpression {
    /// Expression that matches nothing.
    pub fn none() -> Self {
        FilesetExpression::None
    }

    /// Expression that matches everything.
    pub fn all() -> Self {
        FilesetExpression::All
    }

    /// Expression that matches the given `pattern`.
    pub fn pattern(pattern: FilePattern) -> Self {
        FilesetExpression::Pattern(pattern)
    }

    /// Expression that matches file (or exact) path.
    pub fn file_path(path: RepoPathBuf) -> Self {
        FilesetExpression::Pattern(FilePattern::FilePath(path))
    }

    /// Expression that matches path prefix.
    pub fn prefix_path(path: RepoPathBuf) -> Self {
        FilesetExpression::Pattern(FilePattern::PrefixPath(path))
    }

    /// Expression that matches either `self` or `other` (or both).
    pub fn union(self, other: Self) -> Self {
        match self {
            // Micro optimization for "x | y | z"
            FilesetExpression::UnionAll(mut expressions) => {
                expressions.push(other);
                FilesetExpression::UnionAll(expressions)
            }
            expr => FilesetExpression::UnionAll(vec![expr, other]),
        }
    }

    /// Expression that matches any of the given `expressions`.
    pub fn union_all(expressions: Vec<FilesetExpression>) -> Self {
        match expressions.len() {
            0 => FilesetExpression::none(),
            1 => expressions.into_iter().next().unwrap(),
            _ => FilesetExpression::UnionAll(expressions),
        }
    }

    /// Expression that matches both `self` and `other`.
    pub fn intersection(self, other: Self) -> Self {
        FilesetExpression::Intersection(Box::new(self), Box::new(other))
    }

    /// Expression that matches `self` but not `other`.
    pub fn difference(self, other: Self) -> Self {
        FilesetExpression::Difference(Box::new(self), Box::new(other))
    }

    /// Flattens union expression at most one level.
    fn as_union_all(&self) -> &[Self] {
        match self {
            FilesetExpression::None => &[],
            FilesetExpression::UnionAll(exprs) => exprs,
            _ => slice::from_ref(self),
        }
    }

    fn dfs_pre(&self) -> impl Iterator<Item = &Self> {
        let mut stack: Vec<&Self> = vec![self];
        iter::from_fn(move || {
            let expr = stack.pop()?;
            match expr {
                FilesetExpression::None
                | FilesetExpression::All
                | FilesetExpression::Pattern(_) => {}
                FilesetExpression::UnionAll(exprs) => stack.extend(exprs.iter().rev()),
                FilesetExpression::Intersection(expr1, expr2)
                | FilesetExpression::Difference(expr1, expr2) => {
                    stack.push(expr2);
                    stack.push(expr1);
                }
            }
            Some(expr)
        })
    }

    /// Iterates literal paths recursively from this expression.
    ///
    /// For example, `"a", "b", "c"` will be yielded in that order for
    /// expression `"a" | all() & "b" | ~"c"`.
    pub fn explicit_paths(&self) -> impl Iterator<Item = &RepoPath> {
        // pre/post-ordering doesn't matter so long as children are visited from
        // left to right.
        self.dfs_pre().flat_map(|expr| match expr {
            FilesetExpression::Pattern(pattern) => pattern.as_path(),
            _ => None,
        })
    }

    /// Transforms the expression tree to `Matcher` object.
    pub fn to_matcher(&self) -> Box<dyn Matcher> {
        build_union_matcher(self.as_union_all())
    }
}

/// Transforms the union `expressions` to `Matcher` object.
///
/// Since `Matcher` typically accepts a set of patterns to be OR-ed, this
/// function takes a list of union `expressions` as input.
fn build_union_matcher(expressions: &[FilesetExpression]) -> Box<dyn Matcher> {
    let mut file_paths = Vec::new();
    let mut prefix_paths = Vec::new();
    let mut file_globs = Vec::new();
    let mut matchers: Vec<Option<Box<dyn Matcher>>> = Vec::new();
    for expr in expressions {
        let matcher: Box<dyn Matcher> = match expr {
            // None and All are supposed to be simplified by caller.
            FilesetExpression::None => Box::new(NothingMatcher),
            FilesetExpression::All => Box::new(EverythingMatcher),
            FilesetExpression::Pattern(pattern) => {
                match pattern {
                    FilePattern::FilePath(path) => file_paths.push(path),
                    FilePattern::PrefixPath(path) => prefix_paths.push(path),
                    FilePattern::FileGlob { dir, pattern } => {
                        file_globs.push((dir, pattern.clone()))
                    }
                }
                continue;
            }
            // UnionAll is supposed to be flattened by caller.
            FilesetExpression::UnionAll(exprs) => build_union_matcher(exprs),
            FilesetExpression::Intersection(expr1, expr2) => {
                let m1 = build_union_matcher(expr1.as_union_all());
                let m2 = build_union_matcher(expr2.as_union_all());
                Box::new(IntersectionMatcher::new(m1, m2))
            }
            FilesetExpression::Difference(expr1, expr2) => {
                let m1 = build_union_matcher(expr1.as_union_all());
                let m2 = build_union_matcher(expr2.as_union_all());
                Box::new(DifferenceMatcher::new(m1, m2))
            }
        };
        matchers.push(Some(matcher));
    }

    if !file_paths.is_empty() {
        matchers.push(Some(Box::new(FilesMatcher::new(file_paths))));
    }
    if !prefix_paths.is_empty() {
        matchers.push(Some(Box::new(PrefixMatcher::new(prefix_paths))));
    }
    if !file_globs.is_empty() {
        matchers.push(Some(Box::new(FileGlobsMatcher::new(file_globs))));
    }
    union_all_matchers(&mut matchers)
}

/// Concatenates all `matchers` as union.
///
/// Each matcher element must be wrapped in `Some` so the matchers can be moved
/// in arbitrary order.
fn union_all_matchers(matchers: &mut [Option<Box<dyn Matcher>>]) -> Box<dyn Matcher> {
    match matchers {
        [] => Box::new(NothingMatcher),
        [matcher] => matcher.take().expect("matcher should still be available"),
        _ => {
            // Build balanced tree to minimize the recursion depth.
            let (left, right) = matchers.split_at_mut(matchers.len() / 2);
            let m1 = union_all_matchers(left);
            let m2 = union_all_matchers(right);
            Box::new(UnionMatcher::new(m1, m2))
        }
    }
}

type FilesetFunction =
    fn(&RepoPathUiConverter, &FunctionCallNode) -> FilesetParseResult<FilesetExpression>;

static BUILTIN_FUNCTION_MAP: Lazy<HashMap<&'static str, FilesetFunction>> = Lazy::new(|| {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map: HashMap<&'static str, FilesetFunction> = HashMap::new();
    map.insert("none", |_path_converter, function| {
        function.expect_no_arguments()?;
        Ok(FilesetExpression::none())
    });
    map.insert("all", |_path_converter, function| {
        function.expect_no_arguments()?;
        Ok(FilesetExpression::all())
    });
    map
});

fn resolve_function(
    path_converter: &RepoPathUiConverter,
    function: &FunctionCallNode,
) -> FilesetParseResult<FilesetExpression> {
    if let Some(func) = BUILTIN_FUNCTION_MAP.get(function.name) {
        func(path_converter, function)
    } else {
        Err(FilesetParseError::new(
            FilesetParseErrorKind::NoSuchFunction {
                name: function.name.to_owned(),
                candidates: collect_similar(function.name, BUILTIN_FUNCTION_MAP.keys()),
            },
            function.name_span,
        ))
    }
}

fn resolve_expression(
    path_converter: &RepoPathUiConverter,
    node: &ExpressionNode,
) -> FilesetParseResult<FilesetExpression> {
    let wrap_pattern_error =
        |err| FilesetParseError::expression("Invalid file pattern", node.span).with_source(err);
    match &node.kind {
        ExpressionKind::Identifier(name) => {
            let pattern =
                FilePattern::cwd_prefix_path(path_converter, name).map_err(wrap_pattern_error)?;
            Ok(FilesetExpression::pattern(pattern))
        }
        ExpressionKind::String(name) => {
            let pattern =
                FilePattern::cwd_prefix_path(path_converter, name).map_err(wrap_pattern_error)?;
            Ok(FilesetExpression::pattern(pattern))
        }
        ExpressionKind::StringPattern { kind, value } => {
            let pattern = FilePattern::from_str_kind(path_converter, value, kind)
                .map_err(wrap_pattern_error)?;
            Ok(FilesetExpression::pattern(pattern))
        }
        ExpressionKind::Unary(op, arg_node) => {
            let arg = resolve_expression(path_converter, arg_node)?;
            match op {
                UnaryOp::Negate => Ok(FilesetExpression::all().difference(arg)),
            }
        }
        ExpressionKind::Binary(op, lhs_node, rhs_node) => {
            let lhs = resolve_expression(path_converter, lhs_node)?;
            let rhs = resolve_expression(path_converter, rhs_node)?;
            match op {
                BinaryOp::Union => Ok(lhs.union(rhs)),
                BinaryOp::Intersection => Ok(lhs.intersection(rhs)),
                BinaryOp::Difference => Ok(lhs.difference(rhs)),
            }
        }
        ExpressionKind::FunctionCall(function) => resolve_function(path_converter, function),
    }
}

/// Parses text into `FilesetExpression` with bare string fallback.
///
/// If the text can't be parsed as a fileset expression, and if it doesn't
/// contain any operator-like characters, it will be parsed as a file path.
pub fn parse_maybe_bare(
    text: &str,
    path_converter: &RepoPathUiConverter,
) -> FilesetParseResult<FilesetExpression> {
    let node = fileset_parser::parse_program_or_bare_string(text)?;
    // TODO: add basic tree substitution pass to eliminate redundant expressions
    resolve_expression(path_converter, &node)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn repo_path_buf(value: impl Into<String>) -> RepoPathBuf {
        RepoPathBuf::from_internal_string(value)
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
    fn test_parse_file_pattern() {
        let path_converter = RepoPathUiConverter::Fs {
            cwd: PathBuf::from("/ws/cur"),
            base: PathBuf::from("/ws"),
        };
        let parse = |text| parse_maybe_bare(text, &path_converter);

        // cwd-relative patterns
        assert_eq!(
            parse(".").unwrap(),
            FilesetExpression::prefix_path(repo_path_buf("cur"))
        );
        assert_eq!(
            parse("..").unwrap(),
            FilesetExpression::prefix_path(RepoPathBuf::root())
        );
        assert!(parse("../..").is_err());
        assert_eq!(
            parse("foo").unwrap(),
            FilesetExpression::prefix_path(repo_path_buf("cur/foo"))
        );
        assert_eq!(
            parse("cwd:.").unwrap(),
            FilesetExpression::prefix_path(repo_path_buf("cur"))
        );
        assert_eq!(
            parse("cwd-file:foo").unwrap(),
            FilesetExpression::file_path(repo_path_buf("cur/foo"))
        );
        assert_eq!(
            parse("file:../foo/bar").unwrap(),
            FilesetExpression::file_path(repo_path_buf("foo/bar"))
        );

        // workspace-relative patterns
        assert_eq!(
            parse("root:.").unwrap(),
            FilesetExpression::prefix_path(RepoPathBuf::root())
        );
        assert!(parse("root:..").is_err());
        assert_eq!(
            parse("root:foo/bar").unwrap(),
            FilesetExpression::prefix_path(repo_path_buf("foo/bar"))
        );
        assert_eq!(
            parse("root-file:bar").unwrap(),
            FilesetExpression::file_path(repo_path_buf("bar"))
        );
    }

    #[test]
    fn test_parse_glob_pattern() {
        let path_converter = RepoPathUiConverter::Fs {
            // meta character in cwd path shouldn't be expanded
            cwd: PathBuf::from("/ws/cur*"),
            base: PathBuf::from("/ws"),
        };
        let parse = |text| parse_maybe_bare(text, &path_converter);
        let glob_expr = |dir: &str, pattern: &str| {
            FilesetExpression::pattern(FilePattern::FileGlob {
                dir: repo_path_buf(dir),
                pattern: glob::Pattern::new(pattern).unwrap(),
            })
        };

        // cwd-relative, without meta characters
        assert_eq!(
            parse(r#"cwd-glob:"foo""#).unwrap(),
            FilesetExpression::file_path(repo_path_buf("cur*/foo"))
        );
        // Strictly speaking, glob:"" shouldn't match a file named <cwd>, but
        // file pattern doesn't distinguish "foo/" from "foo".
        assert_eq!(
            parse(r#"glob:"""#).unwrap(),
            FilesetExpression::file_path(repo_path_buf("cur*"))
        );
        assert_eq!(
            parse(r#"glob:".""#).unwrap(),
            FilesetExpression::file_path(repo_path_buf("cur*"))
        );
        assert_eq!(
            parse(r#"glob:"..""#).unwrap(),
            FilesetExpression::file_path(RepoPathBuf::root())
        );

        // cwd-relative, with meta characters
        assert_eq!(parse(r#"glob:"*""#).unwrap(), glob_expr("cur*", "*"));
        assert_eq!(parse(r#"glob:"./*""#).unwrap(), glob_expr("cur*", "*"));
        assert_eq!(parse(r#"glob:"../*""#).unwrap(), glob_expr("", "*"));
        // glob:"**" is equivalent to root-glob:"<cwd>/**", not root-glob:"**"
        assert_eq!(parse(r#"glob:"**""#).unwrap(), glob_expr("cur*", "**"));
        assert_eq!(
            parse(r#"glob:"../foo/b?r/baz""#).unwrap(),
            glob_expr("foo", "b?r/baz")
        );
        assert!(parse(r#"glob:"../../*""#).is_err());
        assert!(parse(r#"glob:"/*""#).is_err());
        // no support for relative path component after glob meta character
        assert!(parse(r#"glob:"*/..""#).is_err());

        // cwd-relative, with Windows path separators
        assert_eq!(
            parse(r#"glob:"..\\foo\\*\\bar""#).unwrap(),
            if cfg!(windows) {
                glob_expr("foo", "*/bar")
            } else {
                glob_expr("cur*", r"..\foo\*\bar")
            }
        );

        // workspace-relative, without meta characters
        assert_eq!(
            parse(r#"root-glob:"foo""#).unwrap(),
            FilesetExpression::file_path(repo_path_buf("foo"))
        );
        assert_eq!(
            parse(r#"root-glob:"""#).unwrap(),
            FilesetExpression::file_path(RepoPathBuf::root())
        );
        assert_eq!(
            parse(r#"root-glob:".""#).unwrap(),
            FilesetExpression::file_path(RepoPathBuf::root())
        );

        // workspace-relative, with meta characters
        assert_eq!(parse(r#"root-glob:"*""#).unwrap(), glob_expr("", "*"));
        assert_eq!(
            parse(r#"root-glob:"foo/bar/b[az]""#).unwrap(),
            glob_expr("foo/bar", "b[az]")
        );
        assert!(parse(r#"root-glob:"../*""#).is_err());
        assert!(parse(r#"root-glob:"/*""#).is_err());
    }

    #[test]
    fn test_parse_function() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let path_converter = RepoPathUiConverter::Fs {
            cwd: PathBuf::from("/ws/cur"),
            base: PathBuf::from("/ws"),
        };
        let parse = |text| parse_maybe_bare(text, &path_converter);

        assert_eq!(parse("all()").unwrap(), FilesetExpression::all());
        assert_eq!(parse("none()").unwrap(), FilesetExpression::none());
        insta::assert_debug_snapshot!(parse("all(x)").unwrap_err().kind(), @r###"
        InvalidArguments {
            name: "all",
            message: "Expected 0 arguments",
        }
        "###);
        insta::assert_debug_snapshot!(parse("ale()").unwrap_err().kind(), @r###"
        NoSuchFunction {
            name: "ale",
            candidates: [
                "all",
            ],
        }
        "###);
    }

    #[test]
    fn test_parse_compound_expression() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let path_converter = RepoPathUiConverter::Fs {
            cwd: PathBuf::from("/ws/cur"),
            base: PathBuf::from("/ws"),
        };
        let parse = |text| parse_maybe_bare(text, &path_converter);

        insta::assert_debug_snapshot!(parse("~x").unwrap(), @r###"
        Difference(
            All,
            Pattern(PrefixPath("cur/x")),
        )
        "###);
        insta::assert_debug_snapshot!(parse("x|y|root:z").unwrap(), @r###"
        UnionAll(
            [
                Pattern(PrefixPath("cur/x")),
                Pattern(PrefixPath("cur/y")),
                Pattern(PrefixPath("z")),
            ],
        )
        "###);
        insta::assert_debug_snapshot!(parse("x|y&z").unwrap(), @r###"
        UnionAll(
            [
                Pattern(PrefixPath("cur/x")),
                Intersection(
                    Pattern(PrefixPath("cur/y")),
                    Pattern(PrefixPath("cur/z")),
                ),
            ],
        )
        "###);
    }

    #[test]
    fn test_explicit_paths() {
        let collect = |expr: &FilesetExpression| -> Vec<RepoPathBuf> {
            expr.explicit_paths().map(|path| path.to_owned()).collect()
        };
        let file_expr = |path: &str| FilesetExpression::file_path(repo_path_buf(path));
        assert!(collect(&FilesetExpression::none()).is_empty());
        assert_eq!(collect(&file_expr("a")), ["a"].map(repo_path_buf));
        assert_eq!(
            collect(&FilesetExpression::union_all(vec![
                file_expr("a"),
                file_expr("b"),
                file_expr("c"),
            ])),
            ["a", "b", "c"].map(repo_path_buf)
        );
        assert_eq!(
            collect(&FilesetExpression::intersection(
                FilesetExpression::union_all(vec![
                    file_expr("a"),
                    FilesetExpression::none(),
                    file_expr("b"),
                    file_expr("c"),
                ]),
                FilesetExpression::difference(
                    file_expr("d"),
                    FilesetExpression::union_all(vec![file_expr("e"), file_expr("f")])
                )
            )),
            ["a", "b", "c", "d", "e", "f"].map(repo_path_buf)
        );
    }

    #[test]
    fn test_build_matcher_simple() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        insta::assert_debug_snapshot!(FilesetExpression::none().to_matcher(), @"NothingMatcher");
        insta::assert_debug_snapshot!(FilesetExpression::all().to_matcher(), @"EverythingMatcher");
        insta::assert_debug_snapshot!(
            FilesetExpression::file_path(repo_path_buf("foo")).to_matcher(),
            @r###"
        FilesMatcher {
            tree: Dir {
                "foo": File {},
            },
        }
        "###);
        insta::assert_debug_snapshot!(
            FilesetExpression::prefix_path(repo_path_buf("foo")).to_matcher(),
            @r###"
        PrefixMatcher {
            tree: Dir {
                "foo": Prefix {},
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_glob_pattern() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let glob_expr = |dir: &str, pattern: &str| {
            FilesetExpression::pattern(FilePattern::FileGlob {
                dir: repo_path_buf(dir),
                pattern: glob::Pattern::new(pattern).unwrap(),
            })
        };

        insta::assert_debug_snapshot!(glob_expr("", "*").to_matcher(), @r###"
        FileGlobsMatcher {
            tree: [
                Pattern {
                    original: "*",
                    tokens: [
                        AnySequence,
                    ],
                    is_recursive: false,
                },
            ] {},
        }
        "###);

        let expr =
            FilesetExpression::union_all(vec![glob_expr("foo", "*"), glob_expr("foo/bar", "*")]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        FileGlobsMatcher {
            tree: [] {
                "foo": [
                    Pattern {
                        original: "*",
                        tokens: [
                            AnySequence,
                        ],
                        is_recursive: false,
                    },
                ] {
                    "bar": [
                        Pattern {
                            original: "*",
                            tokens: [
                                AnySequence,
                            ],
                            is_recursive: false,
                        },
                    ] {},
                },
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_union_patterns_of_same_kind() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        let expr = FilesetExpression::union_all(vec![
            FilesetExpression::file_path(repo_path_buf("foo")),
            FilesetExpression::file_path(repo_path_buf("foo/bar")),
        ]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        FilesMatcher {
            tree: Dir {
                "foo": File {
                    "bar": File {},
                },
            },
        }
        "###);

        let expr = FilesetExpression::union_all(vec![
            FilesetExpression::prefix_path(repo_path_buf("bar")),
            FilesetExpression::prefix_path(repo_path_buf("bar/baz")),
        ]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        PrefixMatcher {
            tree: Dir {
                "bar": Prefix {
                    "baz": Prefix {},
                },
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_union_patterns_of_different_kind() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        let expr = FilesetExpression::union_all(vec![
            FilesetExpression::file_path(repo_path_buf("foo")),
            FilesetExpression::prefix_path(repo_path_buf("bar")),
        ]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        UnionMatcher {
            input1: FilesMatcher {
                tree: Dir {
                    "foo": File {},
                },
            },
            input2: PrefixMatcher {
                tree: Dir {
                    "bar": Prefix {},
                },
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_unnormalized_union() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        let expr = FilesetExpression::UnionAll(vec![]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @"NothingMatcher");

        let expr =
            FilesetExpression::UnionAll(vec![FilesetExpression::None, FilesetExpression::All]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        UnionMatcher {
            input1: NothingMatcher,
            input2: EverythingMatcher,
        }
        "###);
    }

    #[test]
    fn test_build_matcher_combined() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();

        let expr = FilesetExpression::union_all(vec![
            FilesetExpression::intersection(FilesetExpression::all(), FilesetExpression::none()),
            FilesetExpression::difference(FilesetExpression::none(), FilesetExpression::all()),
            FilesetExpression::file_path(repo_path_buf("foo")),
            FilesetExpression::prefix_path(repo_path_buf("bar")),
        ]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        UnionMatcher {
            input1: UnionMatcher {
                input1: IntersectionMatcher {
                    input1: EverythingMatcher,
                    input2: NothingMatcher,
                },
                input2: DifferenceMatcher {
                    wanted: NothingMatcher,
                    unwanted: EverythingMatcher,
                },
            },
            input2: UnionMatcher {
                input1: FilesMatcher {
                    tree: Dir {
                        "foo": File {},
                    },
                },
                input2: PrefixMatcher {
                    tree: Dir {
                        "bar": Prefix {},
                    },
                },
            },
        }
        "###);
    }
}
