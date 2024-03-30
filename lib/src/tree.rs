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
use tracing::instrument;

use crate::backend::{
    BackendError, BackendResult, ConflictId, TreeEntriesNonRecursiveIterator, TreeEntry, TreeId,
    TreeValue,
};
use crate::files::MergeResult;
use crate::matchers::{EverythingMatcher, Matcher};
use crate::merge::{trivial_merge, Merge, MergedTreeValue};
use crate::object_id::ObjectId;
use crate::repo_path::{RepoPath, RepoPathBuf, RepoPathComponent, RepoPathComponentsIter};
use crate::store::Store;
use crate::{backend, files};

#[derive(Clone)]
pub struct Tree {
    store: Arc<Store>,
    dir: RepoPathBuf,
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

impl Tree {
    pub fn new(store: Arc<Store>, dir: RepoPathBuf, id: TreeId, data: Arc<backend::Tree>) -> Self {
        Tree {
            store,
            dir,
            id,
            data,
        }
    }

    pub fn null(store: Arc<Store>, dir: RepoPathBuf) -> Self {
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
        assert_eq!(self.dir(), RepoPath::root());
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

    fn sub_tree_recursive(&self, mut components: RepoPathComponentsIter) -> Option<Tree> {
        if let Some(first) = components.next() {
            components.try_fold(self.sub_tree(first)?, |tree, name| tree.sub_tree(name))
        } else {
            // TODO: It would be nice to be able to return a reference here, but
            // then we would have to figure out how to share Tree instances
            // across threads.
            Some(self.clone())
        }
    }

    pub fn conflicts_matching(&self, matcher: &dyn Matcher) -> Vec<(RepoPathBuf, ConflictId)> {
        let mut conflicts = vec![];
        for (name, value) in self.entries_matching(matcher) {
            if let TreeValue::Conflict(id) = value {
                conflicts.push((name.clone(), id.clone()));
            }
        }
        conflicts
    }

    #[instrument]
    pub fn conflicts(&self) -> Vec<(RepoPathBuf, ConflictId)> {
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
    tree: Tree,
    entries: Vec<(RepoPathBuf, TreeValue)>,
}

impl From<Tree> for TreeEntriesDirItem {
    fn from(tree: Tree) -> Self {
        let mut entries = tree
            .entries_non_recursive()
            .map(|entry| (tree.dir().join(entry.name()), entry.value().clone()))
            .collect_vec();
        entries.reverse();
        Self { tree, entries }
    }
}

impl<'matcher> TreeEntriesIterator<'matcher> {
    fn new(tree: Tree, matcher: &'matcher dyn Matcher) -> Self {
        // TODO: Restrict walk according to Matcher::visit()
        Self {
            stack: vec![TreeEntriesDirItem::from(tree)],
            matcher,
        }
    }
}

impl Iterator for TreeEntriesIterator<'_> {
    type Item = (RepoPathBuf, TreeValue);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            if let Some((path, value)) = top.entries.pop() {
                match value {
                    TreeValue::Tree(id) => {
                        // TODO: Handle the other cases (specific files and trees)
                        if self.matcher.visit(&path).is_nothing() {
                            continue;
                        }
                        let subtree = top.tree.known_sub_tree(&path, &id);
                        self.stack.push(TreeEntriesDirItem::from(subtree));
                    }
                    value => {
                        if self.matcher.matches(&path) {
                            return Some((path, value));
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

pub fn merge_trees(side1_tree: &Tree, base_tree: &Tree, side2_tree: &Tree) -> BackendResult<Tree> {
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
    store.write_tree(dir, new_tree)
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
) -> BackendResult<Option<TreeValue>> {
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
            // Start by creating a Merge object. Merges can cleanly represent a single
            // resolved state, the absence of a state, or a conflicted state.
            let conflict = Merge::from_vec(vec![
                maybe_side1.cloned(),
                maybe_base.cloned(),
                maybe_side2.cloned(),
            ]);
            let filename = dir.join(basename);
            let expanded = conflict.try_map(|term| match term {
                Some(TreeValue::Conflict(id)) => store.read_conflict(&filename, id),
                _ => Ok(Merge::resolved(term.clone())),
            })?;
            let merge = expanded.flatten().simplify();
            match merge.into_resolved() {
                Ok(value) => value,
                Err(conflict) => {
                    if let Some(tree_value) =
                        try_resolve_file_conflict(store, &filename, &conflict)?
                    {
                        Some(tree_value)
                    } else {
                        let conflict_id = store.write_conflict(&filename, &conflict)?;
                        Some(TreeValue::Conflict(conflict_id))
                    }
                }
            }
        }
    })
}

/// Resolves file-level conflict by merging content hunks.
///
/// The input `conflict` is supposed to be simplified. It shouldn't contain
/// non-file values that cancel each other.
pub fn try_resolve_file_conflict(
    store: &Store,
    filename: &RepoPath,
    conflict: &MergedTreeValue,
) -> BackendResult<Option<TreeValue>> {
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

    // While the input conflict should be simplified by caller, it might contain
    // terms which only differ in executable bits. Simplify the conflict further
    // for two reasons:
    // 1. Avoid reading unchanged file contents
    // 2. The simplified conflict can sometimes be resolved when the unsimplfied one
    //    cannot
    let file_id_conflict = file_id_conflict.simplify();

    let contents: Merge<Vec<u8>> =
        file_id_conflict.try_map(|&file_id| -> BackendResult<Vec<u8>> {
            let mut content = vec![];
            store
                .read_file(filename, file_id)?
                .read_to_end(&mut content)
                .map_err(|err| BackendError::ReadObject {
                    object_type: file_id.object_type(),
                    hash: file_id.hex(),
                    source: err.into(),
                })?;
            Ok(content)
        })?;
    let slices = contents.map(|content| content.as_slice());
    let merge_result = files::merge(&slices);
    match merge_result {
        MergeResult::Resolved(merged_content) => {
            let id = store.write_file(filename, &mut merged_content.0.as_slice())?;
            Ok(Some(TreeValue::File { id, executable }))
        }
        MergeResult::Conflict(_) => Ok(None),
    }
}
