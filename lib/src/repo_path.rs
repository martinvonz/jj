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
use std::fmt::{Debug, Error, Formatter};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};

use itertools::Itertools;
use ref_cast::{ref_cast_custom, RefCastCustom};
use thiserror::Error;

use crate::file_util;

content_hash! {
    /// Owned `RepoPath` component.
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
    pub struct RepoPathComponentBuf {
        // Don't add more fields. Eq, Hash, and Ord must be compatible with the
        // borrowed RepoPathComponent type.
        value: String,
    }
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath {
    components: Vec<RepoPathComponentBuf>,
}

impl Debug for RepoPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_fmt(format_args!("{:?}", &self.to_internal_file_string()))
    }
}

impl RepoPath {
    pub fn root() -> Self {
        RepoPath { components: vec![] }
    }

    /// Creates `RepoPath` from valid string representation.
    ///
    /// The input `value` must not contain empty path components. For example,
    /// `"/"`, `"/foo"`, `"foo/"`, `"foo//bar"` are all invalid.
    pub fn from_internal_string(value: &str) -> Self {
        assert!(is_valid_repo_path_str(value));
        if value.is_empty() {
            RepoPath::root()
        } else {
            let components = value
                .split('/')
                .map(|value| RepoPathComponentBuf {
                    value: value.to_string(),
                })
                .collect();
            RepoPath { components }
        }
    }

    /// Converts repo-relative `Path` to `RepoPath`.
    ///
    /// The input path should not contain `.` or `..`.
    pub fn from_relative_path(relative_path: impl AsRef<Path>) -> Option<Self> {
        let relative_path = relative_path.as_ref();
        let components = relative_path
            .components()
            .map(|c| match c {
                Component::Normal(a) => Some(RepoPathComponentBuf::from(a.to_str().unwrap())),
                // TODO: better to return Err instead of None?
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some(RepoPath::from_components(components))
    }

    pub fn from_components(components: Vec<RepoPathComponentBuf>) -> Self {
        RepoPath { components }
    }

    /// Parses an `input` path into a `RepoPath` relative to `base`.
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
        if repo_relative_path == Path::new(".") {
            return Ok(RepoPath::root());
        }
        Self::from_relative_path(repo_relative_path)
            .ok_or_else(|| FsPathParseError::InputNotInRepo(input.to_owned()))
    }

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator). This format includes a
    /// trailing slash, unless this path represents the root directory. That
    /// way it can be concatenated with a basename and produce a valid path.
    pub fn to_internal_dir_string(&self) -> String {
        let mut result = String::new();
        for component in &self.components {
            result.push_str(component.as_str());
            result.push('/');
        }
        result
    }

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn to_internal_file_string(&self) -> String {
        let strings = self
            .components
            .iter()
            .map(|component| component.value.clone())
            .collect_vec();
        strings.join("/")
    }

    pub fn to_fs_path(&self, base: &Path) -> PathBuf {
        let repo_path_len: usize = self.components.iter().map(|x| x.as_str().len() + 1).sum();
        let mut result = PathBuf::with_capacity(base.as_os_str().len() + repo_path_len);
        result.push(base);
        result.extend(self.components.iter().map(|dir| &dir.value));
        result
    }

    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    pub fn contains(&self, other: &RepoPath) -> bool {
        other.components.starts_with(&self.components)
    }

    pub fn parent(&self) -> Option<RepoPath> {
        if self.is_root() {
            None
        } else {
            Some(RepoPath {
                components: self.components[0..self.components.len() - 1].to_vec(),
            })
        }
    }

    pub fn split(&self) -> Option<(RepoPath, &RepoPathComponent)> {
        if self.is_root() {
            None
        } else {
            Some((self.parent().unwrap(), self.components.last().unwrap()))
        }
    }

    pub fn components(&self) -> &Vec<RepoPathComponentBuf> {
        &self.components
    }

