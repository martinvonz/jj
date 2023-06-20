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

use itertools::Itertools;

use crate::backend::TreeValue;
use crate::conflicts::Conflict;
use crate::repo_path::{RepoPath, RepoPathComponent};
use crate::store::Store;
use crate::tree::Tree;
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
}
