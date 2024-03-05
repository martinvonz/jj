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

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt::{Debug, Error, Formatter};
use std::iter::FusedIterator;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};

use ref_cast::{ref_cast_custom, RefCastCustom};
use thiserror::Error;

use crate::content_hash::ContentHash;
use crate::file_util;

/// Owned `RepoPath` component.
#[derive(ContentHash, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct RepoPathComponentBuf {
    // Don't add more fields. Eq, Hash, and Ord must be compatible with the
    // borrowed RepoPathComponent type.
    value: String,
}

/// Borrowed `RepoPath` component.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Hash, RefCastCustom)]
#[repr(transparent)]
pub struct RepoPathComponent {
    value: str,
}

impl RepoPathComponent {
    /// Wraps `value` as `RepoPathComponent`.
    ///
    /// The input `value` must not be empty and not contain path separator.
    pub fn new(value: &str) -> &Self {
        assert!(is_valid_repo_path_component_str(value));
        Self::new_unchecked(value)
    }

    #[ref_cast_custom]
    const fn new_unchecked(value: &str) -> &Self;

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl From<&str> for RepoPathComponentBuf {
    fn from(value: &str) -> Self {
        RepoPathComponentBuf::from(value.to_owned())
    }
}

impl From<String> for RepoPathComponentBuf {
    fn from(value: String) -> Self {
        assert!(is_valid_repo_path_component_str(&value));
        RepoPathComponentBuf { value }
    }
}

impl AsRef<RepoPathComponent> for RepoPathComponent {
    fn as_ref(&self) -> &RepoPathComponent {
        self
    }
}

impl AsRef<RepoPathComponent> for RepoPathComponentBuf {
    fn as_ref(&self) -> &RepoPathComponent {
        self
    }
}

impl Borrow<RepoPathComponent> for RepoPathComponentBuf {
    fn borrow(&self) -> &RepoPathComponent {
        self
    }
}

impl Deref for RepoPathComponentBuf {
    type Target = RepoPathComponent;

    fn deref(&self) -> &Self::Target {
        RepoPathComponent::new_unchecked(&self.value)
    }
}

impl ToOwned for RepoPathComponent {
    type Owned = RepoPathComponentBuf;

    fn to_owned(&self) -> Self::Owned {
        let value = self.value.to_owned();
        RepoPathComponentBuf { value }
    }

    fn clone_into(&self, target: &mut Self::Owned) {
        self.value.clone_into(&mut target.value);
    }
}

/// Iterator over `RepoPath` components.
#[derive(Clone, Debug)]
pub struct RepoPathComponentsIter<'a> {
    value: &'a str,
}

impl<'a> RepoPathComponentsIter<'a> {
    /// Returns the remaining part as repository path.
    pub fn as_path(&self) -> &'a RepoPath {
        RepoPath::from_internal_string_unchecked(self.value)
    }
}

impl<'a> Iterator for RepoPathComponentsIter<'a> {
    type Item = &'a RepoPathComponent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.value.is_empty() {
            return None;
        }
        let (name, remainder) = self
            .value
            .split_once('/')
            .unwrap_or_else(|| (self.value, &self.value[self.value.len()..]));
        self.value = remainder;
        Some(RepoPathComponent::new_unchecked(name))
    }
}

impl DoubleEndedIterator for RepoPathComponentsIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.value.is_empty() {
            return None;
        }
        let (remainder, name) = self
            .value
            .rsplit_once('/')
            .unwrap_or_else(|| (&self.value[..0], self.value));
        self.value = remainder;
        Some(RepoPathComponent::new_unchecked(name))
    }
}

impl FusedIterator for RepoPathComponentsIter<'_> {}

/// Owned repository path.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RepoPathBuf {
    // Don't add more fields. Eq, Hash, and Ord must be compatible with the
    // borrowed RepoPath type.
    value: String,
}

/// Borrowed repository path.
#[derive(Eq, Hash, PartialEq, RefCastCustom)]
#[repr(transparent)]
pub struct RepoPath {
    value: str,
}

impl Debug for RepoPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_fmt(format_args!("{:?}", &self.value))
    }
}

impl Debug for RepoPathBuf {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        <RepoPath as Debug>::fmt(self, f)
    }
}

impl RepoPathBuf {
    /// Creates owned repository path pointing to the root.
    pub const fn root() -> Self {
        RepoPathBuf {
            value: String::new(),
        }
    }