    pub fn join(&self, entry: &RepoPathComponent) -> RepoPath {
        let components =
            itertools::chain(self.components.iter().cloned(), [entry.to_owned()]).collect();
        RepoPath { components }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum FsPathParseError {
    #[error(r#"Path "{}" is not in the repo"#, .0.display())]
    InputNotInRepo(PathBuf),
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

    use super::*;

    fn repo_path(value: &str) -> RepoPath {
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
        assert_eq!(repo_path(""), RepoPath::root());
        assert!(panic::catch_unwind(|| repo_path("/")).is_err());
        assert!(panic::catch_unwind(|| repo_path("/x")).is_err());
        assert!(panic::catch_unwind(|| repo_path("x/")).is_err());
        assert!(panic::catch_unwind(|| repo_path("x//y")).is_err());
    }

    #[test]
    fn test_to_internal_string() {
        assert_eq!(RepoPath::root().to_internal_file_string(), "");
        assert_eq!(repo_path("dir").to_internal_file_string(), "dir");
        assert_eq!(repo_path("dir/file").to_internal_file_string(), "dir/file");
    }

    #[test]
    fn test_order() {
        assert!(RepoPath::root() < repo_path("dir"));
        assert!(repo_path("dir") < repo_path("dirx"));
        // '#' < '/'
        assert!(repo_path("dir") < repo_path("dir#"));
        assert!(repo_path("dir") < repo_path("dir/sub"));

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
        assert_eq!(dir, repo_path("dir"));
        let subdir = dir.join(RepoPathComponent::new("subdir"));
        assert_eq!(subdir, repo_path("dir/subdir"));
        assert_eq!(
            subdir.join(RepoPathComponent::new("file")),
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
        assert_eq!(subdir.parent(), Some(dir));
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
        assert_eq!(file.split(), Some((dir, file_component)));
    }

    #[test]
    fn test_components() {
        assert_eq!(RepoPath::root().components(), &vec![]);
        assert_eq!(
            repo_path("dir").components(),
            &vec![RepoPathComponentBuf::from("dir")]
        );
        assert_eq!(
            repo_path("dir/subdir").components(),
            &vec![
                RepoPathComponentBuf::from("dir"),
                RepoPathComponentBuf::from("subdir")
            ]
        );
    }

    #[test]
    fn test_to_fs_path() {
        assert_eq!(
            repo_path("").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir")
        );
        assert_eq!(repo_path("").to_fs_path(Path::new("")), Path::new(""));
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
            RepoPath::parse_fs_path(&cwd_path, wc_path, ""),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, wc_path, "."),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, wc_path, "file"),
            Ok(repo_path("file"))
        );
        // Both slash and the platform's separator are allowed
        assert_eq!(
            RepoPath::parse_fs_path(
                &cwd_path,
                wc_path,
                format!("dir{}file", std::path::MAIN_SEPARATOR)
            ),
            Ok(repo_path("dir/file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, wc_path, "dir/file"),
            Ok(repo_path("dir/file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, wc_path, ".."),
            Err(FsPathParseError::InputNotInRepo("..".into()))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &cwd_path, "../repo"),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &cwd_path, "../repo/file"),
            Ok(repo_path("file"))
        );
        // Input may be absolute path with ".."
        assert_eq!(
            RepoPath::parse_fs_path(
                &cwd_path,
                &cwd_path,
                cwd_path.join("../repo").to_str().unwrap()
            ),
            Ok(RepoPath::root())
        );
    }

    #[test]
    fn parse_fs_path_wc_in_cwd_parent() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("dir");
        let wc_path = cwd_path.parent().unwrap().to_path_buf();

        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, ""),
            Ok(repo_path("dir"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "."),
            Ok(repo_path("dir"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "file"),
            Ok(repo_path("dir/file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "subdir/file"),
            Ok(repo_path("dir/subdir/file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, ".."),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "../.."),
            Err(FsPathParseError::InputNotInRepo("../..".into()))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "../other-dir/file"),
            Ok(repo_path("other-dir/file"))
        );
    }

    #[test]
    fn parse_fs_path_wc_in_cwd_child() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("cwd");
        let wc_path = cwd_path.join("repo");

        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, ""),
            Err(FsPathParseError::InputNotInRepo("".into()))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "not-repo"),
            Err(FsPathParseError::InputNotInRepo("not-repo".into()))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "repo"),
            Ok(RepoPath::root())
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "repo/file"),
            Ok(repo_path("file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "repo/dir/file"),
            Ok(repo_path("dir/file"))
        );
    }
}
