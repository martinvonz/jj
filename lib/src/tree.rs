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
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::Arc;

use itertools::Itertools;
use thiserror::Error;

use crate::backend::{
    BackendError, ConflictId, FileId, ObjectId, TreeEntriesNonRecursiveIterator, TreeEntry, TreeId,
    TreeValue,
};
use crate::conflicts::Conflict;
use crate::files::MergeResult;
use crate::matchers::{EverythingMatcher, Matcher};
use crate::merge::trivial_merge;
use crate::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use crate::store::Store;
use crate::{backend, files};

#[derive(Debug, Error)]
pub enum TreeMergeError {
    #[error("Failed to read file with ID {} ", .file_id.hex())]
    ReadError {
        source: std::io::Error,
        file_id: FileId,
    },
    #[error("Backend error: {0}")]
    BackendError(#[from] BackendError),
}

#[derive(Clone)]
pub struct Tree {
    store: Arc<Store>,
    dir: RepoPath,
    id: TreeId,
    data: Arc<backend::Tree>,
}

impl Debug for Tree {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("Tree")
            .field("dir", &self.dir)
            .field("id", &self.id)
            .finish()
    }
}

impl PartialEq for Tree {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.dir == other.dir
    }
}

impl Eq for Tree {}

impl Hash for Tree {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.dir.hash(state);
        self.id.hash(state);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DiffSummary {
    pub modified: Vec<RepoPath>,
    pub added: Vec<RepoPath>,
    pub removed: Vec<RepoPath>,
}

impl DiffSummary {
    pub fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

impl Tree {
    pub fn new(store: Arc<Store>, dir: RepoPath, id: TreeId, data: Arc<backend::Tree>) -> Self {
        Tree {
            store,
            dir,
            id,
            data,
        }
    }

    pub fn null(store: Arc<Store>, dir: RepoPath) -> Self {
        Tree {
            store,
            dir,
            id: TreeId::new(vec![]),
            data: Arc::new(backend::Tree::default()),
        }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn dir(&self) -> &RepoPath {
        &self.dir
    }

    pub fn id(&self) -> &TreeId {
        &self.id
    }

    pub fn data(&self) -> &backend::Tree {
        &self.data
    }

    pub fn entries_non_recursive(&self) -> TreeEntriesNonRecursiveIterator {
        self.data.entries()
    }

    pub fn entries(&self) -> TreeEntriesIterator<'static> {
        TreeEntriesIterator::new(self.clone(), &EverythingMatcher)
    }

    pub fn entries_matching<'matcher>(
        &self,
        matcher: &'matcher dyn Matcher,
    ) -> TreeEntriesIterator<'matcher> {
        TreeEntriesIterator::new(self.clone(), matcher)
    }

    pub fn entry(&self, basename: &RepoPathComponent) -> Option<TreeEntry> {
        self.data.entry(basename)
    }

    pub fn value(&self, basename: &RepoPathComponent) -> Option<&TreeValue> {
        self.data.value(basename)
    }

    pub fn path_value(&self, path: &RepoPath) -> Option<TreeValue> {
        assert_eq!(self.dir(), &RepoPath::root());
        match path.split() {
            Some((dir, basename)) => self
                .sub_tree_recursive(dir.components())
                .and_then(|tree| tree.data.value(basename).cloned()),
            None => Some(TreeValue::Tree(self.id.clone())),
        }
    }

    pub fn sub_tree(&self, name: &RepoPathComponent) -> Option<Tree> {
        self.data.value(name).and_then(|sub_tree| match sub_tree {
            TreeValue::Tree(sub_tree_id) => {
                let subdir = self.dir.join(name);
                Some(self.store.get_tree(&subdir, sub_tree_id).unwrap())
            }
            _ => None,
        })
    }

    fn known_sub_tree(&self, subdir: &RepoPath, id: &TreeId) -> Tree {
        self.store.get_tree(subdir, id).unwrap()
    }

    fn sub_tree_recursive(&self, components: &[RepoPathComponent]) -> Option<Tree> {
        if let Some((first, tail)) = components.split_first() {
            tail.iter()
                .try_fold(self.sub_tree(first)?, |tree, name| tree.sub_tree(name))
        } else {
            // TODO: It would be nice to be able to return a reference here, but
            // then we would have to figure out how to share Tree instances
            // across threads.
            Some(self.clone())
        }
    }

