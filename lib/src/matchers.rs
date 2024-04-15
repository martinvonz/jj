// Copyright 2020 The Jujutsu Authors
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

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::{fmt, iter};

use itertools::Itertools as _;
use tracing::instrument;

use crate::repo_path::{RepoPath, RepoPathComponentBuf};

#[derive(PartialEq, Eq, Debug)]
pub enum Visit {
    /// Everything in the directory is *guaranteed* to match, no need to check
    /// descendants
    AllRecursively,
    Specific {
        dirs: VisitDirs,
        files: VisitFiles,
    },
    /// Nothing in the directory or its subdirectories will match.
    ///
    /// This is the same as `Specific` with no directories or files. Use
    /// `Visit::set()` to get create an instance that's `Specific` or
    /// `Nothing` depending on the values at runtime.
    Nothing,
}

impl Visit {
    fn sets(dirs: HashSet<RepoPathComponentBuf>, files: HashSet<RepoPathComponentBuf>) -> Self {
        if dirs.is_empty() && files.is_empty() {
            Self::Nothing
        } else {
            Self::Specific {
                dirs: VisitDirs::Set(dirs),
                files: VisitFiles::Set(files),
            }
        }
    }

    pub fn is_nothing(&self) -> bool {
        *self == Visit::Nothing
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitDirs {
    All,
    Set(HashSet<RepoPathComponentBuf>),
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitFiles {
    All,
    Set(HashSet<RepoPathComponentBuf>),
}

pub trait Matcher: Debug + Sync {
    fn matches(&self, file: &RepoPath) -> bool;
    fn visit(&self, dir: &RepoPath) -> Visit;
}

impl<T: Matcher + ?Sized> Matcher for &T {
    fn matches(&self, file: &RepoPath) -> bool {
        <T as Matcher>::matches(self, file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        <T as Matcher>::visit(self, dir)
    }
}

impl<T: Matcher + ?Sized> Matcher for Box<T> {
    fn matches(&self, file: &RepoPath) -> bool {
        <T as Matcher>::matches(self, file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        <T as Matcher>::visit(self, dir)
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct NothingMatcher;

impl Matcher for NothingMatcher {
    fn matches(&self, _file: &RepoPath) -> bool {
        false
    }

    fn visit(&self, _dir: &RepoPath) -> Visit {
        Visit::Nothing
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct EverythingMatcher;

impl Matcher for EverythingMatcher {
    fn matches(&self, _file: &RepoPath) -> bool {
        true
    }

    fn visit(&self, _dir: &RepoPath) -> Visit {
        Visit::AllRecursively
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct FilesMatcher {
    tree: RepoPathTree<FilesNodeKind>,
}

impl FilesMatcher {
    pub fn new(files: impl IntoIterator<Item = impl AsRef<RepoPath>>) -> Self {
        let mut tree = RepoPathTree::default();
        for f in files {
            tree.add(f.as_ref()).value = FilesNodeKind::File;
        }
        FilesMatcher { tree }
    }
}

impl Matcher for FilesMatcher {
    fn matches(&self, file: &RepoPath) -> bool {
        self.tree
            .get(file)
            .map_or(false, |sub| sub.value == FilesNodeKind::File)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        self.tree
            .get(dir)
            .map_or(Visit::Nothing, files_tree_to_visit_sets)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FilesNodeKind {
    /// Represents an intermediate directory.
    #[default]
    Dir,
    /// Represents a file (which might also be an intermediate directory.)
    File,
}

fn files_tree_to_visit_sets(tree: &RepoPathTree<FilesNodeKind>) -> Visit {
    let mut dirs = HashSet::new();
    let mut files = HashSet::new();
    for (name, sub) in &tree.entries {
        // should visit only intermediate directories
        if !sub.entries.is_empty() {
            dirs.insert(name.clone());
        }
        if sub.value == FilesNodeKind::File {
            files.insert(name.clone());
        }
    }
    Visit::sets(dirs, files)
}

#[derive(Debug)]
pub struct PrefixMatcher {
    tree: RepoPathTree<PrefixNodeKind>,
}

impl PrefixMatcher {
    #[instrument(skip(prefixes))]
    pub fn new(prefixes: impl IntoIterator<Item = impl AsRef<RepoPath>>) -> Self {
        let mut tree = RepoPathTree::default();
        for prefix in prefixes {
            tree.add(prefix.as_ref()).value = PrefixNodeKind::Prefix;
        }
        PrefixMatcher { tree }
    }
}

impl Matcher for PrefixMatcher {
    fn matches(&self, file: &RepoPath) -> bool {
        self.tree
            .walk_to(file)
            .any(|(sub, _)| sub.value == PrefixNodeKind::Prefix)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        for (sub, tail_path) in self.tree.walk_to(dir) {
            // ancestor of 'dir' matches prefix paths
            if sub.value == PrefixNodeKind::Prefix {
                return Visit::AllRecursively;
            }
            // 'dir' found, and is an ancestor of prefix paths
            if tail_path.is_root() {
                return prefix_tree_to_visit_sets(sub);
            }
        }
        Visit::Nothing
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PrefixNodeKind {
    /// Represents an intermediate directory.
    #[default]
    Dir,
    /// Represents a file and prefix directory.
    Prefix,
}

fn prefix_tree_to_visit_sets(tree: &RepoPathTree<PrefixNodeKind>) -> Visit {
    let mut dirs = HashSet::new();
    let mut files = HashSet::new();
    for (name, sub) in &tree.entries {
        // should visit both intermediate and prefix directories
        dirs.insert(name.clone());
        if sub.value == PrefixNodeKind::Prefix {
            files.insert(name.clone());
        }
    }
    Visit::sets(dirs, files)
}

/// Matches paths that are matched by any of the input matchers.
#[derive(Clone, Debug)]
pub struct UnionMatcher<M1, M2> {
    input1: M1,
    input2: M2,
}

impl<M1: Matcher, M2: Matcher> UnionMatcher<M1, M2> {
    pub fn new(input1: M1, input2: M2) -> Self {
        Self { input1, input2 }
    }
}

impl<M1: Matcher, M2: Matcher> Matcher for UnionMatcher<M1, M2> {
    fn matches(&self, file: &RepoPath) -> bool {
        self.input1.matches(file) || self.input2.matches(file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        match self.input1.visit(dir) {
            Visit::AllRecursively => Visit::AllRecursively,
            Visit::Nothing => self.input2.visit(dir),
            Visit::Specific {
                dirs: dirs1,
                files: files1,
            } => match self.input2.visit(dir) {
                Visit::AllRecursively => Visit::AllRecursively,
                Visit::Nothing => Visit::Specific {
                    dirs: dirs1,
                    files: files1,
                },
                Visit::Specific {
                    dirs: dirs2,
                    files: files2,
                } => {
                    let dirs = match (dirs1, dirs2) {
                        (VisitDirs::All, _) | (_, VisitDirs::All) => VisitDirs::All,
                        (VisitDirs::Set(dirs1), VisitDirs::Set(dirs2)) => {
                            VisitDirs::Set(dirs1.iter().chain(&dirs2).cloned().collect())
                        }
                    };
                    let files = match (files1, files2) {
                        (VisitFiles::All, _) | (_, VisitFiles::All) => VisitFiles::All,
                        (VisitFiles::Set(files1), VisitFiles::Set(files2)) => {
                            VisitFiles::Set(files1.iter().chain(&files2).cloned().collect())
                        }
                    };
                    Visit::Specific { dirs, files }
                }
            },
        }
    }
}

/// Matches paths that are matched by the first input matcher but not by the
/// second.
#[derive(Clone, Debug)]
pub struct DifferenceMatcher<M1, M2> {
    /// The minuend
    wanted: M1,
    /// The subtrahend
    unwanted: M2,
}

impl<M1: Matcher, M2: Matcher> DifferenceMatcher<M1, M2> {
    pub fn new(wanted: M1, unwanted: M2) -> Self {
        Self { wanted, unwanted }
    }
}

impl<M1: Matcher, M2: Matcher> Matcher for DifferenceMatcher<M1, M2> {
    fn matches(&self, file: &RepoPath) -> bool {
        self.wanted.matches(file) && !self.unwanted.matches(file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        match self.unwanted.visit(dir) {
            Visit::AllRecursively => Visit::Nothing,
            Visit::Nothing => self.wanted.visit(dir),
            Visit::Specific { .. } => match self.wanted.visit(dir) {
                Visit::AllRecursively => Visit::Specific {
                    dirs: VisitDirs::All,
                    files: VisitFiles::All,
                },
                wanted_visit => wanted_visit,
            },
        }
    }
}

/// Matches paths that are matched by both input matchers.
#[derive(Clone, Debug)]
pub struct IntersectionMatcher<M1, M2> {
    input1: M1,
    input2: M2,
}

impl<M1: Matcher, M2: Matcher> IntersectionMatcher<M1, M2> {
    pub fn new(input1: M1, input2: M2) -> Self {
        Self { input1, input2 }
    }
}

impl<M1: Matcher, M2: Matcher> Matcher for IntersectionMatcher<M1, M2> {
    fn matches(&self, file: &RepoPath) -> bool {
        self.input1.matches(file) && self.input2.matches(file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        match self.input1.visit(dir) {
            Visit::AllRecursively => self.input2.visit(dir),
            Visit::Nothing => Visit::Nothing,
            Visit::Specific {
                dirs: dirs1,
                files: files1,
            } => match self.input2.visit(dir) {
                Visit::AllRecursively => Visit::Specific {
                    dirs: dirs1,
                    files: files1,
                },
                Visit::Nothing => Visit::Nothing,
                Visit::Specific {
                    dirs: dirs2,
                    files: files2,
                } => {
                    let dirs = match (dirs1, dirs2) {
                        (VisitDirs::All, VisitDirs::All) => VisitDirs::All,
                        (dirs1, VisitDirs::All) => dirs1,
                        (VisitDirs::All, dirs2) => dirs2,
                        (VisitDirs::Set(dirs1), VisitDirs::Set(dirs2)) => {
                            VisitDirs::Set(dirs1.intersection(&dirs2).cloned().collect())
                        }
                    };
                    let files = match (files1, files2) {
                        (VisitFiles::All, VisitFiles::All) => VisitFiles::All,
                        (files1, VisitFiles::All) => files1,
                        (VisitFiles::All, files2) => files2,
                        (VisitFiles::Set(files1), VisitFiles::Set(files2)) => {
                            VisitFiles::Set(files1.intersection(&files2).cloned().collect())
                        }
                    };
                    match (&dirs, &files) {
                        (VisitDirs::Set(dirs), VisitFiles::Set(files))
                            if dirs.is_empty() && files.is_empty() =>
                        {
                            Visit::Nothing
                        }
                        _ => Visit::Specific { dirs, files },
                    }
                }
            },
        }
    }
}

/// Tree that maps `RepoPath` to value of type `V`.
#[derive(Clone, Default, Eq, PartialEq)]
struct RepoPathTree<V> {
    entries: HashMap<RepoPathComponentBuf, RepoPathTree<V>>,
    value: V,
}

impl<V> RepoPathTree<V> {
    fn add(&mut self, dir: &RepoPath) -> &mut Self
    where
        V: Default,
    {
        dir.components().fold(self, |sub, name| {
            // Avoid name.clone() if entry already exists.
            if !sub.entries.contains_key(name) {
                sub.entries.insert(name.to_owned(), Self::default());
            }
            sub.entries.get_mut(name).unwrap()
        })
    }

    fn get(&self, dir: &RepoPath) -> Option<&Self> {
        dir.components()
            .try_fold(self, |sub, name| sub.entries.get(name))
    }

    /// Walks the tree from the root to the given `dir`, yielding each sub tree
    /// and remaining path.
    fn walk_to<'a, 'b: 'a>(
        &'a self,
        dir: &'b RepoPath,
    ) -> impl Iterator<Item = (&'a Self, &'b RepoPath)> + 'a {
        iter::successors(Some((self, dir)), |(sub, dir)| {
            let mut components = dir.components();
            let name = components.next()?;
            Some((sub.entries.get(name)?, components.as_path()))
        })
    }
}

impl<V: Debug> Debug for RepoPathTree<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)?;
        f.write_str(" ")?;
        f.debug_map()
            .entries(
                self.entries
                    .iter()
                    .sorted_unstable_by_key(|&(name, _)| name),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use maplit::hashset;

    use super::*;

    fn repo_path(value: &str) -> &RepoPath {
        RepoPath::from_internal_string(value)
    }

    #[test]
    fn test_nothingmatcher() {
        let m = NothingMatcher;
        assert!(!m.matches(repo_path("file")));
        assert!(!m.matches(repo_path("dir/file")));
        assert_eq!(m.visit(RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_filesmatcher_empty() {
        let m = FilesMatcher::new([] as [&RepoPath; 0]);
        assert!(!m.matches(repo_path("file")));
        assert!(!m.matches(repo_path("dir/file")));
        assert_eq!(m.visit(RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_filesmatcher_nonempty() {
        let m = FilesMatcher::new([
            repo_path("dir1/subdir1/file1"),
            repo_path("dir1/subdir1/file2"),
            repo_path("dir1/subdir2/file3"),
            repo_path("file4"),
        ]);

        assert!(!m.matches(repo_path("dir1")));
        assert!(!m.matches(repo_path("dir1/subdir1")));
        assert!(m.matches(repo_path("dir1/subdir1/file1")));
        assert!(m.matches(repo_path("dir1/subdir1/file2")));
        assert!(!m.matches(repo_path("dir1/subdir1/file3")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("dir1")},
                hashset! {RepoPathComponentBuf::from("file4")}
            )
        );
        assert_eq!(
            m.visit(repo_path("dir1")),
            Visit::sets(
                hashset! {
                    RepoPathComponentBuf::from("subdir1"),
                    RepoPathComponentBuf::from("subdir2"),
                },
                hashset! {}
            )
        );
        assert_eq!(
            m.visit(repo_path("dir1/subdir1")),
            Visit::sets(
                hashset! {},
                hashset! {
                    RepoPathComponentBuf::from("file1"),
                    RepoPathComponentBuf::from("file2"),
                },
            )
        );
        assert_eq!(
            m.visit(repo_path("dir1/subdir2")),
            Visit::sets(hashset! {}, hashset! {RepoPathComponentBuf::from("file3")})
        );
    }

    #[test]
    fn test_prefixmatcher_empty() {
        let m = PrefixMatcher::new([] as [&RepoPath; 0]);
        assert!(!m.matches(repo_path("file")));
        assert!(!m.matches(repo_path("dir/file")));
        assert_eq!(m.visit(RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_prefixmatcher_root() {
        let m = PrefixMatcher::new([RepoPath::root()]);
        // Matches all files
        assert!(m.matches(repo_path("file")));
        assert!(m.matches(repo_path("dir/file")));
        // Visits all directories
        assert_eq!(m.visit(RepoPath::root()), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::AllRecursively);
    }

    #[test]
    fn test_prefixmatcher_single_prefix() {
        let m = PrefixMatcher::new([repo_path("foo/bar")]);

        // Parts of the prefix should not match
        assert!(!m.matches(repo_path("foo")));
        assert!(!m.matches(repo_path("bar")));
        // A file matching the prefix exactly should match
        assert!(m.matches(repo_path("foo/bar")));
        // Files in subdirectories should match
        assert!(m.matches(repo_path("foo/bar/baz")));
        assert!(m.matches(repo_path("foo/bar/baz/qux")));
        // Sibling files should not match
        assert!(!m.matches(repo_path("foo/foo")));
        // An unrooted "foo/bar" should not match
        assert!(!m.matches(repo_path("bar/foo/bar")));

        // The matcher should only visit directory foo/ in the root (file "foo"
        // shouldn't be visited)
        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(hashset! {RepoPathComponentBuf::from("foo")}, hashset! {})
        );
        // Inside parent directory "foo/", both subdirectory "bar" and file "bar" may
        // match
        assert_eq!(
            m.visit(repo_path("foo")),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("bar")},
                hashset! {RepoPathComponentBuf::from("bar")}
            )
        );
        // Inside a directory that matches the prefix, everything matches recursively
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::AllRecursively);
        // Same thing in subdirectories of the prefix
        assert_eq!(m.visit(repo_path("foo/bar/baz")), Visit::AllRecursively);
        // Nothing in directories that are siblings of the prefix can match, so don't
        // visit
        assert_eq!(m.visit(repo_path("bar")), Visit::Nothing);
    }

    #[test]
    fn test_prefixmatcher_nested_prefixes() {
        let m = PrefixMatcher::new([repo_path("foo"), repo_path("foo/bar/baz")]);

        assert!(m.matches(repo_path("foo")));
        assert!(!m.matches(repo_path("bar")));
        assert!(m.matches(repo_path("foo/bar")));
        // Matches because the "foo" pattern matches
        assert!(m.matches(repo_path("foo/baz/foo")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("foo")},
                hashset! {RepoPathComponentBuf::from("foo")}
            )
        );
        // Inside a directory that matches the prefix, everything matches recursively
        assert_eq!(m.visit(repo_path("foo")), Visit::AllRecursively);
        // Same thing in subdirectories of the prefix
        assert_eq!(m.visit(repo_path("foo/bar/baz")), Visit::AllRecursively);
    }

    #[test]
    fn test_unionmatcher_concatenate_roots() {
        let m1 = PrefixMatcher::new([repo_path("foo"), repo_path("bar")]);
        let m2 = PrefixMatcher::new([repo_path("bar"), repo_path("baz")]);
        let m = UnionMatcher::new(&m1, &m2);

        assert!(m.matches(repo_path("foo")));
        assert!(m.matches(repo_path("foo/bar")));
        assert!(m.matches(repo_path("bar")));
        assert!(m.matches(repo_path("bar/foo")));
        assert!(m.matches(repo_path("baz")));
        assert!(m.matches(repo_path("baz/foo")));
        assert!(!m.matches(repo_path("qux")));
        assert!(!m.matches(repo_path("qux/foo")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {
                    RepoPathComponentBuf::from("foo"),
                    RepoPathComponentBuf::from("bar"),
                    RepoPathComponentBuf::from("baz"),
                },
                hashset! {
                    RepoPathComponentBuf::from("foo"),
                    RepoPathComponentBuf::from("bar"),
                    RepoPathComponentBuf::from("baz"),
                },
            )
        );
        assert_eq!(m.visit(repo_path("foo")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("bar")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("bar/foo")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("baz")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("baz/foo")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("qux")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("qux/foo")), Visit::Nothing);
    }

    #[test]
    fn test_unionmatcher_concatenate_subdirs() {
        let m1 = PrefixMatcher::new([repo_path("common/bar"), repo_path("1/foo")]);
        let m2 = PrefixMatcher::new([repo_path("common/baz"), repo_path("2/qux")]);
        let m = UnionMatcher::new(&m1, &m2);

        assert!(!m.matches(repo_path("common")));
        assert!(!m.matches(repo_path("1")));
        assert!(!m.matches(repo_path("2")));
        assert!(m.matches(repo_path("common/bar")));
        assert!(m.matches(repo_path("common/bar/baz")));
        assert!(m.matches(repo_path("common/baz")));
        assert!(m.matches(repo_path("1/foo")));
        assert!(m.matches(repo_path("1/foo/qux")));
        assert!(m.matches(repo_path("2/qux")));
        assert!(!m.matches(repo_path("2/quux")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {
                    RepoPathComponentBuf::from("common"),
                    RepoPathComponentBuf::from("1"),
                    RepoPathComponentBuf::from("2"),
                },
                hashset! {},
            )
        );
        assert_eq!(
            m.visit(repo_path("common")),
            Visit::sets(
                hashset! {
                    RepoPathComponentBuf::from("bar"),
                    RepoPathComponentBuf::from("baz"),
                },
                hashset! {
                    RepoPathComponentBuf::from("bar"),
                    RepoPathComponentBuf::from("baz"),
                },
            )
        );
        assert_eq!(
            m.visit(repo_path("1")),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("foo")},
                hashset! {RepoPathComponentBuf::from("foo")},
            )
        );
        assert_eq!(
            m.visit(repo_path("2")),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("qux")},
                hashset! {RepoPathComponentBuf::from("qux")},
            )
        );
        assert_eq!(m.visit(repo_path("common/bar")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("1/foo")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("2/qux")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("2/quux")), Visit::Nothing);
    }

    #[test]
    fn test_differencematcher_remove_subdir() {
        let m1 = PrefixMatcher::new([repo_path("foo"), repo_path("bar")]);
        let m2 = PrefixMatcher::new([repo_path("foo/bar")]);
        let m = DifferenceMatcher::new(&m1, &m2);

        assert!(m.matches(repo_path("foo")));
        assert!(!m.matches(repo_path("foo/bar")));
        assert!(!m.matches(repo_path("foo/bar/baz")));
        assert!(m.matches(repo_path("foo/baz")));
        assert!(m.matches(repo_path("bar")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {
                    RepoPathComponentBuf::from("foo"),
                    RepoPathComponentBuf::from("bar"),
                },
                hashset! {
                    RepoPathComponentBuf::from("foo"),
                    RepoPathComponentBuf::from("bar"),
                },
            )
        );
        assert_eq!(
            m.visit(repo_path("foo")),
            Visit::Specific {
                dirs: VisitDirs::All,
                files: VisitFiles::All,
            }
        );
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("foo/baz")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("bar")), Visit::AllRecursively);
    }

    #[test]
    fn test_differencematcher_shared_patterns() {
        let m1 = PrefixMatcher::new([repo_path("foo"), repo_path("bar")]);
        let m2 = PrefixMatcher::new([repo_path("foo")]);
        let m = DifferenceMatcher::new(&m1, &m2);

        assert!(!m.matches(repo_path("foo")));
        assert!(!m.matches(repo_path("foo/bar")));
        assert!(m.matches(repo_path("bar")));
        assert!(m.matches(repo_path("bar/foo")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {
                    RepoPathComponentBuf::from("foo"),
                    RepoPathComponentBuf::from("bar"),
                },
                hashset! {
                    RepoPathComponentBuf::from("foo"),
                    RepoPathComponentBuf::from("bar"),
                },
            )
        );
        assert_eq!(m.visit(repo_path("foo")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("bar")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("bar/foo")), Visit::AllRecursively);
    }

    #[test]
    fn test_intersectionmatcher_intersecting_roots() {
        let m1 = PrefixMatcher::new([repo_path("foo"), repo_path("bar")]);
        let m2 = PrefixMatcher::new([repo_path("bar"), repo_path("baz")]);
        let m = IntersectionMatcher::new(&m1, &m2);

        assert!(!m.matches(repo_path("foo")));
        assert!(!m.matches(repo_path("foo/bar")));
        assert!(m.matches(repo_path("bar")));
        assert!(m.matches(repo_path("bar/foo")));
        assert!(!m.matches(repo_path("baz")));
        assert!(!m.matches(repo_path("baz/foo")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("bar")},
                hashset! {RepoPathComponentBuf::from("bar")}
            )
        );
        assert_eq!(m.visit(repo_path("foo")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("bar")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("bar/foo")), Visit::AllRecursively);
        assert_eq!(m.visit(repo_path("baz")), Visit::Nothing);
        assert_eq!(m.visit(repo_path("baz/foo")), Visit::Nothing);
    }

    #[test]
    fn test_intersectionmatcher_subdir() {
        let m1 = PrefixMatcher::new([repo_path("foo")]);
        let m2 = PrefixMatcher::new([repo_path("foo/bar")]);
        let m = IntersectionMatcher::new(&m1, &m2);

        assert!(!m.matches(repo_path("foo")));
        assert!(!m.matches(repo_path("bar")));
        assert!(m.matches(repo_path("foo/bar")));
        assert!(m.matches(repo_path("foo/bar/baz")));
        assert!(!m.matches(repo_path("foo/baz")));

        assert_eq!(
            m.visit(RepoPath::root()),
            Visit::sets(hashset! {RepoPathComponentBuf::from("foo")}, hashset! {})
        );
        assert_eq!(m.visit(repo_path("bar")), Visit::Nothing);
        assert_eq!(
            m.visit(repo_path("foo")),
            Visit::sets(
                hashset! {RepoPathComponentBuf::from("bar")},
                hashset! {RepoPathComponentBuf::from("bar")}
            )
        );
        assert_eq!(m.visit(repo_path("foo/bar")), Visit::AllRecursively);
    }
}
