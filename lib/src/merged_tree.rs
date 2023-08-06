// Copyright 2023 The Jujutsu Authors
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

//! A lazily merged view of a set of trees.

use std::cmp::max;
use std::sync::Arc;
use std::{iter, vec};

use itertools::Itertools;

use crate::backend;
use crate::backend::{ConflictId, TreeValue};
use crate::merge::Merge;
use crate::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use crate::store::Store;
use crate::tree::{try_resolve_file_conflict, Tree, TreeMergeError};
use crate::tree_builder::TreeBuilder;

/// Presents a view of a merged set of trees.
#[derive(Clone, Debug)]
pub enum MergedTree {
    /// A single tree, possibly with path-level conflicts.
    Legacy(Tree),
    /// A merge of multiple trees, or just a single tree. The individual trees
    /// have no path-level conflicts.
    Merge(Merge<Tree>),
}

/// The value at a given path in a `MergedTree`.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub enum MergedTreeValue<'a> {
    /// A single non-conflicted value.
    Resolved(Option<&'a TreeValue>),
    /// TODO: Make this a `Merge<Option<&'a TreeValue>>` (reference to the
    /// value) once we have removed the `MergedTree::Legacy` variant.
    Conflict(Merge<Option<TreeValue>>),
}

impl MergedTree {
    /// Creates a new `MergedTree` representing a single tree without conflicts.
    pub fn resolved(tree: Tree) -> Self {
        MergedTree::new(Merge::resolved(tree))
    }

    /// Creates a new `MergedTree` representing a merge of a set of trees. The
    /// individual trees must not have any conflicts.
    pub fn new(conflict: Merge<Tree>) -> Self {
        debug_assert!(!conflict.removes().iter().any(|t| t.has_conflict()));
        debug_assert!(!conflict.adds().iter().any(|t| t.has_conflict()));
        debug_assert!(itertools::chain(conflict.removes(), conflict.adds())
            .map(|tree| tree.dir())
            .all_equal());
        debug_assert!(itertools::chain(conflict.removes(), conflict.adds())
            .map(|tree| Arc::as_ptr(tree.store()))
            .all_equal());
        MergedTree::Merge(conflict)
    }

    /// Creates a new `MergedTree` backed by a tree with path-level conflicts.
    pub fn legacy(tree: Tree) -> Self {
        MergedTree::Legacy(tree)
    }

    /// Takes a tree in the legacy format (with path-level conflicts in the
    /// tree) and returns a `MergedTree` with any conflicts converted to
    /// tree-level conflicts.
    pub fn from_legacy_tree(tree: Tree) -> Self {
        let conflict_ids = tree.conflicts();
        if conflict_ids.is_empty() {
            return MergedTree::resolved(tree);
        }
        // Find the number of removes in the most complex conflict. We will then
        // build `2*num_removes + 1` trees
        let mut max_num_removes = 0;
        let store = tree.store();
        let mut conflicts: Vec<(&RepoPath, Merge<Option<TreeValue>>)> = vec![];
        for (path, conflict_id) in &conflict_ids {
            let conflict = store.read_conflict(path, conflict_id).unwrap();
            max_num_removes = max(max_num_removes, conflict.removes().len());
            conflicts.push((path, conflict));
        }
        let mut removes = vec![];
        let mut adds = vec![store.tree_builder(tree.id().clone())];
        for _ in 0..max_num_removes {
            removes.push(store.tree_builder(tree.id().clone()));
            adds.push(store.tree_builder(tree.id().clone()));
        }
        for (path, conflict) in conflicts {
            let num_removes = conflict.removes().len();
            // If there are fewer terms in this conflict than in some other conflict, we can
            // add canceling removes and adds of any value. The simplest value is an absent
            // value, so we use that.
            for i in num_removes..max_num_removes {
                removes[i].remove(path.clone());
                adds[i + 1].remove(path.clone());
            }
            // Now add the terms that were present in the conflict to the appropriate trees.
            for (i, term) in conflict.removes().iter().enumerate() {
                match term {
                    Some(value) => removes[i].set(path.clone(), value.clone()),
                    None => removes[i].remove(path.clone()),
                }
            }
            for (i, term) in conflict.adds().iter().enumerate() {
                match term {
                    Some(value) => adds[i].set(path.clone(), value.clone()),
                    None => adds[i].remove(path.clone()),
                }
            }
        }

        let write_tree = |builder: TreeBuilder| {
            let tree_id = builder.write_tree();
            store.get_tree(&RepoPath::root(), &tree_id).unwrap()
        };

        MergedTree::Merge(Merge::new(
            removes.into_iter().map(write_tree).collect(),
            adds.into_iter().map(write_tree).collect(),
        ))
    }

