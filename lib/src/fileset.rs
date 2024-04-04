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

use std::slice;

use crate::matchers::{
    DifferenceMatcher, EverythingMatcher, FilesMatcher, IntersectionMatcher, Matcher,
    NothingMatcher, PrefixMatcher, UnionMatcher,
};
use crate::repo_path::RepoPathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_path_buf(value: impl Into<String>) -> RepoPathBuf {
        RepoPathBuf::from_internal_string(value)
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
