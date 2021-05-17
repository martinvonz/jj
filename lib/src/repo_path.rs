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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct RepoPathComponent {
    value: String,
}

impl RepoPathComponent {
    pub fn value(&self) -> &str {
        &self.value
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

// Does not include a trailing slash
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct DirRepoPathComponent {
    value: String,
}

impl DirRepoPathComponent {
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl From<&str> for DirRepoPathComponent {
    fn from(value: &str) -> Self {
        assert!(!value.contains('/'));
        DirRepoPathComponent {
            value: value.to_owned(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath {
    dir: DirRepoPath,
    basename: RepoPathComponent,
}

impl Debug for RepoPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_fmt(format_args!("{:?}", &self.to_internal_file_string()))
    }
}

impl RepoPath {
    pub fn root() -> Self {
        RepoPath {
            dir: DirRepoPath::root(),
            basename: RepoPathComponent {
                value: String::from(""),
            },
        }
    }

    pub fn new(dir: DirRepoPath, basename: RepoPathComponent) -> Self {
        RepoPath { dir, basename }
    }

    pub fn from_internal_string(value: &str) -> Self {
        assert!(!value.ends_with('/'));
        match value.rfind('/') {
            None => RepoPath {
                dir: DirRepoPath::root(),
                basename: RepoPathComponent::from(value),
            },
            Some(i) => RepoPath {
                dir: DirRepoPath::from_internal_dir_string(&value[..=i]),
                basename: RepoPathComponent::from(&value[i + 1..]),
            },
        }
    }

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn to_internal_file_string(&self) -> String {
        self.dir.to_internal_dir_string() + self.basename.value()
    }

    pub fn to_fs_path(&self, base: &Path) -> PathBuf {
        let mut result = base.to_owned();
        for dir in self.dir.components() {
            result = result.join(&dir.value);
        }
        result.join(&self.basename.value)
    }

    pub fn is_root(&self) -> bool {
        self.dir.is_root() && self.basename.value.is_empty()
    }

    pub fn dir(&self) -> Option<&DirRepoPath> {
        if self.is_root() {
            None
        } else {
            Some(&self.dir)
        }
    }

    pub fn split(&self) -> Option<(&DirRepoPath, &RepoPathComponent)> {
        if self.is_root() {
            None
        } else {
            Some((&self.dir, &self.basename))
        }
    }
}

// Includes a trailing slash
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirRepoPath {
    value: Vec<DirRepoPathComponent>,
}

impl Debug for DirRepoPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_fmt(format_args!("{:?}", &self.to_internal_dir_string()))
    }
}

impl DirRepoPath {
    pub fn root() -> Self {
        DirRepoPath { value: Vec::new() }
    }

    pub fn is_root(&self) -> bool {
        return self.components().is_empty();
    }

    pub fn from_internal_dir_string(value: &str) -> Self {
        assert!(value.is_empty() || value.ends_with('/'));
        let mut parts: Vec<&str> = value.split('/').collect();
        // remove the trailing empty string
        parts.pop();

        DirRepoPath {
            value: parts
                .iter()
                .map(|x| DirRepoPathComponent {
                    value: x.to_string(),
                })
                .collect(),
        }
    }

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn to_internal_dir_string(&self) -> String {
        let mut result = String::new();
        for component in &self.value {
            result.push_str(component.value());
            result.push('/');
        }
        result
    }

    // TODO: consider making this return a Option<DirRepoPathSlice> or similar,
    // where the slice would borrow from this instance.
    pub fn parent(&self) -> Option<DirRepoPath> {
        match self.value.len() {
            0 => None,
            n => Some(DirRepoPath {
                value: self.value[..n - 1].to_vec(),
            }),
        }
    }

    pub fn split(&self) -> Option<(DirRepoPath, DirRepoPathComponent)> {
        match self.value.len() {
            0 => None,
            n => Some((
                DirRepoPath {
                    value: self.value[..n - 1].to_vec(),
                },
                self.value[n - 1].clone(),
            )),
        }
    }

    pub fn components(&self) -> &Vec<DirRepoPathComponent> {
        &self.value
    }
}

pub trait RepoPathJoin<T> {
    type Result;

    fn join(&self, entry: &T) -> Self::Result;
}

impl RepoPathJoin<DirRepoPathComponent> for DirRepoPath {
    type Result = DirRepoPath;

    fn join(&self, entry: &DirRepoPathComponent) -> DirRepoPath {
        let mut new_dir = self.value.clone();
        new_dir.push(entry.clone());
        DirRepoPath { value: new_dir }
    }
}

impl RepoPathJoin<RepoPathComponent> for DirRepoPath {
    type Result = RepoPath;

    fn join(&self, entry: &RepoPathComponent) -> RepoPath {
        RepoPath {
            dir: self.clone(),
            basename: entry.clone(),
        }
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
        assert!(DirRepoPath::root().is_root());
        assert!(DirRepoPath::from_internal_dir_string("").is_root());
        assert!(!DirRepoPath::from_internal_dir_string("foo/").is_root());
    }

    #[test]
    fn test_to_internal_string() {
        assert_eq!(RepoPath::root().to_internal_file_string(), "");
        assert_eq!(
            RepoPath::from_internal_string("dir").to_internal_file_string(),
            "dir"
        );
        assert_eq!(
            RepoPath::from_internal_string("file").to_internal_file_string(),
            "file"
        );
        assert_eq!(
            RepoPath::from_internal_string("dir/file").to_internal_file_string(),
            "dir/file"
        );
        assert_eq!(DirRepoPath::root().to_internal_dir_string(), "");
        assert_eq!(
            DirRepoPath::from_internal_dir_string("dir/").to_internal_dir_string(),
            "dir/"
        );
        assert_eq!(
            DirRepoPath::from_internal_dir_string("dir/subdir/").to_internal_dir_string(),
            "dir/subdir/"
        );
    }

    #[test]
    fn test_order() {
        assert!(DirRepoPath::root() < DirRepoPath::from_internal_dir_string("dir/"));
        assert!(
            DirRepoPath::from_internal_dir_string("dir/")
                < DirRepoPath::from_internal_dir_string("dirx/")
        );
        // '#' < '/'
        assert!(
            DirRepoPath::from_internal_dir_string("dir/")
                < DirRepoPath::from_internal_dir_string("dir#/")
        );
        assert!(
            DirRepoPath::from_internal_dir_string("dir/")
                < DirRepoPath::from_internal_dir_string("dir/sub/")
        );

        assert!(RepoPath::from_internal_string("abc") < RepoPath::from_internal_string("dir/file"));
        assert!(RepoPath::from_internal_string("dir") < RepoPath::from_internal_string("dir/file"));
        assert!(RepoPath::from_internal_string("dis") < RepoPath::from_internal_string("dir/file"));
        assert!(RepoPath::from_internal_string("xyz") < RepoPath::from_internal_string("dir/file"));
        assert!(
            RepoPath::from_internal_string("dir1/xyz") < RepoPath::from_internal_string("dir2/abc")
        );
    }

    #[test]
    fn test_join() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let subdir_component = DirRepoPathComponent::from("subdir");
        let file_component = RepoPathComponent::from("file");
        assert_eq!(
            root.join(&file_component),
            RepoPath::from_internal_string("file")
        );
        let dir = root.join(&dir_component);
        assert_eq!(dir, DirRepoPath::from_internal_dir_string("dir/"));
        assert_eq!(
            dir.join(&file_component),
            RepoPath::from_internal_string("dir/file")
        );
        let subdir = dir.join(&subdir_component);
        assert_eq!(subdir, DirRepoPath::from_internal_dir_string("dir/subdir/"));
        assert_eq!(
            subdir.join(&file_component),
            RepoPath::from_internal_string("dir/subdir/file")
        );
    }

    #[test]
    fn test_parent() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let subdir_component = DirRepoPathComponent::from("subdir");

        let dir = root.join(&dir_component);
        let subdir = dir.join(&subdir_component);

        assert_eq!(root.parent(), None);
        assert_eq!(dir.parent(), Some(root));
        assert_eq!(subdir.parent(), Some(dir));
    }

    #[test]
    fn test_split_dir() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let subdir_component = DirRepoPathComponent::from("subdir");

        let dir = root.join(&dir_component);
        let subdir = dir.join(&subdir_component);

        assert_eq!(root.split(), None);
        assert_eq!(dir.split(), Some((root, dir_component)));
        assert_eq!(subdir.split(), Some((dir, subdir_component)));
    }

    #[test]
    fn test_split_file() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let file_component = RepoPathComponent::from("file");

        let dir = root.join(&dir_component);

        assert_eq!(
            root.join(&file_component).split(),
            Some((&root, &file_component.clone()))
        );
        assert_eq!(
            dir.join(&file_component).split(),
            Some((&dir, &file_component))
        );
    }

    #[test]
    fn test_dir() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let file_component = RepoPathComponent::from("file");

        let dir = root.join(&dir_component);

        assert_eq!(root.join(&file_component).dir(), Some(&root));
        assert_eq!(dir.join(&file_component).dir(), Some(&dir));
    }

    #[test]
    fn test_components() {
        assert_eq!(DirRepoPath::root().components(), &vec![]);
        assert_eq!(
            DirRepoPath::from_internal_dir_string("dir/").components(),
            &vec![DirRepoPathComponent::from("dir")]
        );
        assert_eq!(
            DirRepoPath::from_internal_dir_string("dir/subdir/").components(),
            &vec![
                DirRepoPathComponent::from("dir"),
                DirRepoPathComponent::from("subdir")
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