    /// Creates `RepoPathBuf` from valid string representation.
    ///
    /// The input `value` must not contain empty path components. For example,
    /// `"/"`, `"/foo"`, `"foo/"`, `"foo//bar"` are all invalid.
    pub fn from_internal_string(value: impl Into<String>) -> Self {
        let value = value.into();
        assert!(is_valid_repo_path_str(&value));
        RepoPathBuf { value }
    }

    /// Converts repo-relative `Path` to `RepoPathBuf`.
    ///
    /// The input path should not contain redundant `.` or `..`.
    pub fn from_relative_path(
        relative_path: impl AsRef<Path>,
    ) -> Result<Self, RelativePathParseError> {
        let relative_path = relative_path.as_ref();
        if relative_path == Path::new(".") {
            return Ok(Self::root());
        }

        let mut components = relative_path
            .components()
            .map(|c| match c {
                Component::Normal(name) => {
                    name.to_str()
                        .ok_or_else(|| RelativePathParseError::InvalidUtf8 {
                            path: relative_path.into(),
                        })
                }
                _ => Err(RelativePathParseError::InvalidComponent {
                    component: c.as_os_str().to_string_lossy().into(),
                    path: relative_path.into(),
                }),
            })
            .fuse();
        let mut value = String::with_capacity(relative_path.as_os_str().len());
        if let Some(name) = components.next() {
            value.push_str(name?);
        }
        for name in components {
            value.push('/');
            value.push_str(name?);
        }
        Ok(RepoPathBuf { value })
    }

    /// Parses an `input` path into a `RepoPathBuf` relative to `base`.
    ///
    /// The `cwd` and `base` paths are supposed to be absolute and normalized in
    /// the same manner. The `input` path may be either relative to `cwd` or
    /// absolute.
    pub fn parse_fs_path(
        cwd: &Path,
        base: &Path,
        input: impl AsRef<Path>,
    ) -> Result<Self, FsPathParseError> {
        let input = input.as_ref();
        let abs_input_path = file_util::normalize_path(&cwd.join(input));
        let repo_relative_path = file_util::relative_path(base, &abs_input_path);
        Self::from_relative_path(repo_relative_path).map_err(|source| FsPathParseError {
            base: file_util::relative_path(cwd, base).into(),
            input: input.into(),
            source,
        })
    }

    /// Consumes this and returns the underlying string representation.
    pub fn into_internal_string(self) -> String {
        self.value
    }
}

impl RepoPath {
    /// Returns repository path pointing to the root.
    pub const fn root() -> &'static Self {
        Self::from_internal_string_unchecked("")
    }

    /// Wraps valid string representation as `RepoPath`.
    ///
    /// The input `value` must not contain empty path components. For example,
    /// `"/"`, `"/foo"`, `"foo/"`, `"foo//bar"` are all invalid.
    pub fn from_internal_string(value: &str) -> &Self {
        assert!(is_valid_repo_path_str(value));
        Self::from_internal_string_unchecked(value)
    }

    #[ref_cast_custom]
    const fn from_internal_string_unchecked(value: &str) -> &Self;

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator). This format includes a
    /// trailing slash, unless this path represents the root directory. That
    /// way it can be concatenated with a basename and produce a valid path.
    pub fn to_internal_dir_string(&self) -> String {
        if self.value.is_empty() {
            String::new()
        } else {
            [&self.value, "/"].concat()
        }
    }

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn as_internal_file_string(&self) -> &str {
        &self.value
    }

    pub fn to_fs_path(&self, base: &Path) -> PathBuf {
        let mut result = PathBuf::with_capacity(base.as_os_str().len() + self.value.len() + 1);
        result.push(base);
        result.extend(self.components().map(RepoPathComponent::as_str));
        if result.as_os_str().is_empty() {
            result.push(".");
        }
        result
    }

    pub fn is_root(&self) -> bool {
        self.value.is_empty()
    }

    /// Returns true if the `base` is a prefix of this path.
    pub fn starts_with(&self, base: &RepoPath) -> bool {
        self.strip_prefix(base).is_some()
    }

    /// Returns the remaining path with the `base` path removed.
    pub fn strip_prefix(&self, base: &RepoPath) -> Option<&RepoPath> {
        if base.value.is_empty() {
            Some(self)
        } else {
            let tail = self.value.strip_prefix(&base.value)?;
            if tail.is_empty() {
                Some(RepoPath::from_internal_string_unchecked(tail))
            } else {
                tail.strip_prefix('/')
                    .map(RepoPath::from_internal_string_unchecked)
            }
        }
    }

    /// Returns the parent path without the base name component.
    pub fn parent(&self) -> Option<&RepoPath> {
        self.split().map(|(parent, _)| parent)
    }

    /// Splits this into the parent path and base name component.
    pub fn split(&self) -> Option<(&RepoPath, &RepoPathComponent)> {
        let mut components = self.components();
        let basename = components.next_back()?;
        Some((components.as_path(), basename))
    }

    pub fn components(&self) -> RepoPathComponentsIter<'_> {
        RepoPathComponentsIter { value: &self.value }
    }

    pub fn join(&self, entry: &RepoPathComponent) -> RepoPathBuf {
        let value = if self.value.is_empty() {
            entry.as_str().to_owned()
        } else {
            [&self.value, "/", entry.as_str()].concat()
        };
        RepoPathBuf { value }
    }
}

