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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct FileRepoPathComponent {
    value: String,
}

impl FileRepoPathComponent {
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl From<&str> for FileRepoPathComponent {
    fn from(value: &str) -> Self {
        assert!(!value.contains('/'));
        assert!(!value.is_empty());
        FileRepoPathComponent {
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
        f.write_fmt(format_args!("{:?}", &self.to_internal_string()))
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

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn to_internal_string(&self) -> String {
        self.dir.to_internal_string() + self.basename.value()
    }

    pub fn to_file_repo_path(&self) -> FileRepoPath {
        FileRepoPath {
            dir: self.dir.clone(),
            basename: FileRepoPathComponent {
                value: self.basename.value.clone(),
            },
        }
    }
    pub fn to_dir_repo_path(&self) -> DirRepoPath {
        if self.is_root() {
            DirRepoPath::root()
        } else {
            self.dir.join(&DirRepoPathComponent {
                value: self.basename.value.clone(),
            })
        }
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

impl From<&str> for RepoPath {
    fn from(value: &str) -> Self {
        assert!(!value.ends_with('/'));
        match value.rfind('/') {
            None => RepoPath {
                dir: DirRepoPath::root(),
                basename: RepoPathComponent::from(value),
            },
            Some(i) => RepoPath {
                dir: DirRepoPath::from(&value[..=i]),
                basename: RepoPathComponent::from(&value[i + 1..]),
            },
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
        f.write_fmt(format_args!("{:?}", &self.to_internal_string()))
    }
}

impl DirRepoPath {
    pub fn root() -> Self {
        DirRepoPath { value: Vec::new() }
    }

    pub fn is_root(&self) -> bool {
        return self.components().is_empty();
    }

    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn to_internal_string(&self) -> String {
        let mut result = String::new();
        for component in &self.value {
            result.push_str(component.value());
            result.push('/');
        }
        result
    }

    pub fn contains_dir(&self, other: &DirRepoPath) -> bool {
        other.value.starts_with(&self.value)
    }

    pub fn contains_file(&self, other: &FileRepoPath) -> bool {
        other.dir.value.starts_with(&self.value)
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

impl From<&str> for DirRepoPath {
    fn from(value: &str) -> Self {
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
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileRepoPath {
    dir: DirRepoPath,
    basename: FileRepoPathComponent,
}

impl Debug for FileRepoPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_fmt(format_args!("{:?}", &self.to_internal_string()))
    }
}

impl FileRepoPath {
    /// The full string form used internally, not for presenting to users (where
    /// we may want to use the platform's separator).
    pub fn to_internal_string(&self) -> String {
        self.dir.to_internal_string() + self.basename.value()
    }

    pub fn dir(&self) -> &DirRepoPath {
        &self.dir
    }

    pub fn split(&self) -> (&DirRepoPath, &FileRepoPathComponent) {
        (&self.dir, &self.basename)
    }

    pub fn to_repo_path(&self) -> RepoPath {
        RepoPath {
            dir: self.dir.clone(),
            basename: RepoPathComponent {
                value: self.basename.value.clone(),
            },
        }
    }
}

impl From<&str> for FileRepoPath {
    fn from(value: &str) -> Self {
        assert!(!value.ends_with('/'));
        match value.rfind('/') {
            None => FileRepoPath {
                dir: DirRepoPath::root(),
                basename: FileRepoPathComponent::from(value),
            },
            Some(i) => FileRepoPath {
                dir: DirRepoPath::from(&value[..=i]),
                basename: FileRepoPathComponent::from(&value[i + 1..]),
            },
        }
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

impl RepoPathJoin<FileRepoPathComponent> for DirRepoPath {
    type Result = FileRepoPath;

    fn join(&self, entry: &FileRepoPathComponent) -> FileRepoPath {
        FileRepoPath {
            dir: self.clone(),
            basename: entry.clone(),
        }
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
    fn is_root() {
        assert!(RepoPath::root().is_root());
        assert!(RepoPath::from("").is_root());
        assert!(!RepoPath::from("foo").is_root());
        assert!(DirRepoPath::root().is_root());
        assert!(DirRepoPath::from("").is_root());
        assert!(!DirRepoPath::from("foo/").is_root());
    }

    #[test]
    fn value() {
        assert_eq!(RepoPath::root().to_internal_string(), "");
        assert_eq!(RepoPath::from("dir").to_internal_string(), "dir");
        assert_eq!(RepoPath::from("file").to_internal_string(), "file");
        assert_eq!(RepoPath::from("dir/file").to_internal_string(), "dir/file");
        assert_eq!(DirRepoPath::root().to_internal_string(), "");
        assert_eq!(DirRepoPath::from("dir/").to_internal_string(), "dir/");
        assert_eq!(
            DirRepoPath::from("dir/subdir/").to_internal_string(),
            "dir/subdir/"
        );
        assert_eq!(FileRepoPath::from("file").to_internal_string(), "file");
        assert_eq!(
            FileRepoPath::from("dir/file").to_internal_string(),
            "dir/file"
        );
    }

    #[test]
    fn order() {
        assert!(DirRepoPath::root() < DirRepoPath::from("dir/"));
        assert!(DirRepoPath::from("dir/") < DirRepoPath::from("dirx/"));
        // '#' < '/'
        assert!(DirRepoPath::from("dir/") < DirRepoPath::from("dir#/"));
        assert!(DirRepoPath::from("dir/") < DirRepoPath::from("dir/sub/"));

        assert!(FileRepoPath::from("abc") < FileRepoPath::from("dir/file"));
        assert!(FileRepoPath::from("dir") < FileRepoPath::from("dir/file"));
        assert!(FileRepoPath::from("dis") < FileRepoPath::from("dir/file"));
        assert!(FileRepoPath::from("xyz") < FileRepoPath::from("dir/file"));
        assert!(FileRepoPath::from("dir1/xyz") < FileRepoPath::from("dir2/abc"));
    }

    #[test]
    fn join() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let subdir_component = DirRepoPathComponent::from("subdir");
        let file_component = FileRepoPathComponent::from("file");
        assert_eq!(root.join(&file_component), FileRepoPath::from("file"));
        let dir = root.join(&dir_component);
        assert_eq!(dir, DirRepoPath::from("dir/"));
        assert_eq!(dir.join(&file_component), FileRepoPath::from("dir/file"));
        let subdir = dir.join(&subdir_component);
        assert_eq!(subdir, DirRepoPath::from("dir/subdir/"));
        assert_eq!(
            subdir.join(&file_component),
            FileRepoPath::from("dir/subdir/file")
        );
    }

    #[test]
    fn parent() {
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
    fn split_dir() {
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
    fn split_file() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let file_component = FileRepoPathComponent::from("file");

        let dir = root.join(&dir_component);

        assert_eq!(
            root.join(&file_component).split(),
            (&root, &file_component.clone())
        );
        assert_eq!(dir.join(&file_component).split(), (&dir, &file_component));
    }

    #[test]
    fn dir() {
        let root = DirRepoPath::root();
        let dir_component = DirRepoPathComponent::from("dir");
        let file_component = FileRepoPathComponent::from("file");

        let dir = root.join(&dir_component);

        assert_eq!(root.join(&file_component).dir(), &root);
        assert_eq!(dir.join(&file_component).dir(), &dir);
    }

    #[test]
    fn components() {
        assert_eq!(DirRepoPath::root().components(), &vec![]);
        assert_eq!(
            DirRepoPath::from("dir/").components(),
            &vec![DirRepoPathComponent::from("dir")]
        );
        assert_eq!(
            DirRepoPath::from("dir/subdir/").components(),
            &vec![
                DirRepoPathComponent::from("dir"),
                DirRepoPathComponent::from("subdir")
            ]
        );
    }

    #[test]
    fn convert() {
        assert_eq!(RepoPath::root().to_dir_repo_path(), DirRepoPath::root());
        assert_eq!(
            RepoPath::from("dir").to_dir_repo_path(),
            DirRepoPath::from("dir/")
        );
        assert_eq!(
            RepoPath::from("dir/subdir").to_dir_repo_path(),
            DirRepoPath::from("dir/subdir/")
        );
        assert_eq!(
            RepoPath::from("file").to_file_repo_path(),
            FileRepoPath::from("file")
        );
        assert_eq!(
            RepoPath::from("dir/file").to_file_repo_path(),
            FileRepoPath::from("dir/file")
        );
    }
}
