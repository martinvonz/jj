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

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use crate::repo_path::{RepoPath, RepoPathComponent};

#[derive(PartialEq, Eq, Debug)]
pub struct Visit<'matcher> {
    pub dirs: VisitDirs<'matcher>,
    pub files: VisitFiles<'matcher>,
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitDirs<'matcher> {
    All,
    Set(&'matcher HashSet<RepoPathComponent>),
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitFiles<'matcher> {
    All,
    Set(&'matcher HashSet<RepoPathComponent>),
}

pub trait Matcher {
    fn matches(&self, file: &RepoPath) -> bool;
    fn visit(&self, dir: &RepoPath) -> Visit;
}

#[derive(PartialEq, Eq, Debug)]
pub struct EverythingMatcher;

impl Matcher for EverythingMatcher {
    fn matches(&self, _file: &RepoPath) -> bool {
        true
    }

    fn visit(&self, _dir: &RepoPath) -> Visit {
        Visit {
            dirs: VisitDirs::All,
            files: VisitFiles::All,
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct FilesMatcher {
    files: HashSet<RepoPath>,
    dirs: Dirs,
}

impl FilesMatcher {
    pub fn new(files: HashSet<RepoPath>) -> Self {
        let mut dirs = Dirs::new();
        for f in &files {
            dirs.add_file(f);
        }
        FilesMatcher { files, dirs }
    }
}

impl Matcher for FilesMatcher {
    fn matches(&self, file: &RepoPath) -> bool {
        self.files.contains(file)
    }

    fn visit(&self, dir: &RepoPath) -> Visit {
        let dirs = self.dirs.get_dirs(dir);
        let files = self.dirs.get_files(dir);
        Visit {
            dirs: VisitDirs::Set(dirs),
            files: VisitFiles::Set(files),
        }
    }
}

/// Keeps track of which subdirectories and files of each directory need to be
/// visited.
#[derive(PartialEq, Eq, Debug)]
struct Dirs {
    dirs: HashMap<RepoPath, HashSet<RepoPathComponent>>,
    files: HashMap<RepoPath, HashSet<RepoPathComponent>>,
    empty_dirs: HashSet<RepoPathComponent>,
    empty_files: HashSet<RepoPathComponent>,
}

impl Dirs {
    fn new() -> Self {
        Dirs {
            dirs: HashMap::new(),
            files: HashMap::new(),
            empty_dirs: HashSet::new(),
            empty_files: HashSet::new(),
        }
    }

    fn add_dir(&mut self, mut dir: RepoPath) {
        let mut maybe_child = None;
        loop {
            let was_present = self.dirs.contains_key(&dir);
            let children = self.dirs.entry(dir.clone()).or_default();
            if let Some(child) = maybe_child {
                children.insert(child);
            }
            if was_present {
                break;
            }
            match dir.split() {
                None => break,
                Some((new_dir, new_child)) => {
                    maybe_child = Some(new_child.clone());
                    dir = new_dir;
                }
            };
        }
    }

    fn add_file(&mut self, file: &RepoPath) {
        let (dir, basename) = file
            .split()
            .unwrap_or_else(|| panic!("got empty filename: {:?}", file));
        self.add_dir(dir.clone());
        self.files.entry(dir).or_default().insert(basename.clone());
    }

    fn get_dirs(&self, dir: &RepoPath) -> &HashSet<RepoPathComponent> {
        self.dirs.get(dir).unwrap_or(&self.empty_dirs)
    }

    fn get_files(&self, dir: &RepoPath) -> &HashSet<RepoPathComponent> {
        self.files.get(dir).unwrap_or(&self.empty_files)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::repo_path::{RepoPath, RepoPathComponent};

    #[test]
    fn dirs_empty() {
        let dirs = Dirs::new();
        assert_eq!(dirs.get_dirs(&RepoPath::root()), &hashset! {});
    }

    #[test]
    fn dirs_root() {
        let mut dirs = Dirs::new();
        dirs.add_dir(RepoPath::root());
        assert_eq!(dirs.get_dirs(&RepoPath::root()), &hashset! {});
    }

    #[test]
    fn dirs_dir() {
        let mut dirs = Dirs::new();
        dirs.add_dir(RepoPath::from_internal_string("dir"));
        assert_eq!(
            dirs.get_dirs(&RepoPath::root()),
            &hashset! {RepoPathComponent::from("dir")}
        );
    }

    #[test]
    fn dirs_file() {
        let mut dirs = Dirs::new();
        dirs.add_file(&RepoPath::from_internal_string("dir/file"));
        assert_eq!(
            dirs.get_dirs(&RepoPath::root()),
            &hashset! {RepoPathComponent::from("dir")}
        );
        assert_eq!(dirs.get_files(&RepoPath::root()), &hashset! {});
    }

    #[test]
    fn filesmatcher_empty() {
        let m = FilesMatcher::new(HashSet::new());
        assert!(!m.matches(&RepoPath::from_internal_string("file")));
        assert!(!m.matches(&RepoPath::from_internal_string("dir/file")));
        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit {
                dirs: VisitDirs::Set(&HashSet::new()),
                files: VisitFiles::Set(&HashSet::new()),
            }
        );
    }

    #[test]
    fn filesmatcher_nonempty() {
        let m = FilesMatcher::new(hashset! {
            RepoPath::from_internal_string("dir1/subdir1/file1"),
            RepoPath::from_internal_string("dir1/subdir1/file2"),
            RepoPath::from_internal_string("dir1/subdir2/file3"),
            RepoPath::from_internal_string("file4"),
        });

        assert_eq!(
            m.visit(&RepoPath::root()),
            Visit {
                dirs: VisitDirs::Set(&hashset! {RepoPathComponent::from("dir1")}),
                files: VisitFiles::Set(&hashset! {RepoPathComponent::from("file4")}),
            }
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("dir1")),
            Visit {
                dirs: VisitDirs::Set(
                    &hashset! {RepoPathComponent::from("subdir1"), RepoPathComponent::from("subdir2")}
                ),
                files: VisitFiles::Set(&hashset! {}),
            }
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("dir1/subdir1")),
            Visit {
                dirs: VisitDirs::Set(&hashset! {}),
                files: VisitFiles::Set(
                    &hashset! {RepoPathComponent::from("file1"), RepoPathComponent::from("file2")}
                ),
            }
        );
        assert_eq!(
            m.visit(&RepoPath::from_internal_string("dir1/subdir2")),
            Visit {
                dirs: VisitDirs::Set(&hashset! {}),
                files: VisitFiles::Set(&hashset! {RepoPathComponent::from("file3")}),
            }
        );
    }
}
