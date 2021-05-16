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

use crate::repo_path::{DirRepoPath, DirRepoPathComponent, FileRepoPath, FileRepoPathComponent};

#[derive(PartialEq, Eq, Debug)]
pub struct Visit<'a> {
    dirs: VisitDirs<'a>,
    files: VisitFiles<'a>,
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitDirs<'a> {
    All,
    Set(&'a HashSet<DirRepoPathComponent>),
}

#[derive(PartialEq, Eq, Debug)]
pub enum VisitFiles<'a> {
    All,
    Set(&'a HashSet<FileRepoPathComponent>),
}

pub trait Matcher {
    fn matches(&self, file: &FileRepoPath) -> bool;
    fn visit(&self, dir: &DirRepoPath) -> Visit;
}

#[derive(PartialEq, Eq, Debug)]
pub struct AlwaysMatcher;

impl Matcher for AlwaysMatcher {
    fn matches(&self, _file: &FileRepoPath) -> bool {
        true
    }

    fn visit(&self, _dir: &DirRepoPath) -> Visit {
        Visit {
            dirs: VisitDirs::All,
            files: VisitFiles::All,
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct FilesMatcher {
    files: HashSet<FileRepoPath>,
    dirs: Dirs,
}

impl FilesMatcher {
    fn new(files: HashSet<FileRepoPath>) -> Self {
        let mut dirs = Dirs::new();
        for f in &files {
            dirs.add_file(f);
        }
        FilesMatcher { files, dirs }
    }
}

impl Matcher for FilesMatcher {
    fn matches(&self, file: &FileRepoPath) -> bool {
        self.files.contains(file)
    }

    fn visit(&self, dir: &DirRepoPath) -> Visit {
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
    dirs: HashMap<DirRepoPath, HashSet<DirRepoPathComponent>>,
    files: HashMap<DirRepoPath, HashSet<FileRepoPathComponent>>,
    empty_dirs: HashSet<DirRepoPathComponent>,
    empty_files: HashSet<FileRepoPathComponent>,
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

    fn add_dir(&mut self, mut dir: DirRepoPath) {
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
                    dir = new_dir;
                    maybe_child = Some(new_child);
                }
            };
        }
    }

    fn add_file(&mut self, file: &FileRepoPath) {
        let (dir, basename) = file.split();
        self.add_dir(dir.clone());
        self.files
            .entry(dir.clone())
            .or_default()
            .insert(basename.clone());
    }

    fn get_dirs(&self, dir: &DirRepoPath) -> &HashSet<DirRepoPathComponent> {
        self.dirs.get(&dir).unwrap_or(&self.empty_dirs)
    }

    fn get_files(&self, dir: &DirRepoPath) -> &HashSet<FileRepoPathComponent> {
        self.files.get(&dir).unwrap_or(&self.empty_files)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::repo_path::{
        DirRepoPath, DirRepoPathComponent, FileRepoPath, FileRepoPathComponent,
    };

    #[test]
    fn dirs_empty() {
        let dirs = Dirs::new();
        assert_eq!(dirs.get_dirs(&DirRepoPath::root()), &hashset! {});
    }

    #[test]
    fn dirs_root() {
        let mut dirs = Dirs::new();
        dirs.add_dir(DirRepoPath::root());
        assert_eq!(dirs.get_dirs(&DirRepoPath::root()), &hashset! {});
    }

    #[test]
    fn dirs_dir() {
        let mut dirs = Dirs::new();
        dirs.add_dir(DirRepoPath::from("dir/"));
        assert_eq!(
            dirs.get_dirs(&DirRepoPath::root()),
            &hashset! {DirRepoPathComponent::from("dir")}
        );
    }

    #[test]
    fn dirs_file() {
        let mut dirs = Dirs::new();
        dirs.add_file(&FileRepoPath::from("dir/file"));
        assert_eq!(
            dirs.get_dirs(&DirRepoPath::root()),
            &hashset! {DirRepoPathComponent::from("dir")}
        );
        assert_eq!(dirs.get_files(&DirRepoPath::root()), &hashset! {});
    }

    #[test]
    fn filesmatcher_empty() {
        let m = FilesMatcher::new(HashSet::new());
        assert!(!m.matches(&FileRepoPath::from("file")));
        assert!(!m.matches(&FileRepoPath::from("dir/file")));
        assert_eq!(
            m.visit(&DirRepoPath::root()),
            Visit {
                dirs: VisitDirs::Set(&HashSet::new()),
                files: VisitFiles::Set(&HashSet::new()),
            }
        );
    }

    #[test]
    fn filesmatcher_nonempty() {
        let m = FilesMatcher::new(hashset! {
            FileRepoPath::from("dir1/subdir1/file1"),
            FileRepoPath::from("dir1/subdir1/file2"),
            FileRepoPath::from("dir1/subdir2/file3"),
            FileRepoPath::from("file4"),
        });

        assert_eq!(
            m.visit(&DirRepoPath::root()),
            Visit {
                dirs: VisitDirs::Set(&hashset! {DirRepoPathComponent::from("dir1")}),
                files: VisitFiles::Set(&hashset! {FileRepoPathComponent::from("file4")}),
            }
        );
        assert_eq!(
            m.visit(&DirRepoPath::from("dir1/")),
            Visit {
                dirs: VisitDirs::Set(
                    &hashset! {DirRepoPathComponent::from("subdir1"), DirRepoPathComponent::from("subdir2")}
                ),
                files: VisitFiles::Set(&hashset! {}),
            }
        );
        assert_eq!(
            m.visit(&DirRepoPath::from("dir1/subdir1/")),
            Visit {
                dirs: VisitDirs::Set(&hashset! {}),
                files: VisitFiles::Set(
                    &hashset! {FileRepoPathComponent::from("file1"), FileRepoPathComponent::from("file2")}
                ),
            }
        );
        assert_eq!(
            m.visit(&DirRepoPath::from("dir1/subdir2/")),
            Visit {
                dirs: VisitDirs::Set(&hashset! {}),
                files: VisitFiles::Set(&hashset! {FileRepoPathComponent::from("file3")}),
            }
        );
    }
}