    pub fn diff<'matcher>(
        &self,
        other: &Tree,
        matcher: &'matcher dyn Matcher,
    ) -> TreeDiffIterator<'matcher> {
        TreeDiffIterator::new(self.clone(), other.clone(), matcher)
    }

    pub fn diff_summary(&self, other: &Tree, matcher: &dyn Matcher) -> DiffSummary {
        let mut modified = vec![];
        let mut added = vec![];
        let mut removed = vec![];
        for (file, diff) in self.diff(other, matcher) {
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

    pub fn conflicts_matching(&self, matcher: &dyn Matcher) -> Vec<(RepoPath, ConflictId)> {
        let mut conflicts = vec![];
        for (name, value) in self.entries_matching(matcher) {
            if let TreeValue::Conflict(id) = value {
                conflicts.push((name.clone(), id.clone()));
            }
        }
        conflicts
    }

    pub fn conflicts(&self) -> Vec<(RepoPath, ConflictId)> {
        self.conflicts_matching(&EverythingMatcher)
    }

    pub fn has_conflict(&self) -> bool {
        !self.conflicts().is_empty()
    }
}

pub struct TreeEntriesIterator<'matcher> {
    stack: Vec<TreeEntriesDirItem>,
    matcher: &'matcher dyn Matcher,
}

struct TreeEntriesDirItem {
    entry_iterator: TreeEntriesNonRecursiveIterator<'static>,
    // On drop, tree must outlive entry_iterator
    tree: Box<Tree>,
}

impl TreeEntriesDirItem {
    fn new(tree: Tree) -> Self {
        let tree = Box::new(tree);
        let entry_iterator = tree.entries_non_recursive();
        let entry_iterator: TreeEntriesNonRecursiveIterator<'static> =
            unsafe { std::mem::transmute(entry_iterator) };
        Self {
            entry_iterator,
            tree,
        }
    }
}

impl<'matcher> TreeEntriesIterator<'matcher> {
    fn new(tree: Tree, matcher: &'matcher dyn Matcher) -> Self {
        // TODO: Restrict walk according to Matcher::visit()
        Self {
            stack: vec![TreeEntriesDirItem::new(tree)],
            matcher,
        }
    }
}

impl Iterator for TreeEntriesIterator<'_> {
    type Item = (RepoPath, TreeValue);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            if let Some(entry) = top.entry_iterator.next() {
                let path = top.tree.dir().join(entry.name());
                match entry.value() {
                    TreeValue::Tree(id) => {
                        // TODO: Handle the other cases (specific files and trees)
                        if self.matcher.visit(&path).is_nothing() {
                            continue;
                        }
                        let subtree = top.tree.known_sub_tree(&path, id);
                        self.stack.push(TreeEntriesDirItem::new(subtree));
                    }
                    value => {
                        if self.matcher.matches(&path) {
                            return Some((path, value.clone()));
                        }
                    }
                };
            } else {
                self.stack.pop();
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Diff<T> {
    Modified(T, T),
    Added(T),
    Removed(T),
}

impl<T> Diff<T> {
    pub fn from_options(left: Option<T>, right: Option<T>) -> Self {
        match (left, right) {
            (Some(left), Some(right)) => Diff::Modified(left, right),
            (None, Some(right)) => Diff::Added(right),
            (Some(left), None) => Diff::Removed(left),
            (None, None) => panic!("left and right cannot both be None"),
        }
    }

    pub fn into_options(self) -> (Option<T>, Option<T>) {
        match self {
            Diff::Modified(left, right) => (Some(left), Some(right)),
            Diff::Added(right) => (None, Some(right)),
            Diff::Removed(left) => (Some(left), None),
        }
    }
}

struct TreeEntryDiffIterator<'trees> {
    tree1: &'trees Tree,
    tree2: &'trees Tree,
    basename_iter: Box<dyn Iterator<Item = &'trees RepoPathComponent> + 'trees>,
}

impl<'trees> TreeEntryDiffIterator<'trees> {
    fn new(tree1: &'trees Tree, tree2: &'trees Tree) -> Self {
        let basename_iter = Box::new(tree1.data.names().merge(tree2.data.names()).dedup());
        TreeEntryDiffIterator {
            tree1,
            tree2,
            basename_iter,
        }
    }
}

