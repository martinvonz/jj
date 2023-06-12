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

use itertools::Itertools;

use crate::backend;
use crate::backend::TreeValue;
use crate::conflicts::Conflict;
use crate::repo_path::{RepoPath, RepoPathComponent};
use crate::store::Store;
use crate::tree::{try_resolve_file_conflict, Tree, TreeMergeError};
use crate::tree_builder::TreeBuilder;

/// Presents a view of a merged set of trees.
pub enum MergedTree {
    /// A single tree, possibly with path-level conflicts.
    Legacy(Tree),
    /// A merge of multiple trees, or just a single tree. The individual trees
    /// have no path-level conflicts.
    Merge(Conflict<Tree>),
}

/// The value at a given path in a `MergedTree`.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub enum MergedTreeValue<'a> {
    /// A single non-conflicted value.
    Resolved(Option<&'a TreeValue>),
    /// TODO: Make this a `Conflict<Option<&'a TreeValue>>` (reference to the
    /// value) once we have removed the `MergedTree::Legacy` variant.
    Conflict(Conflict<Option<TreeValue>>),
}

impl MergedTree {
    /// Creates a new `MergedTree` representing a single tree without conflicts.
    pub fn resolved(tree: Tree) -> Self {
        MergedTree::new(Conflict::resolved(tree))
    }

    /// Creates a new `MergedTree` representing a merge of a set of trees. The
    /// individual trees must not have any conflicts.
    pub fn new(conflict: Conflict<Tree>) -> Self {
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
        let mut conflicts: Vec<(&RepoPath, Conflict<Option<TreeValue>>)> = vec![];
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

        MergedTree::Merge(Conflict::new(
            removes.into_iter().map(write_tree).collect(),
            adds.into_iter().map(write_tree).collect(),
        ))
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
    pub fn resolve(&self) -> Result<Conflict<Tree>, TreeMergeError> {
        match self {
            MergedTree::Legacy(tree) => Ok(Conflict::resolved(tree.clone())),
            MergedTree::Merge(conflict) => merge_trees(conflict),
        }
    }
}

fn merge_trees(conflict: &Conflict<Tree>) -> Result<Conflict<Tree>, TreeMergeError> {
    if let Some(tree) = conflict.resolve_trivial() {
        return Ok(Conflict::resolved(tree.clone()));
    }

    let base_names = itertools::chain(conflict.removes(), conflict.adds())
        .map(|tree| tree.data().names())
        .kmerge()
        .dedup();

    let base_tree = &conflict.adds()[0];
    let store = base_tree.store();
    let dir = base_tree.dir();
    // Keep resolved entries in `new_tree` and conflicted entries in `conflicts` to
    // start with. Then we'll create the full trees later, and only if there are
    // any conflicts.
    let mut new_tree = backend::Tree::default();
    let mut conflicts = vec![];
    for basename in base_names {
        let path_conflict = conflict.map(|tree| tree.value(basename).cloned());
        let path_conflict = merge_tree_values(store, dir, path_conflict)?;
        if let Some(value) = path_conflict.as_resolved() {
            new_tree.set_or_remove(basename, value.clone());
        } else {
            conflicts.push((basename, path_conflict));
        };
    }
    if conflicts.is_empty() {
        let new_tree_id = store.write_tree(dir, new_tree)?;
        Ok(Conflict::resolved(new_tree_id))
    } else {
        // For each side of the conflict, overwrite the entries in `new_tree` with the
        // values from  `conflicts`. Entries that are not in `conflicts` will remain
        // unchanged and will be reused for each side.
        let mut tree_removes = vec![];
        for i in 0..conflict.removes().len() {
            for (basename, path_conflict) in &conflicts {
                new_tree.set_or_remove(basename, path_conflict.removes()[i].clone());
            }
            let tree = store.write_tree(dir, new_tree.clone())?;
            tree_removes.push(tree);
        }
        let mut tree_adds = vec![];
        for i in 0..conflict.adds().len() {
            for (basename, path_conflict) in &conflicts {
                new_tree.set_or_remove(basename, path_conflict.adds()[i].clone());
            }
            let tree = store.write_tree(dir, new_tree.clone())?;
            tree_adds.push(tree);
        }

        Ok(Conflict::new(tree_removes, tree_adds))
    }
}

/// Tries to resolve a conflict between tree values. Returns
/// Ok(Conflict::resolved(Some(value))) if the conflict was resolved, and
/// Ok(Conflict::resolved(None)) if the path should be removed. Returns the
/// conflict unmodified if it cannot be resolved automatically.
fn merge_tree_values(
    store: &Arc<Store>,
    path: &RepoPath,
    conflict: Conflict<Option<TreeValue>>,
) -> Result<Conflict<Option<TreeValue>>, TreeMergeError> {
    if let Some(resolved) = conflict.resolve_trivial() {
        return Ok(Conflict::resolved(resolved.clone()));
    }

    if let Some(tree_conflict) = conflict.to_tree_conflict(store, path)? {
        // If all sides are trees or missing, merge the trees recursively, treating
        // missing trees as empty.
        let merged_tree = merge_trees(&tree_conflict)?;
        if merged_tree.as_resolved().map(|tree| tree.id()) == Some(store.empty_tree_id()) {
            Ok(Conflict::resolved(None))
        } else {
            Ok(merged_tree.map(|tree| Some(TreeValue::Tree(tree.id().clone()))))
        }
    } else {
        // Try to resolve file conflicts by merging the file contents. Treats missing
        // files as empty.
        if let Some(resolved) = try_resolve_file_conflict(store, path, &conflict)? {
            Ok(Conflict::resolved(Some(resolved)))
        } else {
            // Failed to merge the files, or the paths are not files
            Ok(conflict)
        }
    }
}