    /// This tree's directory
    pub fn dir(&self) -> &RepoPath {
        match self {
            MergedTree::Legacy(tree) => tree.dir(),
            MergedTree::Merge(conflict) => conflict.adds()[0].dir(),
        }
    }

    /// The `Store` associated with this tree.
    pub fn store(&self) -> &Arc<Store> {
        match self {
            MergedTree::Legacy(tree) => tree.store(),
            MergedTree::Merge(conflict) => conflict.adds()[0].store(),
        }
    }

    /// The value at the given basename. The value can be `Resolved` even if
    /// `self` is a `Conflict`, which happens if the value at the path can be
    /// trivially merged. Does not recurse, so if `basename` refers to a Tree,
    /// then a `TreeValue::Tree` will be returned.
    pub fn value(&self, basename: &RepoPathComponent) -> MergedTreeValue {
        match self {
            MergedTree::Legacy(tree) => match tree.value(basename) {
                Some(TreeValue::Conflict(conflict_id)) => {
                    let conflict = tree.store().read_conflict(tree.dir(), conflict_id).unwrap();
                    MergedTreeValue::Conflict(conflict)
                }
                other => MergedTreeValue::Resolved(other),
            },
            MergedTree::Merge(conflict) => {
                if let Some(tree) = conflict.as_resolved() {
                    return MergedTreeValue::Resolved(tree.value(basename));
                }
                let value = conflict.map(|tree| tree.value(basename));
                if let Some(resolved) = value.resolve_trivial() {
                    return MergedTreeValue::Resolved(*resolved);
                }

                MergedTreeValue::Conflict(value.map(|x| x.cloned()))
            }
        }
    }

    /// Tries to resolve any conflicts, resolving any conflicts that can be
    /// automatically resolved and leaving the rest unresolved. The returned
    /// conflict will either be resolved or have the same number of sides as
    /// the input.
    pub fn resolve(&self) -> Result<Merge<Tree>, TreeMergeError> {
        match self {
            MergedTree::Legacy(tree) => Ok(Merge::resolved(tree.clone())),
            MergedTree::Merge(conflict) => merge_trees(conflict),
        }
    }

    /// An iterator over the conflicts in this tree, including subtrees.
    /// Recurses into subtrees and yields conflicts in those, but only if
    /// all sides are trees, so tree/file conflicts will be reported as a single
    /// conflict, not one for each path in the tree.
    // TODO: Restrict this by a matcher (or add a separate method for that).
    pub fn conflicts(&self) -> impl Iterator<Item = (RepoPath, Merge<Option<TreeValue>>)> {
        ConflictIterator::new(self.clone())
    }

    /// Whether this tree has conflicts.
    pub fn has_conflict(&self) -> bool {
        match self {
            MergedTree::Legacy(tree) => tree.has_conflict(),
            MergedTree::Merge(conflict) => !conflict.is_resolved(),
        }
    }
}

fn all_tree_conflict_names(conflict: &Merge<Tree>) -> impl Iterator<Item = &RepoPathComponent> {
    itertools::chain(conflict.removes(), conflict.adds())
        .map(|tree| tree.data().names())
        .kmerge()
        .dedup()
}

