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

use std::path::Path;
use std::slice;

use thiserror::Error;

pub use crate::fileset_parser::{FilesetParseError, FilesetParseErrorKind, FilesetParseResult};
use crate::matchers::{
    DifferenceMatcher, EverythingMatcher, FilesMatcher, IntersectionMatcher, Matcher,
    NothingMatcher, PrefixMatcher, UnionMatcher,
};
use crate::repo_path::{FsPathParseError, RelativePathParseError, RepoPathBuf};

/// Error occurred during file pattern parsing.
#[derive(Debug, Error)]
pub enum FilePatternParseError {
    /// Unknown pattern kind is specified.
    #[error(r#"Invalid file pattern kind "{0}:""#)]
    InvalidKind(String),
    /// Failed to parse input cwd-relative path.
    #[error(transparent)]
    FsPath(#[from] FsPathParseError),
    /// Failed to parse input workspace-relative path.
    #[error(transparent)]
    RelativePath(#[from] RelativePathParseError),
}

/// Basic pattern to match `RepoPath`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilePattern {
    /// Matches file (or exact) path.
    FilePath(RepoPathBuf),
    /// Matches path prefix.
    PrefixPath(RepoPathBuf),
    // TODO: add more patterns:
    // - FilesInPath: files in directory, non-recursively?
    // - FileGlob: file (or exact) path with glob?
    // - NameGlob or SuffixGlob: file name with glob?
}

impl FilePattern {
    /// Parses the given `input` string as a file pattern.
    // TODO: If we decide to parse any file argument as a fileset expression,
    // this function can be removed.
    pub fn parse(ctx: &FilesetParseContext, input: &str) -> Result<Self, FilePatternParseError> {
        if let Some((kind, pat)) = input.split_once(':') {
            Self::from_str_kind(ctx, pat, kind)
        } else {
            Self::cwd_prefix_path(ctx, input)
        }
    }

    /// Parses the given `input` string as pattern of the specified `kind`.
    pub fn from_str_kind(
        ctx: &FilesetParseContext,
        input: &str,
        kind: &str,
    ) -> Result<Self, FilePatternParseError> {
        // Naming convention:
        // * path normalization
        //   * cwd: cwd-relative path (default)
        //   * root: workspace-relative path
        // * where to anchor
        //   * file: exact file path
        //   * prefix: path prefix (files under directory recursively) (default)
        //   * files-in: files in directory non-recursively
        //   * name: file name component (or suffix match?)
        //   * substring: substring match?
        // * string pattern syntax (+ case sensitivity?)
        //   * path: literal path (default)
        //   * glob
        //   * regex?
        match kind {
            "cwd" => Self::cwd_prefix_path(ctx, input),
            "cwd-file" | "file" => Self::cwd_file_path(ctx, input),
            "root" => Self::root_prefix_path(input),
            "root-file" => Self::root_file_path(input),
            _ => Err(FilePatternParseError::InvalidKind(kind.to_owned())),
        }
    }

    /// Pattern that matches cwd-relative file (or exact) path.
    pub fn cwd_file_path(
        ctx: &FilesetParseContext,
        input: impl AsRef<Path>,
    ) -> Result<Self, FilePatternParseError> {
        let path = ctx.parse_cwd_path(input)?;
        Ok(FilePattern::FilePath(path))
    }

    /// Pattern that matches cwd-relative path prefix.
    pub fn cwd_prefix_path(
        ctx: &FilesetParseContext,
        input: impl AsRef<Path>,
    ) -> Result<Self, FilePatternParseError> {
        let path = ctx.parse_cwd_path(input)?;
        Ok(FilePattern::PrefixPath(path))
    }

    /// Pattern that matches workspace-relative file (or exact) path.
    pub fn root_file_path(input: impl AsRef<Path>) -> Result<Self, FilePatternParseError> {
        let path = RepoPathBuf::from_relative_path(input)?;
        Ok(FilePattern::FilePath(path))
    }

    /// Pattern that matches workspace-relative path prefix.
    pub fn root_prefix_path(input: impl AsRef<Path>) -> Result<Self, FilePatternParseError> {
        let path = RepoPathBuf::from_relative_path(input)?;
        Ok(FilePattern::PrefixPath(path))
    }
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

/// Environment where fileset expression is parsed.
#[derive(Clone, Debug)]
pub struct FilesetParseContext<'a> {
    /// Normalized path to the current working directory.
    pub cwd: &'a Path,
    /// Normalized path to the workspace root.
    pub workspace_root: &'a Path,
}

impl FilesetParseContext<'_> {
    fn parse_cwd_path(&self, input: impl AsRef<Path>) -> Result<RepoPathBuf, FsPathParseError> {
        RepoPathBuf::parse_fs_path(self.cwd, self.workspace_root, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_path_buf(value: impl Into<String>) -> RepoPathBuf {
        RepoPathBuf::from_internal_string(value)
    }

    #[test]
    fn test_parse_file_pattern() {
        let ctx = FilesetParseContext {
            cwd: Path::new("/ws/cur"),
            workspace_root: Path::new("/ws"),
        };
        // TODO: implement fileset expression parser and test it instead
        let parse = |input| FilePattern::parse(&ctx, input).map(FilesetExpression::pattern);

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
    fn test_build_matcher_simple() {
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
                "foo": Dir|File {},
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_union_patterns_of_same_kind() {
        let expr = FilesetExpression::union_all(vec![
            FilesetExpression::file_path(repo_path_buf("foo")),
            FilesetExpression::file_path(repo_path_buf("foo/bar")),
        ]);
        insta::assert_debug_snapshot!(expr.to_matcher(), @r###"
        FilesMatcher {
            tree: Dir {
                "foo": Dir|File {
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
                "bar": Dir|File {
                    "baz": Dir|File {},
                },
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_union_patterns_of_different_kind() {
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
                    "bar": Dir|File {},
                },
            },
        }
        "###);
    }

    #[test]
    fn test_build_matcher_unnormalized_union() {
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
                        "bar": Dir|File {},
                    },
                },
            },
        }
        "###);
    }
}