impl AsRef<RepoPath> for RepoPath {
    fn as_ref(&self) -> &RepoPath {
        self
    }
}

impl AsRef<RepoPath> for RepoPathBuf {
    fn as_ref(&self) -> &RepoPath {
        self
    }
}

impl Borrow<RepoPath> for RepoPathBuf {
    fn borrow(&self) -> &RepoPath {
        self
    }
}

impl Deref for RepoPathBuf {
    type Target = RepoPath;

    fn deref(&self) -> &Self::Target {
        RepoPath::from_internal_string_unchecked(&self.value)
    }
}

impl ToOwned for RepoPath {
    type Owned = RepoPathBuf;

    fn to_owned(&self) -> Self::Owned {
        let value = self.value.to_owned();
        RepoPathBuf { value }
    }

    fn clone_into(&self, target: &mut Self::Owned) {
        self.value.clone_into(&mut target.value);
    }
}

impl Ord for RepoPath {
    fn cmp(&self, other: &Self) -> Ordering {
        // If there were leading/trailing slash, components-based Ord would
        // disagree with str-based Eq.
        debug_assert!(is_valid_repo_path_str(&self.value));
        self.components().cmp(other.components())
    }
}

impl Ord for RepoPathBuf {
    fn cmp(&self, other: &Self) -> Ordering {
        <RepoPath as Ord>::cmp(self, other)
    }
}

