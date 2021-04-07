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

use std::borrow::Borrow;
use std::fmt::{Debug, Error, Formatter};
use std::pin::Pin;
use std::sync::Arc;

use crate::repo_path::{
    DirRepoPath, DirRepoPathComponent, FileRepoPath, RepoPath, RepoPathComponent, RepoPathJoin,
};
use crate::store;
use crate::store::{ConflictId, TreeEntriesNonRecursiveIter, TreeEntry, TreeId, TreeValue};
use crate::store_wrapper::StoreWrapper;
use crate::trees::{recursive_tree_diff, Diff, TreeDiffIterator};

#[derive(Clone)]
pub struct Tree {
    store: Arc<StoreWrapper>,
    dir: DirRepoPath,
    id: TreeId,
    data: Arc<store::Tree>,
}

impl Debug for Tree {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("Tree")
            .field("dir", &self.dir)
            .field("id", &self.id)
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DiffSummary {
    pub modified: Vec<FileRepoPath>,
    pub added: Vec<FileRepoPath>,
    pub removed: Vec<FileRepoPath>,
}

impl Tree {
    pub fn new(
        store: Arc<StoreWrapper>,
        dir: DirRepoPath,
        id: TreeId,
        data: Arc<store::Tree>,
    ) -> Self {
        Tree {
            store,
            dir,
            id,
            data,
        }
    }

    pub fn null(store: Arc<StoreWrapper>, dir: DirRepoPath) -> Self {
        Tree {
            store,
            dir,
            id: TreeId(vec![]),
            data: Arc::new(store::Tree::default()),
        }
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        &self.store
    }

    pub fn dir(&self) -> &DirRepoPath {
        &self.dir
    }

    pub fn id(&self) -> &TreeId {
        &self.id
    }

    pub fn data(&self) -> &store::Tree {
        &self.data
    }

    pub fn entries_non_recursive(&self) -> TreeEntriesNonRecursiveIter {
        self.data.entries()
    }

    pub fn entries(&self) -> TreeEntriesIter {
        TreeEntriesIter::new(self.clone())
    }

    pub fn entry<N>(&self, basename: &N) -> Option<TreeEntry>
    where
        N: Borrow<str> + ?Sized,
    {
        self.data.entry(basename)
    }

    pub fn value<N>(&self, basename: &N) -> Option<&TreeValue>
    where
        N: Borrow<str> + ?Sized,
    {
        self.data.value(basename)
    }

    pub fn path_value(&self, path: &RepoPath) -> Option<TreeValue> {
        assert_eq!(self.dir(), &DirRepoPath::root());
        match path.split() {
            Some((dir, basename)) => self
                .sub_tree_recursive(dir.components())
                .and_then(|tree| tree.data.value(basename.value()).cloned()),
            None => Some(TreeValue::Tree(self.id.clone())),
        }
    }

    pub fn sub_tree(&self, name: &DirRepoPathComponent) -> Option<Tree> {
        self.data
            .value(name.value())
            .and_then(|sub_tree| match sub_tree {
                TreeValue::Tree(sub_tree_id) => {
                    let subdir = self.dir.join(name);
                    Some(self.store.get_tree(&subdir, sub_tree_id).unwrap())
                }
                _ => None,
            })
    }

    pub fn known_sub_tree(&self, name: &DirRepoPathComponent, id: &TreeId) -> Tree {
        let subdir = self.dir.join(name);
        self.store.get_tree(&subdir, id).unwrap()
    }

    fn sub_tree_recursive(&self, components: &[DirRepoPathComponent]) -> Option<Tree> {
        if components.is_empty() {
            // TODO: It would be nice to be able to return a reference here, but
            // then we would have to figure out how to share Tree instances
            // across threads.
            Some(Tree {
                store: self.store.clone(),
                dir: self.dir.clone(),
                id: self.id.clone(),
                data: self.data.clone(),
            })
        } else {
            match self.data.entry(components[0].value()) {
                None => None,
                Some(entry) => match entry.value() {
                    TreeValue::Tree(sub_tree_id) => {
                        let sub_tree = self
                            .known_sub_tree(&DirRepoPathComponent::from(entry.name()), sub_tree_id);
                        sub_tree.sub_tree_recursive(&components[1..])
                    }
                    _ => None,
                },
            }
        }
    }

    pub fn diff<'a>(&'a self, other: &'a Tree) -> TreeDiffIterator {
        recursive_tree_diff(self.clone(), other.clone())
    }

    pub fn diff_summary(&self, other: &Tree) -> DiffSummary {
        let mut modified = vec![];
        let mut added = vec![];
        let mut removed = vec![];
        for (file, diff) in self.diff(other) {
            match diff {
                Diff::Modified(_, _) => modified.push(file.clone()),
                Diff::Added(_) => added.push(file.clone()),
                Diff::Removed(_) => removed.push(file.clone()),
            }
        }
        modified.sort();
        added.sort();
        removed.sort();
        DiffSummary {
            modified,
            added,
            removed,
        }
    }

    pub fn has_conflict(&self) -> bool {
        !self.conflicts().is_empty()
    }

    pub fn conflicts(&self) -> Vec<(RepoPath, ConflictId)> {
        let mut conflicts = vec![];
        for (name, value) in self.entries() {
            if let TreeValue::Conflict(id) = value {
                conflicts.push((name.clone(), id.clone()));
            }
        }
        conflicts
    }
}

pub struct TreeEntriesIter {
    stack: Vec<(Pin<Box<Tree>>, TreeEntriesNonRecursiveIter<'static>)>,
}

impl TreeEntriesIter {
    fn new(tree: Tree) -> Self {
        let tree = Box::pin(tree);
        let iter = tree.entries_non_recursive();
        let iter: TreeEntriesNonRecursiveIter<'static> = unsafe { std::mem::transmute(iter) };
        Self {
            stack: vec![(tree, iter)],
        }
    }
}

impl Iterator for TreeEntriesIter {
    type Item = (RepoPath, TreeValue);

    fn next(&mut self) -> Option<Self::Item> {
        while !self.stack.is_empty() {
            let (tree, iter) = self.stack.last_mut().unwrap();
            match iter.next() {
                None => {
                    // No more entries in this directory
                    self.stack.pop().unwrap();
                }
                Some(entry) => {
                    match entry.value() {
                        TreeValue::Tree(id) => {
                            let subtree =
                                tree.known_sub_tree(&DirRepoPathComponent::from(entry.name()), id);
                            let subtree = Box::pin(subtree);
                            let iter = subtree.entries_non_recursive();
                            let subtree_iter: TreeEntriesNonRecursiveIter<'static> =
                                unsafe { std::mem::transmute(iter) };
                            self.stack.push((subtree, subtree_iter));
                        }
                        other => {
                            let path = RepoPath::new(
                                tree.dir().clone(),
                                RepoPathComponent::from(entry.name()),
                            );
                            return Some((path, other.clone()));
                        }
                    };
                }
            }
        }
        None
    }
}