fn merge_trees(merge: &Merge<Tree>) -> Result<Merge<Tree>, TreeMergeError> {
    if let Some(tree) = merge.resolve_trivial() {
        return Ok(Merge::resolved(tree.clone()));
    }

    let base_tree = &merge.adds()[0];
    let store = base_tree.store();
    let dir = base_tree.dir();
    // Keep resolved entries in `new_tree` and conflicted entries in `conflicts` to
    // start with. Then we'll create the full trees later, and only if there are
    // any conflicts.
    let mut new_tree = backend::Tree::default();
    let mut conflicts = vec![];
    for basename in all_tree_conflict_names(merge) {
        let path_merge = merge.map(|tree| tree.value(basename).cloned());
        let path_merge = merge_tree_values(store, dir, path_merge)?;
        match path_merge.into_resolved() {
            Ok(value) => {
                new_tree.set_or_remove(basename, value);
            }
            Err(path_merge) => {
                conflicts.push((basename, path_merge));
            }
        };
    }
    if conflicts.is_empty() {
        let new_tree_id = store.write_tree(dir, new_tree)?;
        Ok(Merge::resolved(new_tree_id))
    } else {
        // For each side of the conflict, overwrite the entries in `new_tree` with the
        // values from  `conflicts`. Entries that are not in `conflicts` will remain
        // unchanged and will be reused for each side.
        let mut tree_removes = vec![];
        for i in 0..merge.removes().len() {
            for (basename, path_conflict) in &conflicts {
                new_tree.set_or_remove(basename, path_conflict.removes()[i].clone());
            }
            let tree = store.write_tree(dir, new_tree.clone())?;
            tree_removes.push(tree);
        }
        let mut tree_adds = vec![];
        for i in 0..merge.adds().len() {
            for (basename, path_conflict) in &conflicts {
                new_tree.set_or_remove(basename, path_conflict.adds()[i].clone());
            }
            let tree = store.write_tree(dir, new_tree.clone())?;
            tree_adds.push(tree);
        }

        Ok(Merge::new(tree_removes, tree_adds))
    }
}

/// Tries to resolve a conflict between tree values. Returns
/// Ok(Conflict::normal(value)) if the conflict was resolved, and
/// Ok(Conflict::absent()) if the path should be removed. Returns the
/// conflict unmodified if it cannot be resolved automatically.
fn merge_tree_values(
    store: &Arc<Store>,
    path: &RepoPath,
    conflict: Merge<Option<TreeValue>>,
) -> Result<Merge<Option<TreeValue>>, TreeMergeError> {
    if let Some(resolved) = conflict.resolve_trivial() {
        return Ok(Merge::resolved(resolved.clone()));
    }

    if let Some(tree_conflict) = conflict.to_tree_merge(store, path)? {
        // If all sides are trees or missing, merge the trees recursively, treating
        // missing trees as empty.
        let merged_tree = merge_trees(&tree_conflict)?;
        if merged_tree.as_resolved().map(|tree| tree.id()) == Some(store.empty_tree_id()) {
            Ok(Merge::absent())
        } else {
            Ok(merged_tree.map(|tree| Some(TreeValue::Tree(tree.id().clone()))))
        }
    } else {
        // Try to resolve file conflicts by merging the file contents. Treats missing
        // files as empty.
        if let Some(resolved) = try_resolve_file_conflict(store, path, &conflict)? {
            Ok(Merge::normal(resolved))
        } else {
            // Failed to merge the files, or the paths are not files
            Ok(conflict)
        }
    }
}

struct ConflictEntriesNonRecursiveIterator<'a> {
    merged_tree: &'a MergedTree,
    basename_iter: Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a>,
}

