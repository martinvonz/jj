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

#![allow(dead_code, missing_docs)]

use std::collections::{HashMap, HashSet};
use std::iter;

use crate::repo_path::{RepoPath, RepoPathComponent};

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
    fn sets(dirs: HashSet<RepoPathComponent>, files: HashSet<RepoPathComponent>) -> Self {
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
    Set(HashSet<RepoPathComponent>),
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitFiles {
    All,
    Set(HashSet<RepoPathComponent>),
}

pub trait Matcher {
    fn matches(&self, file: &RepoPath) -> bool;
    fn visit(&self, dir: &RepoPath) -> Visit;
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
    tree: RepoPathTree,
}

impl FilesMatcher {
    pub fn new(files: &[RepoPath]) -> Self {
        let mut tree = RepoPathTree::new();
        for f in files {
            tree.add_file(f);
        }
        FilesMatcher { tree }
    }
}

impl Matcher for FilesMatcher {
    fn matches(&self, file: &RepoPath) -> bool {
        self.tree.get(file).map(|sub| sub.is_file).unwrap_or(false)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        self.tree.get_visit_sets(dir)
    }
}

pub struct PrefixMatcher {
    tree: RepoPathTree,
}

impl PrefixMatcher {
    pub fn new(prefixes: &[RepoPath]) -> Self {
        let mut tree = RepoPathTree::new();
        for prefix in prefixes {
            let sub = tree.add(prefix);
            sub.is_dir = true;
            sub.is_file = true;
        }
        PrefixMatcher { tree }
    }
}

impl Matcher for PrefixMatcher {
    fn matches(&self, file: &RepoPath) -> bool {
        self.tree.walk_to(file).any(|(sub, _)| sub.is_file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        for (sub, tail_components) in self.tree.walk_to(dir) {
            // 'is_file' means the current path matches prefix paths
            if sub.is_file {
                return Visit::AllRecursively;
            }
            // 'dir' found, and is an ancestor of prefix paths
            if tail_components.is_empty() {
                return sub.to_visit_sets();
            }
        }
        Visit::Nothing
    }
}

/// Matches paths that are matched by the first input matcher but not by the
/// second.
pub struct DifferenceMatcher<'input> {
    /// The minuend
    wanted: &'input dyn Matcher,
    /// The subtrahend
    unwanted: &'input dyn Matcher,
}

impl<'input> DifferenceMatcher<'input> {
    pub fn new(wanted: &'input dyn Matcher, unwanted: &'input dyn Matcher) -> Self {
        Self { wanted, unwanted }
    }
}

impl Matcher for DifferenceMatcher<'_> {
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
pub struct IntersectionMatcher<'input> {
    input1: &'input dyn Matcher,
    input2: &'input dyn Matcher,
}

impl<'input> IntersectionMatcher<'input> {
    pub fn new(input1: &'input dyn Matcher, input2: &'input dyn Matcher) -> Self {
        Self { input1, input2 }
    }
}

impl Matcher for IntersectionMatcher<'_> {
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

/// Keeps track of which subdirectories and files of each directory need to be
/// visited.
#[derive(PartialEq, Eq, Debug)]
struct RepoPathTree {
    entries: HashMap<RepoPathComponent, RepoPathTree>,
    // is_dir/is_file aren't exclusive, both can be set to true. If entries is not empty,
    // is_dir should be set.
    is_dir: bool,
    is_file: bool,
}

impl RepoPathTree {
    fn new() -> Self {
        RepoPathTree {
            entries: HashMap::new(),
            is_dir: false,
            is_file: false,
        }
    }

    fn add(&mut self, dir: &RepoPath) -> &mut RepoPathTree {
        dir.components().iter().fold(self, |sub, name| {
            // Avoid name.clone() if entry already exists.
            if !sub.entries.contains_key(name) {
                sub.is_dir = true;
                sub.entries.insert(name.clone(), RepoPathTree::new());
            }
            sub.entries.get_mut(name).unwrap()
        })
    }

    fn add_dir(&mut self, dir: &RepoPath) {
        self.add(dir).is_dir = true;
    }