impl PartialOrd for RepoPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialOrd for RepoPathBuf {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RelativePathParseError {
    #[error(r#"Invalid component "{component}" in repo-relative path "{path}""#)]
    InvalidComponent {
        component: Box<str>,
        path: Box<Path>,
    },
    #[error(r#"Not valid UTF-8 path "{path}""#)]
    InvalidUtf8 { path: Box<Path> },
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error(r#"Path "{input}" is not in the repo "{base}""#)]
pub struct FsPathParseError {
    base: Box<Path>,
    input: Box<Path>,
    source: RelativePathParseError,
}

fn is_valid_repo_path_component_str(value: &str) -> bool {
    !value.is_empty() && !value.contains('/')
}

fn is_valid_repo_path_str(value: &str) -> bool {
    !value.starts_with('/') && !value.ends_with('/') && !value.contains("//")
}

#[cfg(test)]
mod tests {
    use std::panic;

    use assert_matches::assert_matches;
    use itertools::Itertools as _;

    use super::*;

    fn repo_path(value: &str) -> &RepoPath {
        RepoPath::from_internal_string(value)
    }

    #[test]
    fn test_is_root() {
        assert!(RepoPath::root().is_root());
        assert!(repo_path("").is_root());
        assert!(!repo_path("foo").is_root());
    }

    #[test]
    fn test_from_internal_string() {
        let repo_path_buf = |value: &str| RepoPathBuf::from_internal_string(value);
        assert_eq!(repo_path_buf(""), RepoPathBuf::root());
        assert!(panic::catch_unwind(|| repo_path_buf("/")).is_err());
        assert!(panic::catch_unwind(|| repo_path_buf("/x")).is_err());
        assert!(panic::catch_unwind(|| repo_path_buf("x/")).is_err());
        assert!(panic::catch_unwind(|| repo_path_buf("x//y")).is_err());

        assert_eq!(repo_path(""), RepoPath::root());
        assert!(panic::catch_unwind(|| repo_path("/")).is_err());
        assert!(panic::catch_unwind(|| repo_path("/x")).is_err());
        assert!(panic::catch_unwind(|| repo_path("x/")).is_err());
        assert!(panic::catch_unwind(|| repo_path("x//y")).is_err());
    }

    #[test]
    fn test_as_internal_file_string() {
        assert_eq!(RepoPath::root().as_internal_file_string(), "");
        assert_eq!(repo_path("dir").as_internal_file_string(), "dir");
        assert_eq!(repo_path("dir/file").as_internal_file_string(), "dir/file");
    }

    #[test]
    fn test_to_internal_dir_string() {
        assert_eq!(RepoPath::root().to_internal_dir_string(), "");
        assert_eq!(repo_path("dir").to_internal_dir_string(), "dir/");
        assert_eq!(repo_path("dir/file").to_internal_dir_string(), "dir/file/");
    }

    #[test]
    fn test_starts_with() {
        assert!(repo_path("").starts_with(repo_path("")));
        assert!(repo_path("x").starts_with(repo_path("")));
        assert!(!repo_path("").starts_with(repo_path("x")));

        assert!(repo_path("x").starts_with(repo_path("x")));
        assert!(repo_path("x/y").starts_with(repo_path("x")));
        assert!(!repo_path("xy").starts_with(repo_path("x")));
        assert!(!repo_path("x/y").starts_with(repo_path("y")));

        assert!(repo_path("x/y").starts_with(repo_path("x/y")));
        assert!(repo_path("x/y/z").starts_with(repo_path("x/y")));
        assert!(!repo_path("x/yz").starts_with(repo_path("x/y")));
        assert!(!repo_path("x").starts_with(repo_path("x/y")));
        assert!(!repo_path("xy").starts_with(repo_path("x/y")));
    }

    #[test]
    fn test_strip_prefix() {
        assert_eq!(
            repo_path("").strip_prefix(repo_path("")),
            Some(repo_path(""))
        );
        assert_eq!(
            repo_path("x").strip_prefix(repo_path("")),
            Some(repo_path("x"))
        );
        assert_eq!(repo_path("").strip_prefix(repo_path("x")), None);

        assert_eq!(
            repo_path("x").strip_prefix(repo_path("x")),
            Some(repo_path(""))
        );
        assert_eq!(
            repo_path("x/y").strip_prefix(repo_path("x")),
            Some(repo_path("y"))
        );
        assert_eq!(repo_path("xy").strip_prefix(repo_path("x")), None);
        assert_eq!(repo_path("x/y").strip_prefix(repo_path("y")), None);

        assert_eq!(
            repo_path("x/y").strip_prefix(repo_path("x/y")),
            Some(repo_path(""))
        );
        assert_eq!(
            repo_path("x/y/z").strip_prefix(repo_path("x/y")),
            Some(repo_path("z"))
        );
        assert_eq!(repo_path("x/yz").strip_prefix(repo_path("x/y")), None);
        assert_eq!(repo_path("x").strip_prefix(repo_path("x/y")), None);
        assert_eq!(repo_path("xy").strip_prefix(repo_path("x/y")), None);
    }

    #[test]
    fn test_order() {
        assert!(RepoPath::root() < repo_path("dir"));
        assert!(repo_path("dir") < repo_path("dirx"));
        // '#' < '/', but ["dir", "sub"] < ["dir#"]
        assert!(repo_path("dir") < repo_path("dir#"));
        assert!(repo_path("dir") < repo_path("dir/sub"));
        assert!(repo_path("dir/sub") < repo_path("dir#"));

        assert!(repo_path("abc") < repo_path("dir/file"));
        assert!(repo_path("dir") < repo_path("dir/file"));
        assert!(repo_path("dis") > repo_path("dir/file"));
        assert!(repo_path("xyz") > repo_path("dir/file"));
        assert!(repo_path("dir1/xyz") < repo_path("dir2/abc"));
    }

    #[test]
    fn test_join() {
        let root = RepoPath::root();
        let dir = root.join(RepoPathComponent::new("dir"));
        assert_eq!(dir.as_ref(), repo_path("dir"));
        let subdir = dir.join(RepoPathComponent::new("subdir"));
        assert_eq!(subdir.as_ref(), repo_path("dir/subdir"));
        assert_eq!(
            subdir.join(RepoPathComponent::new("file")).as_ref(),
            repo_path("dir/subdir/file")
        );
    }

    #[test]
    fn test_parent() {
        let root = RepoPath::root();
        let dir_component = RepoPathComponent::new("dir");
        let subdir_component = RepoPathComponent::new("subdir");

        let dir = root.join(dir_component);
        let subdir = dir.join(subdir_component);

        assert_eq!(root.parent(), None);
        assert_eq!(dir.parent(), Some(root));
        assert_eq!(subdir.parent(), Some(dir.as_ref()));
    }

    #[test]
    fn test_split() {
        let root = RepoPath::root();
        let dir_component = RepoPathComponent::new("dir");
        let file_component = RepoPathComponent::new("file");

        let dir = root.join(dir_component);
        let file = dir.join(file_component);

        assert_eq!(root.split(), None);
        assert_eq!(dir.split(), Some((root, dir_component)));
        assert_eq!(file.split(), Some((dir.as_ref(), file_component)));
    }

    #[test]
    fn test_components() {
        assert!(RepoPath::root().components().next().is_none());
        assert_eq!(
            repo_path("dir").components().collect_vec(),
            vec![RepoPathComponent::new("dir")]
        );
        assert_eq!(
            repo_path("dir/subdir").components().collect_vec(),
            vec![
                RepoPathComponent::new("dir"),
                RepoPathComponent::new("subdir"),
            ]
        );

        // Iterates from back
        assert!(RepoPath::root().components().next_back().is_none());
        assert_eq!(
            repo_path("dir").components().rev().collect_vec(),
            vec![RepoPathComponent::new("dir")]
        );
        assert_eq!(
            repo_path("dir/subdir").components().rev().collect_vec(),
            vec![
                RepoPathComponent::new("subdir"),
                RepoPathComponent::new("dir"),
            ]
        );
    }

    #[test]
    fn test_to_fs_path() {
        assert_eq!(
            repo_path("").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir")
        );
        assert_eq!(repo_path("").to_fs_path(Path::new("")), Path::new("."));
        assert_eq!(
            repo_path("file").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir/file")
        );
        assert_eq!(
            repo_path("some/deep/dir/file").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir/some/deep/dir/file")
        );
        assert_eq!(
            repo_path("dir/file").to_fs_path(Path::new("")),
            Path::new("dir/file")
        );
    }

    #[test]
    fn parse_fs_path_wc_in_cwd() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("repo");
        let wc_path = &cwd_path;

        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, wc_path, "").as_deref(),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, wc_path, ".").as_deref(),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, wc_path, "file").as_deref(),
            Ok(repo_path("file"))
        );
        // Both slash and the platform's separator are allowed
        assert_eq!(
            RepoPathBuf::parse_fs_path(
                &cwd_path,
                wc_path,
                format!("dir{}file", std::path::MAIN_SEPARATOR)
            )
            .as_deref(),
            Ok(repo_path("dir/file"))
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, wc_path, "dir/file").as_deref(),
            Ok(repo_path("dir/file"))
        );
        assert_matches!(
            RepoPathBuf::parse_fs_path(&cwd_path, wc_path, ".."),
            Err(FsPathParseError {
                source: RelativePathParseError::InvalidComponent { .. },
                ..
            })
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &cwd_path, "../repo").as_deref(),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &cwd_path, "../repo/file").as_deref(),
            Ok(repo_path("file"))
        );
        // Input may be absolute path with ".."
        assert_eq!(
            RepoPathBuf::parse_fs_path(
                &cwd_path,
                &cwd_path,
                cwd_path.join("../repo").to_str().unwrap()
            )
            .as_deref(),
            Ok(RepoPath::root())
        );
    }

    #[test]
    fn parse_fs_path_wc_in_cwd_parent() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("dir");
        let wc_path = cwd_path.parent().unwrap().to_path_buf();

        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "").as_deref(),
            Ok(repo_path("dir"))
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, ".").as_deref(),
            Ok(repo_path("dir"))
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "file").as_deref(),
            Ok(repo_path("dir/file"))
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "subdir/file").as_deref(),
            Ok(repo_path("dir/subdir/file"))
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "..").as_deref(),
            Ok(RepoPath::root())
        );
        assert_matches!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "../.."),
            Err(FsPathParseError {
                source: RelativePathParseError::InvalidComponent { .. },
                ..
            })
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "../other-dir/file").as_deref(),
            Ok(repo_path("other-dir/file"))
        );
    }

    #[test]
    fn parse_fs_path_wc_in_cwd_child() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("cwd");
        let wc_path = cwd_path.join("repo");

        assert_matches!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, ""),
            Err(FsPathParseError {
                source: RelativePathParseError::InvalidComponent { .. },
                ..
            })
        );
        assert_matches!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "not-repo"),
            Err(FsPathParseError {
                source: RelativePathParseError::InvalidComponent { .. },
                ..
            })
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "repo").as_deref(),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "repo/file").as_deref(),
            Ok(repo_path("file"))
        );
        assert_eq!(
            RepoPathBuf::parse_fs_path(&cwd_path, &wc_path, "repo/dir/file").as_deref(),
            Ok(repo_path("dir/file"))
        );
    }
}
