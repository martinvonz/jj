// Copyright 2020 Google LLC
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

use std::fmt::{Debug, Error, Formatter};
use std::path::{Path, PathBuf};

use itertools::Itertools;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct RepoPathComponent {
    value: String,
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
        assert!(!value.contains('/'));
        RepoPathComponent {
            value: value.to_owned(),
        }
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
        let mut result = base.to_owned();
        for dir in &self.components {
            result = result.join(&dir.value);
        }
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
        let mut components: Vec<RepoPathComponent> = self.components.clone();
        components.push(entry.clone());
        RepoPath { components }
    }
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
            RepoPath::from_internal_string("").to_fs_path(&Path::new("base/dir")),
            Path::new("base/dir")
        );
        assert_eq!(
            RepoPath::from_internal_string("").to_fs_path(&Path::new("")),
            Path::new("")
        );
        assert_eq!(
            RepoPath::from_internal_string("file").to_fs_path(&Path::new("base/dir")),
            Path::new("base/dir/file")
        );
        assert_eq!(
            RepoPath::from_internal_string("some/deep/dir/file").to_fs_path(&Path::new("base/dir")),
            Path::new("base/dir/some/deep/dir/file")
        );
        assert_eq!(
            RepoPath::from_internal_string("dir/file").to_fs_path(&Path::new("")),
            Path::new("dir/file")
        );
    }
}