    fn add_file(&mut self, file: &RepoPath) {
        self.add(file).is_file = true;
    }

    fn get(&self, dir: &RepoPath) -> Option<&RepoPathTree> {
        dir.components()
            .iter()
            .try_fold(self, |sub, name| sub.entries.get(name))
    }

    fn get_visit_sets(&self, dir: &RepoPath) -> Visit {
        self.get(dir)
            .map(RepoPathTree::to_visit_sets)
            .unwrap_or(Visit::Nothing)
    }

    fn walk_to<'a>(
        &'a self,
        dir: &'a RepoPath,
    ) -> impl Iterator<Item = (&RepoPathTree, &[RepoPathComponent])> + 'a {
        iter::successors(
            Some((self, dir.components().as_slice())),
            |(sub, components)| {
                let (name, tail) = components.split_first()?;
                Some((sub.entries.get(name)?, tail))
            },
        )
    }

    fn to_visit_sets(&self) -> Visit {
        let mut dirs = HashSet::new();
        let mut files = HashSet::new();
        for (name, sub) in &self.entries {
            if sub.is_dir {
                dirs.insert(name.clone());
            }
            if sub.is_file {
                files.insert(name.clone());
            }
        }
        Visit::sets(dirs, files)
    }
}

#[cfg(test)]
mod tests {
    use maplit::hashset;

    use super::*;
    use crate::repo_path::{RepoPath, RepoPathComponent};