impl<'a> ConflictEntriesNonRecursiveIterator<'a> {
    fn new(merged_tree: &'a MergedTree) -> Self {
        let basename_iter: Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> = match merged_tree
        {
            MergedTree::Legacy(tree) => Box::new(
                tree.entries_non_recursive()
                    .filter(|entry| matches!(entry.value(), &TreeValue::Conflict(_)))
                    .map(|entry| entry.name()),
            ),
            MergedTree::Merge(conflict) => {
                if conflict.is_resolved() {
                    Box::new(iter::empty())
                } else {
                    Box::new(all_tree_conflict_names(conflict))
                }
            }
        };
        ConflictEntriesNonRecursiveIterator {
            merged_tree,
            basename_iter,
        }
    }
}

impl<'a> Iterator for ConflictEntriesNonRecursiveIterator<'a> {
    type Item = (&'a RepoPathComponent, Merge<Option<TreeValue>>);

    fn next(&mut self) -> Option<Self::Item> {
        for basename in self.basename_iter.by_ref() {
            match self.merged_tree.value(basename) {
                MergedTreeValue::Resolved(_) => {}
                MergedTreeValue::Conflict(conflict) => {
                    return Some((basename, conflict));
                }
            }
        }
        None
    }
}

/// The state for the non-recursive iteration over the conflicted entries in a
/// single directory.
struct ConflictsDirItem {
    entry_iterator: ConflictEntriesNonRecursiveIterator<'static>,
    // On drop, tree must outlive entry_iterator
    tree: Box<MergedTree>,
}

impl ConflictsDirItem {
    fn new(tree: MergedTree) -> Self {
        // Put the tree in a box so it doesn't move if `ConflictsDirItem` moves.
        let tree = Box::new(tree);
        let entry_iterator = ConflictEntriesNonRecursiveIterator::new(&tree);
        let entry_iterator: ConflictEntriesNonRecursiveIterator<'static> =
            unsafe { std::mem::transmute(entry_iterator) };
        Self {
            entry_iterator,
            tree,
        }
    }
}

enum ConflictIterator {
    Legacy {
        store: Arc<Store>,
        conflicts_iter: vec::IntoIter<(RepoPath, ConflictId)>,
    },
    Merge {
        stack: Vec<ConflictsDirItem>,
    },
}

impl ConflictIterator {
    fn new(tree: MergedTree) -> Self {
        match tree {
            MergedTree::Legacy(tree) => ConflictIterator::Legacy {
                store: tree.store().clone(),
                conflicts_iter: tree.conflicts().into_iter(),
            },
            MergedTree::Merge(_) => ConflictIterator::Merge {
                stack: vec![ConflictsDirItem::new(tree)],
            },
        }
    }
}

impl Iterator for ConflictIterator {
    type Item = (RepoPath, Merge<Option<TreeValue>>);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            ConflictIterator::Legacy {
                store,
                conflicts_iter,
            } => {
                if let Some((path, conflict_id)) = conflicts_iter.next() {
                    // TODO: propagate errors
                    let conflict = store.read_conflict(&path, &conflict_id).unwrap();
                    Some((path, conflict))
                } else {
                    None
                }
            }
            ConflictIterator::Merge { stack } => {
                while let Some(top) = stack.last_mut() {
                    if let Some((basename, conflict)) = top.entry_iterator.next() {
                        let path = top.tree.dir().join(basename);
                        // TODO: propagate errors
                        if let Some(tree_conflict) =
                            conflict.to_tree_merge(top.tree.store(), &path).unwrap()
                        {
                            // If all sides are trees or missing, descend into the merged tree
                            stack.push(ConflictsDirItem::new(MergedTree::Merge(tree_conflict)));
                        } else {
                            // Otherwise this is a conflict between files, trees, etc. If they could
                            // be automatically resolved, they should have been when the top-level
                            // tree conflict was written, so we assume that they can't be.
                            return Some((path, conflict));
                        }
                    } else {
                        stack.pop();
                    }
                }
                None
            }
        }
    }
}
