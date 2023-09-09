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

use std::fmt::{Debug, Error, Formatter};
use std::path::{Component, Path, PathBuf};

use itertools::Itertools;
use thiserror::Error;

use crate::file_util;

content_hash! {
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
    pub struct RepoPathComponent {
        value: String,
    }
}

impl RepoPathComponent {
    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn string(&self) -> String {
        self.value.to_string()
    }
}

impl From<&str> for RepoPathComponent {
    fn from(value: &str) -> Self {
        RepoPathComponent::from(value.to_owned())
    }
}

impl From<String> for RepoPathComponent {
    fn from(value: String) -> Self {
        assert!(!value.contains('/'));
        RepoPathComponent { value }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath {
    components: Vec<RepoPathComponent>,
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

    pub fn from_internal_string(value: &str) -> Self {
        assert!(!value.ends_with('/'));
        if value.is_empty() {
            RepoPath::root()
        } else {
            let components = value
                .split('/')
                .map(|value| RepoPathComponent {
                    value: value.to_string(),
                })
                .collect();
            RepoPath { components }
        }
    }

    pub fn from_components(components: Vec<RepoPathComponent>) -> Self {
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
        let components = repo_relative_path
            .components()
            .map(|c| match c {
                Component::Normal(a) => Ok(RepoPathComponent::from(a.to_str().unwrap())),
                _ => Err(FsPathParseError::InputNotInRepo(input.to_owned())),
            })
            .try_collect()?;
        Ok(RepoPath::from_components(components))
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

    pub fn components(&self) -> &Vec<RepoPathComponent> {
        &self.components
    }
}

pub trait RepoPathJoin<T> {
    type Result;

    fn join(&self, entry: &T) -> Self::Result;
}

impl RepoPathJoin<RepoPathComponent> for RepoPath {
    type Result = RepoPath;

    fn join(&self, entry: &RepoPathComponent) -> RepoPath {
        let components = self.components.iter().chain([entry]).cloned().collect();
        RepoPath { components }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum FsPathParseError {
    #[error(r#"Path "{}" is not in the repo"#, .0.display())]
    InputNotInRepo(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_root() {
        assert!(RepoPath::root().is_root());
        assert!(RepoPath::from_internal_string("").is_root());
        assert!(!RepoPath::from_internal_string("foo").is_root());
    }

    #[test]
    fn test_to_internal_string() {
        assert_eq!(RepoPath::root().to_internal_file_string(), "");
        assert_eq!(
            RepoPath::from_internal_string("dir").to_internal_file_string(),
            "dir"
        );
        assert_eq!(
            RepoPath::from_internal_string("dir/file").to_internal_file_string(),
            "dir/file"
        );
    }

    #[test]
    fn test_order() {
        assert!(RepoPath::root() < RepoPath::from_internal_string("dir"));
        assert!(RepoPath::from_internal_string("dir") < RepoPath::from_internal_string("dirx"));
        // '#' < '/'
        assert!(RepoPath::from_internal_string("dir") < RepoPath::from_internal_string("dir#"));
        assert!(RepoPath::from_internal_string("dir") < RepoPath::from_internal_string("dir/sub"));

        assert!(RepoPath::from_internal_string("abc") < RepoPath::from_internal_string("dir/file"));
        assert!(RepoPath::from_internal_string("dir") < RepoPath::from_internal_string("dir/file"));
        assert!(RepoPath::from_internal_string("dis") > RepoPath::from_internal_string("dir/file"));
        assert!(RepoPath::from_internal_string("xyz") > RepoPath::from_internal_string("dir/file"));
        assert!(
            RepoPath::from_internal_string("dir1/xyz") < RepoPath::from_internal_string("dir2/abc")
        );
    }

    #[test]
    fn test_join() {
        let root = RepoPath::root();
        let dir = root.join(&RepoPathComponent::from("dir"));
        assert_eq!(dir, RepoPath::from_internal_string("dir"));
        let subdir = dir.join(&RepoPathComponent::from("subdir"));
        assert_eq!(subdir, RepoPath::from_internal_string("dir/subdir"));
        assert_eq!(
            subdir.join(&RepoPathComponent::from("file")),
            RepoPath::from_internal_string("dir/subdir/file")
        );
    }

    #[test]
    fn test_parent() {
        let root = RepoPath::root();
        let dir_component = RepoPathComponent::from("dir");
        let subdir_component = RepoPathComponent::from("subdir");

        let dir = root.join(&dir_component);
        let subdir = dir.join(&subdir_component);

        assert_eq!(root.parent(), None);
        assert_eq!(dir.parent(), Some(root));
        assert_eq!(subdir.parent(), Some(dir));
    }

    #[test]
    fn test_split() {
        let root = RepoPath::root();
        let dir_component = RepoPathComponent::from("dir");
        let file_component = RepoPathComponent::from("file");

        let dir = root.join(&dir_component);
        let file = dir.join(&file_component);

        assert_eq!(root.split(), None);
        assert_eq!(dir.split(), Some((root, &dir_component)));
        assert_eq!(file.split(), Some((dir, &file_component)));
    }

    #[test]
    fn test_components() {
        assert_eq!(RepoPath::root().components(), &vec![]);
        assert_eq!(
            RepoPath::from_internal_string("dir").components(),
            &vec![RepoPathComponent::from("dir")]
        );
        assert_eq!(
            RepoPath::from_internal_string("dir/subdir").components(),
            &vec![
                RepoPathComponent::from("dir"),
                RepoPathComponent::from("subdir")
            ]
        );
    }

    #[test]
    fn test_to_fs_path() {
        assert_eq!(
            RepoPath::from_internal_string("").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir")
        );
        assert_eq!(
            RepoPath::from_internal_string("").to_fs_path(Path::new("")),
            Path::new("")
        );
        assert_eq!(
            RepoPath::from_internal_string("file").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir/file")
        );
        assert_eq!(
            RepoPath::from_internal_string("some/deep/dir/file").to_fs_path(Path::new("base/dir")),
            Path::new("base/dir/some/deep/dir/file")
        );
        assert_eq!(
            RepoPath::from_internal_string("dir/file").to_fs_path(Path::new("")),
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
            Ok(RepoPath::from_internal_string("file"))
        );
        // Both slash and the platform's separator are allowed
        assert_eq!(
            RepoPath::parse_fs_path(
                &cwd_path,
                wc_path,
                format!("dir{}file", std::path::MAIN_SEPARATOR)
            ),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, wc_path, "dir/file"),
            Ok(RepoPath::from_internal_string("dir/file"))
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
            Ok(RepoPath::from_internal_string("file"))
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
            Ok(RepoPath::from_internal_string("dir"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "."),
            Ok(RepoPath::from_internal_string("dir"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "file"),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "subdir/file"),
            Ok(RepoPath::from_internal_string("dir/subdir/file"))
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
            Ok(RepoPath::from_internal_string("other-dir/file"))
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
            Ok(RepoPath::from_internal_string("file"))
        );
        assert_eq!(
            RepoPath::parse_fs_path(&cwd_path, &wc_path, "repo/dir/file"),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
    }
}