    #[test]
    fn test_repo_path_tree_empty() {
        let tree = RepoPathTree::new();
        assert_eq!(tree.get_visit_sets(&RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_repo_path_tree_root() {
        let mut tree = RepoPathTree::new();
        tree.add_dir(&RepoPath::root());
        assert_eq!(tree.get_visit_sets(&RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_repo_path_tree_dir() {
        let mut tree = RepoPathTree::new();
        tree.add_dir(&RepoPath::from_internal_string("dir"));
        assert_eq!(
            tree.get_visit_sets(&RepoPath::root()),
            Visit::sets(hashset! {RepoPathComponent::from("dir")}, hashset! {}),
        );
        tree.add_dir(&RepoPath::from_internal_string("dir/sub"));
        assert_eq!(
            tree.get_visit_sets(&RepoPath::from_internal_string("dir")),
            Visit::sets(hashset! {RepoPathComponent::from("sub")}, hashset! {}),
        );
    }

    #[test]
    fn test_repo_path_tree_file() {
        let mut tree = RepoPathTree::new();
        tree.add_file(&RepoPath::from_internal_string("dir/file"));
        assert_eq!(
            tree.get_visit_sets(&RepoPath::root()),
            Visit::sets(hashset! {RepoPathComponent::from("dir")}, hashset! {}),
        );
        assert_eq!(
            tree.get_visit_sets(&RepoPath::from_internal_string("dir")),
            Visit::sets(hashset! {}, hashset! {RepoPathComponent::from("file")}),
        );
    }

    #[test]
    fn test_nothingmatcher() {
        let m = NothingMatcher;
        assert!(!m.matches(&RepoPath::from_internal_string("file")));
        assert!(!m.matches(&RepoPath::from_internal_string("dir/file")));
        assert_eq!(m.visit(&RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_filesmatcher_empty() {
        let m = FilesMatcher::new(&[]);
        assert!(!m.matches(&RepoPath::from_internal_string("file")));
        assert!(!m.matches(&RepoPath::from_internal_string("dir/file")));
        assert_eq!(m.visit(&RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_filesmatcher_nonempty() {
        let m = FilesMatcher::new(&[
            RepoPath::from_internal_string("dir1/subdir1/file1"),
            RepoPath::from_internal_string("dir1/subdir1/file2"),
            RepoPath::from_internal_string("dir1/subdir2/file3"),
            RepoPath::from_internal_string("file4"),
        ]);

        assert!(!m.matches(&RepoPath::from_internal_string("dir1")));
        assert!(!m.matches(&RepoPath::from_internal_string("dir1/subdir1")));
        assert!(m.matches(&RepoPath::from_internal_string("dir1/subdir1/file1")));
        assert!(m.matches(&RepoPath::from_internal_string("dir1/subdir1/file2")));
        assert!(!m.matches(&RepoPath::from_internal_string("dir1/subdir1/file3")));

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponent::from("dir1")},
                hashset! {RepoPathComponent::from("file4")}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("dir1")),
            Visit::sets(
                hashset! {RepoPathComponent::from("subdir1"), RepoPathComponent::from("subdir2")},
                hashset! {}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("dir1/subdir1")),
            Visit::sets(
                hashset! {},
                hashset! {RepoPathComponent::from("file1"), RepoPathComponent::from("file2")}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("dir1/subdir2")),
            Visit::sets(hashset! {}, hashset! {RepoPathComponent::from("file3")})
        );
    }

    #[test]
    fn test_prefixmatcher_empty() {
        let m = PrefixMatcher::new(&[]);
        assert!(!m.matches(&RepoPath::from_internal_string("file")));
        assert!(!m.matches(&RepoPath::from_internal_string("dir/file")));
        assert_eq!(m.visit(&RepoPath::root()), Visit::Nothing);
    }

    #[test]
    fn test_prefixmatcher_root() {
        let m = PrefixMatcher::new(&[RepoPath::root()]);
        // Matches all files
        assert!(m.matches(&RepoPath::from_internal_string("file")));
        assert!(m.matches(&RepoPath::from_internal_string("dir/file")));
        // Visits all directories
        assert_eq!(m.visit(&RepoPath::root()), Visit::AllRecursively);
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar")),
            Visit::AllRecursively
        );
    }

    #[test]
    fn test_prefixmatcher_single_prefix() {
        let m = PrefixMatcher::new(&[RepoPath::from_internal_string("foo/bar")]);

        // Parts of the prefix should not match
        assert!(!m.matches(&RepoPath::from_internal_string("foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("bar")));
        // A file matching the prefix exactly should match
        assert!(m.matches(&RepoPath::from_internal_string("foo/bar")));
        // Files in subdirectories should match
        assert!(m.matches(&RepoPath::from_internal_string("foo/bar/baz")));
        assert!(m.matches(&RepoPath::from_internal_string("foo/bar/baz/qux")));
        // Sibling files should not match
        assert!(!m.matches(&RepoPath::from_internal_string("foo/foo")));
        // An unrooted "foo/bar" should not match
        assert!(!m.matches(&RepoPath::from_internal_string("bar/foo/bar")));

        // The matcher should only visit directory foo/ in the root (file "foo"
        // shouldn't be visited)
        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(hashset! {RepoPathComponent::from("foo")}, hashset! {})
        );
        // Inside parent directory "foo/", both subdirectory "bar" and file "bar" may
        // match
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo")),
            Visit::sets(
                hashset! {RepoPathComponent::from("bar")},
                hashset! {RepoPathComponent::from("bar")}
            )
        );
        // Inside a directory that matches the prefix, everything matches recursively
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar")),
            Visit::AllRecursively
        );
        // Same thing in subdirectories of the prefix
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar/baz")),
            Visit::AllRecursively
        );
        // Nothing in directories that are siblings of the prefix can match, so don't
        // visit
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar")),
            Visit::Nothing
        );
    }

    #[test]
    fn test_prefixmatcher_nested_prefixes() {
        let m = PrefixMatcher::new(&[
            RepoPath::from_internal_string("foo"),
            RepoPath::from_internal_string("foo/bar/baz"),
        ]);

        assert!(m.matches(&RepoPath::from_internal_string("foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("bar")));
        assert!(m.matches(&RepoPath::from_internal_string("foo/bar")));
        // Matches because the "foo" pattern matches
        assert!(m.matches(&RepoPath::from_internal_string("foo/baz/foo")));

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponent::from("foo")},
                hashset! {RepoPathComponent::from("foo")}
            )
        );
        // Inside a directory that matches the prefix, everything matches recursively
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo")),
            Visit::AllRecursively
        );
        // Same thing in subdirectories of the prefix
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar/baz")),
            Visit::AllRecursively
        );
    }

    #[test]
    fn test_differencematcher_remove_subdir() {
        let m1 = PrefixMatcher::new(&[
            RepoPath::from_internal_string("foo"),
            RepoPath::from_internal_string("bar"),
        ]);
        let m2 = PrefixMatcher::new(&[RepoPath::from_internal_string("foo/bar")]);
        let m = DifferenceMatcher::new(&m1, &m2);

        assert!(m.matches(&RepoPath::from_internal_string("foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("foo/bar")));
        assert!(!m.matches(&RepoPath::from_internal_string("foo/bar/baz")));
        assert!(m.matches(&RepoPath::from_internal_string("foo/baz")));
        assert!(m.matches(&RepoPath::from_internal_string("bar")));

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponent::from("foo"), RepoPathComponent::from("bar")},
                hashset! {RepoPathComponent::from("foo"), RepoPathComponent::from("bar")}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo")),
            Visit::Specific {
                dirs: VisitDirs::All,
                files: VisitFiles::All,
            }
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/baz")),
            Visit::AllRecursively
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar")),
            Visit::AllRecursively
        );
    }

    #[test]
    fn test_differencematcher_shared_patterns() {
        let m1 = PrefixMatcher::new(&[
            RepoPath::from_internal_string("foo"),
            RepoPath::from_internal_string("bar"),
        ]);
        let m2 = PrefixMatcher::new(&[RepoPath::from_internal_string("foo")]);
        let m = DifferenceMatcher::new(&m1, &m2);

        assert!(!m.matches(&RepoPath::from_internal_string("foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("foo/bar")));
        assert!(m.matches(&RepoPath::from_internal_string("bar")));
        assert!(m.matches(&RepoPath::from_internal_string("bar/foo")));

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponent::from("foo"), RepoPathComponent::from("bar")},
                hashset! {RepoPathComponent::from("foo"), RepoPathComponent::from("bar")}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar")),
            Visit::AllRecursively
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar/foo")),
            Visit::AllRecursively
        );
    }

    #[test]
    fn test_intersectionmatcher_intersecting_roots() {
        let m1 = PrefixMatcher::new(&[
            RepoPath::from_internal_string("foo"),
            RepoPath::from_internal_string("bar"),
        ]);
        let m2 = PrefixMatcher::new(&[
            RepoPath::from_internal_string("bar"),
            RepoPath::from_internal_string("baz"),
        ]);
        let m = IntersectionMatcher::new(&m1, &m2);

        assert!(!m.matches(&RepoPath::from_internal_string("foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("foo/bar")));
        assert!(m.matches(&RepoPath::from_internal_string("bar")));
        assert!(m.matches(&RepoPath::from_internal_string("bar/foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("baz")));
        assert!(!m.matches(&RepoPath::from_internal_string("baz/foo")));

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(
                hashset! {RepoPathComponent::from("bar")},
                hashset! {RepoPathComponent::from("bar")}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar")),
            Visit::AllRecursively
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar/foo")),
            Visit::AllRecursively
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("baz")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("baz/foo")),
            Visit::Nothing
        );
    }

    #[test]
    fn test_intersectionmatcher_subdir() {
        let m1 = PrefixMatcher::new(&[RepoPath::from_internal_string("foo")]);
        let m2 = PrefixMatcher::new(&[RepoPath::from_internal_string("foo/bar")]);
        let m = IntersectionMatcher::new(&m1, &m2);

        assert!(!m.matches(&RepoPath::from_internal_string("foo")));
        assert!(!m.matches(&RepoPath::from_internal_string("bar")));
        assert!(m.matches(&RepoPath::from_internal_string("foo/bar")));
        assert!(m.matches(&RepoPath::from_internal_string("foo/bar/baz")));
        assert!(!m.matches(&RepoPath::from_internal_string("foo/baz")));

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit::sets(hashset! {RepoPathComponent::from("foo")}, hashset! {})
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("bar")),
            Visit::Nothing
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo")),
            Visit::sets(
                hashset! {RepoPathComponent::from("bar")},
                hashset! {RepoPathComponent::from("bar")}
            )
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("foo/bar")),
            Visit::AllRecursively
        );
    }
}