impl<'trees> Iterator for TreeEntryDiffIterator<'trees> {
    type Item = (
        &'trees RepoPathComponent,
        Option<&'trees TreeValue>,
        Option<&'trees TreeValue>,
    );

    fn next(&mut self) -> Option<Self::Item> {
        for basename in self.basename_iter.by_ref() {
            let value1 = self.tree1.value(basename);
            let value2 = self.tree2.value(basename);
            if value1 != value2 {
                return Some((basename, value1, value2));
            }
        }
        None
    }
}

pub struct TreeDiffIterator<'matcher> {
    stack: Vec<TreeDiffItem>,
    matcher: &'matcher dyn Matcher,
}

struct TreeDiffDirItem {
    path: RepoPath,
    // Iterator over the diffs between tree1 and tree2
    entry_iterator: TreeEntryDiffIterator<'static>,
    // On drop, tree1 and tree2 must outlive entry_iterator
    tree1: Box<Tree>,
    tree2: Box<Tree>,
}

enum TreeDiffItem {
    Dir(TreeDiffDirItem),
    // This is used for making sure that when a directory gets replaced by a file, we
    // yield the value for the addition of the file after we yield the values
    // for removing files in the directory.
    File(RepoPath, Diff<TreeValue>),
}

impl<'matcher> TreeDiffIterator<'matcher> {
    fn new(tree1: Tree, tree2: Tree, matcher: &'matcher dyn Matcher) -> Self {
        let root_dir = RepoPath::root();
        let mut stack = Vec::new();
        if !matcher.visit(&root_dir).is_nothing() {
            stack.push(TreeDiffItem::Dir(TreeDiffDirItem::new(
                root_dir, tree1, tree2,
            )));
        };
        Self { stack, matcher }
    }
}

impl TreeDiffDirItem {
    fn new(path: RepoPath, tree1: Tree, tree2: Tree) -> Self {
        let tree1 = Box::new(tree1);
        let tree2 = Box::new(tree2);
        let iter: TreeEntryDiffIterator = TreeEntryDiffIterator::new(&tree1, &tree2);
        let iter: TreeEntryDiffIterator<'static> = unsafe { std::mem::transmute(iter) };
        Self {
            path,
            entry_iterator: iter,
            tree1,
            tree2,
        }
    }

    fn subdir(
        &self,
        subdir_path: &RepoPath,
        before: Option<&TreeValue>,
        after: Option<&TreeValue>,
    ) -> Self {
        let before_tree = match before {
            Some(TreeValue::Tree(id_before)) => self.tree1.known_sub_tree(subdir_path, id_before),
            _ => Tree::null(self.tree1.store().clone(), subdir_path.clone()),
        };
        let after_tree = match after {
            Some(TreeValue::Tree(id_after)) => self.tree2.known_sub_tree(subdir_path, id_after),
            _ => Tree::null(self.tree2.store().clone(), subdir_path.clone()),
        };
        Self::new(subdir_path.clone(), before_tree, after_tree)
    }
}

impl Iterator for TreeDiffIterator<'_> {
    type Item = (RepoPath, Diff<TreeValue>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            let (dir, (name, before, after)) = match top {
                TreeDiffItem::Dir(dir) => {
                    if let Some(entry) = dir.entry_iterator.next() {
                        (dir, entry)
                    } else {
                        self.stack.pop().unwrap();
                        continue;
                    }
                }
                TreeDiffItem::File(..) => {
                    if let TreeDiffItem::File(name, diff) = self.stack.pop().unwrap() {
                        return Some((name, diff));
                    } else {
                        unreachable!();
                    }
                }
            };

            let path = dir.path.join(name);
            let tree_before = matches!(before, Some(TreeValue::Tree(_)));
            let tree_after = matches!(after, Some(TreeValue::Tree(_)));
            let post_subdir =
                if (tree_before || tree_after) && !self.matcher.visit(&path).is_nothing() {
                    let subdir = dir.subdir(&path, before, after);
                    self.stack.push(TreeDiffItem::Dir(subdir));
                    self.stack.len() - 1
                } else {
                    self.stack.len()
                };
            if self.matcher.matches(&path) {
                if !tree_before && tree_after {
                    if let Some(value_before) = before {
                        return Some((path, Diff::Removed(value_before.clone())));
                    }
                } else if tree_before && !tree_after {
                    if let Some(value_after) = after {
                        self.stack.insert(
                            post_subdir,
                            TreeDiffItem::File(path, Diff::Added(value_after.clone())),
                        );
                    }
                } else if !tree_before && !tree_after {
                    return Some((path, Diff::from_options(before.cloned(), after.cloned())));
                }
            }
        }
        None
    }
}

pub fn merge_trees(
    side1_tree: &Tree,
    base_tree: &Tree,
    side2_tree: &Tree,
) -> Result<Tree, TreeMergeError> {
    let store = base_tree.store();
    let dir = base_tree.dir();
    assert_eq!(side1_tree.dir(), dir);
    assert_eq!(side2_tree.dir(), dir);

    if let Some(resolved) = trivial_merge(&[base_tree], &[side1_tree, side2_tree]) {
        return Ok((*resolved).clone());
    }

    // Start with a tree identical to side 1 and modify based on changes from base
    // to side 2.
    let mut new_tree = side1_tree.data().clone();
    for (basename, maybe_base, maybe_side2) in TreeEntryDiffIterator::new(base_tree, side2_tree) {
        let maybe_side1 = side1_tree.value(basename);
        if maybe_side1 == maybe_base {
            // side 1 is unchanged: use the value from side 2
            new_tree.set_or_remove(basename, maybe_side2.cloned());
        } else if maybe_side1 == maybe_side2 {
            // Both sides changed in the same way: new_tree already has the
            // value
        } else {
            // The two sides changed in different ways
            let new_value =
                merge_tree_value(store, dir, basename, maybe_base, maybe_side1, maybe_side2)?;
            new_tree.set_or_remove(basename, new_value);
        }
    }
    Ok(store.write_tree(dir, new_tree)?)
}

/// Returns `Some(TreeId)` if this is a directory or missing. If it's missing,
/// we treat it as an empty tree.
fn maybe_tree_id<'id>(
    value: Option<&'id TreeValue>,
    empty_tree_id: &'id TreeId,
) -> Option<&'id TreeId> {
    match value {
        Some(TreeValue::Tree(id)) => Some(id),
        None => Some(empty_tree_id),
        _ => None,
    }
}

fn merge_tree_value(
    store: &Arc<Store>,
    dir: &RepoPath,
    basename: &RepoPathComponent,
    maybe_base: Option<&TreeValue>,
    maybe_side1: Option<&TreeValue>,
    maybe_side2: Option<&TreeValue>,
) -> Result<Option<TreeValue>, TreeMergeError> {
    // Resolve non-trivial conflicts:
    //   * resolve tree conflicts by recursing
    //   * try to resolve file conflicts by merging the file contents
    //   * leave other conflicts (e.g. file/dir conflicts, remove/modify conflicts)
    //     unresolved

    let empty_tree_id = store.empty_tree_id();
    let base_tree_id = maybe_tree_id(maybe_base, empty_tree_id);
    let side1_tree_id = maybe_tree_id(maybe_side1, empty_tree_id);
    let side2_tree_id = maybe_tree_id(maybe_side2, empty_tree_id);
    Ok(match (base_tree_id, side1_tree_id, side2_tree_id) {
        (Some(base_id), Some(side1_id), Some(side2_id)) => {
            let subdir = dir.join(basename);
            let base_tree = store.get_tree(&subdir, base_id)?;
            let side1_tree = store.get_tree(&subdir, side1_id)?;
            let side2_tree = store.get_tree(&subdir, side2_id)?;
            let merged_tree = merge_trees(&side1_tree, &base_tree, &side2_tree)?;
            if merged_tree.id() == empty_tree_id {
                None
            } else {
                Some(TreeValue::Tree(merged_tree.id().clone()))
            }
        }
        _ => {
            // Start by creating a Conflict object. Conflicts can cleanly represent a single
            // resolved state, the absence of a state, or a conflicted state.
            let conflict = Conflict::new(
                vec![maybe_base.cloned()],
                vec![maybe_side1.cloned(), maybe_side2.cloned()],
            );
            let filename = dir.join(basename);
            let conflict = simplify_conflict(store, &filename, conflict)?;
            if let Some(value) = conflict.as_resolved() {
                return Ok(value.clone());
            }
            if let Some(tree_value) = try_resolve_file_conflict(store, &filename, &conflict)? {
                Some(tree_value)
            } else {
                let conflict_id = store.write_conflict(&filename, &conflict)?;
                Some(TreeValue::Conflict(conflict_id))
            }
        }
    })
}

pub fn try_resolve_file_conflict(
    store: &Store,
    filename: &RepoPath,
    conflict: &Conflict<Option<TreeValue>>,
) -> Result<Option<TreeValue>, TreeMergeError> {
    // If there are any non-file or any missing parts in the conflict, we can't
    // merge it. We check early so we don't waste time reading file contents if
    // we can't merge them anyway. At the same time we determine whether the
    // resulting file should be executable.
    let Some(file_id_conflict) = conflict.maybe_map(|term| match term {
        Some(TreeValue::File { id, executable: _ }) => Some(id),
        _ => None,
    }) else {
        return Ok(None);
    };
    let Some(executable_conflict) = conflict.maybe_map(|term| match term {
        Some(TreeValue::File { id: _, executable }) => Some(executable),
        _ => None,
    }) else {
        return Ok(None);
    };
    let Some(&&executable) = executable_conflict.resolve_trivial() else {
        // We're unable to determine whether the result should be executable
        return Ok(None);
    };
    if let Some(&resolved_file_id) = file_id_conflict.resolve_trivial() {
        // Don't bother reading the file contents if the conflict can be trivially
        // resolved.
        return Ok(Some(TreeValue::File {
            id: resolved_file_id.clone(),
            executable,
        }));
    }
    let mut removed_contents = vec![];
    let mut added_contents = vec![];
    for &file_id in file_id_conflict.removes() {
        let mut content = vec![];
        store
            .read_file(filename, file_id)?
            .read_to_end(&mut content)
            .map_err(|err| TreeMergeError::ReadError {
                source: err,
                file_id: file_id.clone(),
            })?;
        removed_contents.push(content);
    }
    for &file_id in file_id_conflict.adds() {
        let mut content = vec![];
        store
            .read_file(filename, file_id)?
            .read_to_end(&mut content)
            .map_err(|err| TreeMergeError::ReadError {
                source: err,
                file_id: file_id.clone(),
            })?;
        added_contents.push(content);
    }
    let merge_result = files::merge(
        &removed_contents.iter().map(Vec::as_slice).collect_vec(),
        &added_contents.iter().map(Vec::as_slice).collect_vec(),
    );
    match merge_result {
        MergeResult::Resolved(merged_content) => {
            let id = store.write_file(filename, &mut merged_content.0.as_slice())?;
            Ok(Some(TreeValue::File { id, executable }))
        }
        MergeResult::Conflict(_) => Ok(None),
    }
}

fn simplify_conflict(
    store: &Store,
    path: &RepoPath,
    conflict: Conflict<Option<TreeValue>>,
) -> Result<Conflict<Option<TreeValue>>, BackendError> {
    // Important cases to simplify:
    //
    // D
    // |
    // B C
    // |/
    // A
    //
    // 1. rebase C to B, then back to A => there should be no conflict
    // 2. rebase C to B, then to D => the conflict should not mention B
    // 3. rebase B to C and D to B', then resolve the conflict in B' and rebase D'
    // on top =>    the conflict should be between B'', B, and D; it should not
    // mention the conflict in B'

    // Case 1 above:
    // After first rebase, the conflict is {+B-A+C}. After rebasing back,
    // the unsimplified conflict is {+A-B+{+B-A+C}}. Since the
    // inner conflict is positive, we can simply move it into the outer conflict. We
    // thus get {+A-B+B-A+C}, which we can then simplify to just C (because {+C} ==
    // C).
    //
    // Case 2 above:
    // After first rebase, the conflict is {+B-A+C}. After rebasing to D,
    // the unsimplified conflict is {+D-C+{+B-A+C}}. As in the
    // previous case, the inner conflict can be moved into the outer one. We then
    // get {+D-C+B-A+C}. That can be simplified to
    // {+D+B-A}, which is the desired conflict.
    //
    // Case 3 above:
    // TODO: describe this case

    let expanded = conflict.try_map(|term| match term {
        Some(TreeValue::Conflict(id)) => store.read_conflict(path, id),
        _ => Ok(Conflict::resolved(term.clone())),
    })?;
    Ok(expanded.flatten().simplify())
}
