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
use std::collections::BTreeMap;
use std::iter::zip;
use std::sync::Arc;
use std::{iter, vec};

use futures::executor::block_on;
use futures::stream::StreamExt;
use itertools::Itertools;

use crate::backend::{BackendError, BackendResult, ConflictId, MergedTreeId, TreeId, TreeValue};
use crate::matchers::{EverythingMatcher, Matcher};
use crate::merge::{Merge, MergeBuilder};
use crate::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use crate::store::Store;
use crate::tree::{try_resolve_file_conflict, Tree, TreeMergeError};
use crate::tree_builder::TreeBuilder;
use crate::{backend, tree};

/// Presents a view of a merged set of trees.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum MergedTree {
    /// A single tree, possibly with path-level conflicts.
    Legacy(Tree),
    /// A merge of multiple trees, or just a single tree. The individual trees
    /// have no path-level conflicts.
    Merge(Merge<Tree>),
}

/// The value at a given path in a `MergedTree`.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub enum MergedTreeVal<'a> {
    /// A single non-conflicted value.
    Resolved(Option<&'a TreeValue>),
    /// TODO: Make this a `Merge<Option<&'a TreeValue>>` (reference to the
    /// value) once we have removed the `MergedTree::Legacy` variant.
    Conflict(Merge<Option<TreeValue>>),
}

impl MergedTreeVal<'_> {
    fn to_merge(&self) -> Merge<Option<TreeValue>> {
        match self {
            MergedTreeVal::Resolved(value) => Merge::resolved(value.cloned()),
            MergedTreeVal::Conflict(merge) => merge.clone(),
        }
    }
}

/// Summary of the changes between two trees.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DiffSummary {
    /// Modified files
    pub modified: Vec<RepoPath>,
    /// Added files
    pub added: Vec<RepoPath>,
    /// Removed files
    pub removed: Vec<RepoPath>,
}

impl MergedTree {
    /// Creates a new `MergedTree` representing a single tree without conflicts.
    pub fn resolved(tree: Tree) -> Self {
        MergedTree::new(Merge::resolved(tree))
    }

    /// Creates a new `MergedTree` representing a merge of a set of trees. The
    /// individual trees must not have any conflicts.
    pub fn new(trees: Merge<Tree>) -> Self {
        debug_assert!(!trees.removes().iter().any(|t| t.has_conflict()));
        debug_assert!(!trees.adds().iter().any(|t| t.has_conflict()));
        debug_assert!(itertools::chain(trees.removes(), trees.adds())
            .map(|tree| tree.dir())
            .all_equal());
        debug_assert!(itertools::chain(trees.removes(), trees.adds())
            .map(|tree| Arc::as_ptr(tree.store()))
            .all_equal());
        MergedTree::Merge(trees)
    }

    /// Creates a new `MergedTree` backed by a tree with path-level conflicts.
    pub fn legacy(tree: Tree) -> Self {
        MergedTree::Legacy(tree)
    }

    /// Takes a tree in the legacy format (with path-level conflicts in the
    /// tree) and returns a `MergedTree` with any conflicts converted to
    /// tree-level conflicts.
    pub fn from_legacy_tree(tree: Tree) -> BackendResult<Self> {
        let conflict_ids = tree.conflicts();
        if conflict_ids.is_empty() {
            return Ok(MergedTree::resolved(tree));
        }
        // Find the number of removes in the most complex conflict. We will then
        // build `2*num_removes + 1` trees
        let mut max_num_removes = 0;
        let store = tree.store();
        let mut conflicts: Vec<(&RepoPath, Merge<Option<TreeValue>>)> = vec![];
        for (path, conflict_id) in &conflict_ids {
            let conflict = store.read_conflict(path, conflict_id)?;
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
                removes[i].set_or_remove(path.clone(), term.clone());
            }
            for (i, term) in conflict.adds().iter().enumerate() {
                adds[i].set_or_remove(path.clone(), term.clone());
            }
        }

        let write_tree = |builder: TreeBuilder| {
            let tree_id = builder.write_tree();
            store.get_tree(&RepoPath::root(), &tree_id)
        };

        let removed_trees = removes.into_iter().map(write_tree).try_collect()?;
        let added_trees = adds.into_iter().map(write_tree).try_collect()?;
        Ok(MergedTree::Merge(Merge::new(removed_trees, added_trees)))
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
            MergedTree::Merge(trees) => trees.adds()[0].store(),
        }
    }

    /// Base names of entries in this directory.
    pub fn names<'a>(&'a self) -> Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> {
        match self {
            MergedTree::Legacy(tree) => Box::new(tree.data().names()),
            MergedTree::Merge(conflict) => Box::new(all_tree_conflict_names(conflict)),
        }
    }

    /// The value at the given basename. The value can be `Resolved` even if
    /// `self` is a `Merge`, which happens if the value at the path can be
    /// trivially merged. Does not recurse, so if `basename` refers to a Tree,
    /// then a `TreeValue::Tree` will be returned.
    pub fn value(&self, basename: &RepoPathComponent) -> MergedTreeVal {
        match self {
            MergedTree::Legacy(tree) => match tree.value(basename) {
                Some(TreeValue::Conflict(conflict_id)) => {
                    let path = tree.dir().join(basename);
                    let conflict = tree.store().read_conflict(&path, conflict_id).unwrap();
                    MergedTreeVal::Conflict(conflict)
                }
                other => MergedTreeVal::Resolved(other),
            },
            MergedTree::Merge(trees) => {
                if let Some(tree) = trees.as_resolved() {
                    return MergedTreeVal::Resolved(tree.value(basename));
                }
                let value = trees.map(|tree| tree.value(basename));
                if let Some(resolved) = value.resolve_trivial() {
                    return MergedTreeVal::Resolved(*resolved);
                }

                MergedTreeVal::Conflict(value.map(|x| x.cloned()))
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
            MergedTree::Merge(trees) => merge_trees(trees),
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
            MergedTree::Merge(trees) => !trees.is_resolved(),
        }
    }

    /// Gets the `MergeTree` in a subdirectory of the current tree. If the path
    /// doesn't correspond to a tree in any of the inputs to the merge, then
    /// that entry will be replace by an empty tree in the result.
    pub fn sub_tree(&self, name: &RepoPathComponent) -> Option<MergedTree> {
        if let MergedTree::Legacy(tree) = self {
            tree.sub_tree(name).map(MergedTree::Legacy)
        } else {
            match self.value(name) {
                MergedTreeVal::Resolved(Some(TreeValue::Tree(sub_tree_id))) => {
                    let subdir = self.dir().join(name);
                    Some(MergedTree::resolved(
                        self.store().get_tree(&subdir, sub_tree_id).unwrap(),
                    ))
                }
                MergedTreeVal::Resolved(_) => None,
                MergedTreeVal::Conflict(merge) => {
                    let merged_trees = merge.map(|value| match value {
                        Some(TreeValue::Tree(sub_tree_id)) => {
                            let subdir = self.dir().join(name);
                            self.store().get_tree(&subdir, sub_tree_id).unwrap()
                        }
                        _ => {
                            let subdir = self.dir().join(name);
                            Tree::null(self.store().clone(), subdir.clone())
                        }
                    });
                    Some(MergedTree::Merge(merged_trees))
                }
            }
        }
    }

    /// The value at the given path. The value can be `Resolved` even if
    /// `self` is a `Conflict`, which happens if the value at the path can be
    /// trivially merged.
    pub fn path_value(&self, path: &RepoPath) -> Merge<Option<TreeValue>> {
        assert_eq!(self.dir(), &RepoPath::root());
        match path.split() {
            Some((dir, basename)) => match self.sub_tree_recursive(dir.components()) {
                None => Merge::absent(),
                Some(tree) => tree.value(basename).to_merge(),
            },
            None => match self {
                MergedTree::Legacy(tree) => Merge::normal(TreeValue::Tree(tree.id().clone())),
                MergedTree::Merge(trees) => {
                    trees.map(|tree| Some(TreeValue::Tree(tree.id().clone())))
                }
            },
        }
    }

    /// The tree's id
    pub fn id(&self) -> MergedTreeId {
        match self {
            MergedTree::Legacy(tree) => MergedTreeId::Legacy(tree.id().clone()),
            MergedTree::Merge(merge) => MergedTreeId::Merge(merge.map(|tree| tree.id().clone())),
        }
    }

    fn sub_tree_recursive(&self, components: &[RepoPathComponent]) -> Option<MergedTree> {
        if let Some((first, tail)) = components.split_first() {
            tail.iter()
                .try_fold(self.sub_tree(first)?, |tree, name| tree.sub_tree(name))
        } else {
            Some(self.clone())
        }
    }

    fn entries_non_recursive(&self) -> TreeEntriesNonRecursiveIterator {
        TreeEntriesNonRecursiveIterator::new(self)
    }

    /// Iterator over the entries matching the given matcher. Subtrees are
    /// visited recursively. Subtrees that differ between the current
    /// `MergedTree`'s terms are merged on the fly. Missing terms are treated as
    /// empty directories. Subtrees that conflict with non-trees are not
    /// visited. For example, if current tree is a merge of 3 trees, and the
    /// entry for 'foo' is a conflict between a change subtree and a symlink
    /// (i.e. the subdirectory was replaced by symlink in one side of the
    /// conflict), then the entry for `foo` itself will be emitted, but no
    /// entries from inside `foo/` from either of the trees will be.
    pub fn entries(&self) -> TreeEntriesIterator<'static> {
        TreeEntriesIterator::new(self.clone(), &EverythingMatcher)
    }

    /// Like `entries()` but restricted by a matcher.
    pub fn entries_matching<'matcher>(
        &self,
        matcher: &'matcher dyn Matcher,
    ) -> TreeEntriesIterator<'matcher> {
        TreeEntriesIterator::new(self.clone(), matcher)
    }

    /// Iterate over the differences between this tree and another tree.
    ///
    /// The files in a removed tree will be returned before a file that replaces
    /// it.
    pub fn diff<'matcher>(
        &self,
        other: &MergedTree,
        matcher: &'matcher dyn Matcher,
    ) -> TreeDiffIterator<'matcher> {
        TreeDiffIterator::new(self.clone(), other.clone(), matcher)
    }

    /// Collects lists of modified, added, and removed files between this tree
    /// and another tree.
    pub fn diff_summary(&self, other: &MergedTree, matcher: &dyn Matcher) -> DiffSummary {
        let mut modified = vec![];
        let mut added = vec![];
        let mut removed = vec![];
        for (file, before, after) in self.diff(other, matcher) {
            if before.is_absent() {
                added.push(file);
            } else if after.is_absent() {
                removed.push(file);
            } else {
                modified.push(file);
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

    /// Merges this tree with `other`, using `base` as base.
    pub fn merge(
        &self,
        base: &MergedTree,
        other: &MergedTree,
    ) -> Result<MergedTree, TreeMergeError> {
        if let (MergedTree::Legacy(this), MergedTree::Legacy(base), MergedTree::Legacy(other)) =
            (self, base, other)
        {
            let merged_tree = tree::merge_trees(this, base, other)?;
            Ok(MergedTree::legacy(merged_tree))
        } else {
            // Convert legacy trees to merged trees and unwrap to `Merge<Tree>`
            let to_merge = |tree: &MergedTree| -> Result<Merge<Tree>, TreeMergeError> {
                match tree {
                    MergedTree::Legacy(tree) => {
                        let MergedTree::Merge(tree) = MergedTree::from_legacy_tree(tree.clone())?
                        else {
                            unreachable!();
                        };
                        Ok(tree)
                    }
                    MergedTree::Merge(conflict) => Ok(conflict.clone()),
                }
            };
            let nested = Merge::new(
                vec![to_merge(base)?],
                vec![to_merge(self)?, to_merge(other)?],
            );
            let tree = merge_trees(&nested.flatten().simplify())?;
            // If the result can be resolved, then `merge_trees()` above would have returned
            // a resolved merge. However, that function will always preserve the arity of
            // conflicts it cannot resolve. So we simplify the conflict again
            // here to possibly reduce a complex conflict to a simpler one.
            let tree = tree.simplify();
            // If debug assertions are enabled, check that the merge was idempotent. In
            // particular,  that this last simplification doesn't enable further automatic
            // resolutions
            if cfg!(debug_assertions) {
                let re_merged = merge_trees(&tree).unwrap();
                debug_assert_eq!(re_merged, tree);
            }
            Ok(MergedTree::Merge(tree))
        }
    }
}

fn all_tree_conflict_names(trees: &Merge<Tree>) -> impl Iterator<Item = &RepoPathComponent> {
    itertools::chain(trees.removes(), trees.adds())
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
        let path = dir.join(basename);
        let path_merge = merge_tree_values(store, &path, path_merge)?;
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
/// Ok(Merge::normal(value)) if the conflict was resolved, and
/// Ok(Merge::absent()) if the path should be removed. Returns the
/// conflict unmodified if it cannot be resolved automatically.
fn merge_tree_values(
    store: &Arc<Store>,
    path: &RepoPath,
    values: Merge<Option<TreeValue>>,
) -> Result<Merge<Option<TreeValue>>, TreeMergeError> {
    if let Some(resolved) = values.resolve_trivial() {
        return Ok(Merge::resolved(resolved.clone()));
    }

    if let Some(trees) = values.to_tree_merge(store, path)? {
        // If all sides are trees or missing, merge the trees recursively, treating
        // missing trees as empty.
        let merged_tree = merge_trees(&trees)?;
        if merged_tree.as_resolved().map(|tree| tree.id()) == Some(store.empty_tree_id()) {
            Ok(Merge::absent())
        } else {
            Ok(merged_tree.map(|tree| Some(TreeValue::Tree(tree.id().clone()))))
        }
    } else {
        // Try to resolve file conflicts by merging the file contents. Treats missing
        // files as empty.
        if let Some(resolved) = try_resolve_file_conflict(store, path, &values)? {
            Ok(Merge::normal(resolved))
        } else {
            // Failed to merge the files, or the paths are not files
            Ok(values)
        }
    }
}

struct TreeEntriesNonRecursiveIterator<'a> {
    merged_tree: &'a MergedTree,
    basename_iter: Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a>,
}

impl<'a> TreeEntriesNonRecursiveIterator<'a> {
    fn new(merged_tree: &'a MergedTree) -> Self {
        TreeEntriesNonRecursiveIterator {
            merged_tree,
            basename_iter: merged_tree.names(),
        }
    }
}

impl<'a> Iterator for TreeEntriesNonRecursiveIterator<'a> {
    type Item = (&'a RepoPathComponent, MergedTreeVal<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.basename_iter.next().map(|basename| {
            let value = self.merged_tree.value(basename);
            (basename, value)
        })
    }
}

/// Recursive iterator over the entries in a tree.
pub struct TreeEntriesIterator<'matcher> {
    stack: Vec<TreeEntriesDirItem>,
    matcher: &'matcher dyn Matcher,
}

struct TreeEntriesDirItem {
    entry_iterator: TreeEntriesNonRecursiveIterator<'static>,
    // On drop, tree must outlive entry_iterator
    tree: Box<MergedTree>,
}

impl TreeEntriesDirItem {
    fn new(tree: MergedTree) -> Self {
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
    fn new(tree: MergedTree, matcher: &'matcher dyn Matcher) -> Self {
        // TODO: Restrict walk according to Matcher::visit()
        Self {
            stack: vec![TreeEntriesDirItem::new(tree)],
            matcher,
        }
    }
}

impl Iterator for TreeEntriesIterator<'_> {
    type Item = (RepoPath, Merge<Option<TreeValue>>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            if let Some((name, value)) = top.entry_iterator.next() {
                let path = top.tree.dir().join(name);
                let value = value.to_merge();
                if value.is_tree() {
                    // TODO: Handle the other cases (specific files and trees)
                    if self.matcher.visit(&path).is_nothing() {
                        continue;
                    }
                    let tree_merge = value
                        .to_tree_merge(top.tree.store(), &path)
                        .unwrap()
                        .unwrap();
                    let merged_tree = MergedTree::Merge(tree_merge);
                    self.stack.push(TreeEntriesDirItem::new(merged_tree));
                } else if self.matcher.matches(&path) {
                    return Some((path, value));
                }
            } else {
                self.stack.pop();
            }
        }
        None
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
            MergedTree::Merge(trees) => {
                if trees.is_resolved() {
                    Box::new(iter::empty())
                } else {
                    Box::new(all_tree_conflict_names(trees))
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
                MergedTreeVal::Resolved(_) => {}
                MergedTreeVal::Conflict(tree_values) => {
                    return Some((basename, tree_values));
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
                    if let Some((basename, tree_values)) = top.entry_iterator.next() {
                        let path = top.tree.dir().join(basename);
                        // TODO: propagate errors
                        if let Some(trees) =
                            tree_values.to_tree_merge(top.tree.store(), &path).unwrap()
                        {
                            // If all sides are trees or missing, descend into the merged tree
                            stack.push(ConflictsDirItem::new(MergedTree::Merge(trees)));
                        } else {
                            // Otherwise this is a conflict between files, trees, etc. If they could
                            // be automatically resolved, they should have been when the top-level
                            // tree conflict was written, so we assume that they can't be.
                            return Some((path, tree_values));
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

struct TreeEntryDiffIterator<'a> {
    before: &'a MergedTree,
    after: &'a MergedTree,
    basename_iter: Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a>,
}

impl<'a> TreeEntryDiffIterator<'a> {
    fn new(before: &'a MergedTree, after: &'a MergedTree) -> Self {
        fn merge_iters<'a>(
            iter1: impl Iterator<Item = &'a RepoPathComponent> + 'a,
            iter2: impl Iterator<Item = &'a RepoPathComponent> + 'a,
        ) -> Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> {
            Box::new(iter1.merge(iter2).dedup())
        }
        let basename_iter: Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> =
            match (before, after) {
                (MergedTree::Legacy(before), MergedTree::Legacy(after)) => {
                    merge_iters(before.data().names(), after.data().names())
                }
                (MergedTree::Merge(before), MergedTree::Legacy(after)) => {
                    merge_iters(all_tree_conflict_names(before), after.data().names())
                }
                (MergedTree::Legacy(before), MergedTree::Merge(after)) => {
                    merge_iters(before.data().names(), all_tree_conflict_names(after))
                }
                (MergedTree::Merge(before), MergedTree::Merge(after)) => merge_iters(
                    all_tree_conflict_names(before),
                    all_tree_conflict_names(after),
                ),
            };
        TreeEntryDiffIterator {
            before,
            after,
            basename_iter,
        }
    }
}

impl<'a> Iterator for TreeEntryDiffIterator<'a> {
    type Item = (&'a RepoPathComponent, MergedTreeVal<'a>, MergedTreeVal<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        for basename in self.basename_iter.by_ref() {
            let value_before = self.before.value(basename);
            let value_after = self.after.value(basename);
            if value_after != value_before {
                return Some((basename, value_before, value_after));
            }
        }
        None
    }
}

/// Iterator over the differences between two trees.
pub struct TreeDiffIterator<'matcher> {
    stack: Vec<TreeDiffItem>,
    matcher: &'matcher dyn Matcher,
}

struct TreeDiffDirItem {
    path: RepoPath,
    // Iterator over the diffs between tree1 and tree2
    entry_iterator: TreeEntryDiffIterator<'static>,
    // On drop, tree1 and tree2 must outlive entry_iterator
    tree1: Box<MergedTree>,
    tree2: Box<MergedTree>,
}

enum TreeDiffItem {
    Dir(TreeDiffDirItem),
    // This is used for making sure that when a directory gets replaced by a file, we
    // yield the value for the addition of the file after we yield the values
    // for removing files in the directory.
    File(RepoPath, Merge<Option<TreeValue>>, Merge<Option<TreeValue>>),
}

impl<'matcher> TreeDiffIterator<'matcher> {
    fn new(tree1: MergedTree, tree2: MergedTree, matcher: &'matcher dyn Matcher) -> Self {
        let root_dir = RepoPath::root();
        let mut stack = Vec::new();
        if !matcher.visit(&root_dir).is_nothing() {
            stack.push(TreeDiffItem::Dir(TreeDiffDirItem::new(
                root_dir, tree1, tree2,
            )));
        };
        Self { stack, matcher }
    }

    async fn single_tree(store: &Arc<Store>, dir: &RepoPath, value: Option<&TreeValue>) -> Tree {
        match value {
            Some(TreeValue::Tree(tree_id)) => store.get_tree_async(dir, tree_id).await.unwrap(),
            _ => Tree::null(store.clone(), dir.clone()),
        }
    }

    /// Gets the given tree if `value` is a tree, otherwise an empty tree.
    async fn tree(
        tree: &MergedTree,
        dir: &RepoPath,
        values: &Merge<Option<TreeValue>>,
    ) -> MergedTree {
        let trees = if values.is_tree() {
            let builder: MergeBuilder<Tree> = futures::stream::iter(values.iter())
                .then(|value| Self::single_tree(tree.store(), dir, value.as_ref()))
                .collect()
                .await;
            builder.build()
        } else {
            Merge::resolved(Tree::null(tree.store().clone(), dir.clone()))
        };
        // Maintain the type of tree, so we resolve `TreeValue::Conflict` as necessary
        // in the subtree
        match tree {
            MergedTree::Legacy(_) => MergedTree::Legacy(trees.into_resolved().unwrap()),
            MergedTree::Merge(_) => MergedTree::Merge(trees),
        }
    }
}

impl TreeDiffDirItem {
    fn new(path: RepoPath, tree1: MergedTree, tree2: MergedTree) -> Self {
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
}

impl Iterator for TreeDiffIterator<'_> {
    type Item = (RepoPath, Merge<Option<TreeValue>>, Merge<Option<TreeValue>>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            let (dir, (name, before, after)) = match top {
                TreeDiffItem::Dir(dir) => {
                    if let Some((name, before, after)) = dir.entry_iterator.next() {
                        (dir, (name, before.to_merge(), after.to_merge()))
                    } else {
                        self.stack.pop().unwrap();
                        continue;
                    }
                }
                TreeDiffItem::File(..) => {
                    if let TreeDiffItem::File(name, before, after) = self.stack.pop().unwrap() {
                        return Some((name, before, after));
                    } else {
                        unreachable!();
                    }
                }
            };

            let path = dir.path.join(name);
            let tree_before = before.is_tree();
            let tree_after = after.is_tree();
            let post_subdir =
                if (tree_before || tree_after) && !self.matcher.visit(&path).is_nothing() {
                    let (before_tree, after_tree) = block_on(async {
                        let before_tree = Self::tree(dir.tree1.as_ref(), &path, &before);
                        let after_tree = Self::tree(dir.tree2.as_ref(), &path, &after);
                        futures::join!(before_tree, after_tree)
                    });
                    let subdir = TreeDiffDirItem::new(path.clone(), before_tree, after_tree);
                    self.stack.push(TreeDiffItem::Dir(subdir));
                    self.stack.len() - 1
                } else {
                    self.stack.len()
                };
            if self.matcher.matches(&path) {
                if !tree_before && tree_after {
                    if before.is_present() {
                        return Some((path, before, Merge::absent()));
                    }
                } else if tree_before && !tree_after {
                    if after.is_present() {
                        self.stack.insert(
                            post_subdir,
                            TreeDiffItem::File(path, Merge::absent(), after),
                        );
                    }
                } else if !tree_before && !tree_after {
                    return Some((path, before, after));
                }
            }
        }
        None
    }
}

/// Helps with writing trees with conflicts. You start by creating an instance
/// of this type with one or more base trees. You then add overrides on top. The
/// overrides may be conflicts. Then you can write the result as a legacy tree
/// (allowing path-level conflicts) or as multiple conflict-free trees.
pub struct MergedTreeBuilder {
    base_tree_id: MergedTreeId,
    overrides: BTreeMap<RepoPath, Merge<Option<TreeValue>>>,
}

impl MergedTreeBuilder {
    /// Create a new builder with the given trees as base.
    pub fn new(base_tree_id: MergedTreeId) -> Self {
        MergedTreeBuilder {
            base_tree_id,
            overrides: BTreeMap::new(),
        }
    }

    /// Set an override compared to  the base tree. The `values` merge must
    /// either be resolved (i.e. have 1 side) or have the same number of
    /// sides as the `base_tree_ids` used to construct this builder. Use
    /// `Merge::absent()` to remove a value from the tree. When the base tree is
    /// a legacy tree, conflicts can be written either as a multi-way `Merge`
    /// value or as a resolved `Merge` value using `TreeValue::Conflict`.
    pub fn set_or_remove(&mut self, path: RepoPath, values: Merge<Option<TreeValue>>) {
        if let MergedTreeId::Merge(_) = &self.base_tree_id {
            assert!(!values
                .iter()
                .flatten()
                .any(|value| matches!(value, TreeValue::Conflict(_))));
        }
        self.overrides.insert(path, values);
    }

    /// Create new tree(s) from the base tree(s) and overrides.
    ///
    /// When the base tree was a legacy tree and the
    /// `format.tree-level-conflicts` config is disabled, then the result will
    /// be another legacy tree. Overrides with conflicts will result in
    /// conflict objects being written to the store. If
    /// `format.tree-tree-level-conflicts` is enabled, then a legacy tree will
    /// still be written and immediately converted and returned as a merged
    /// tree.
    pub fn write_tree(self, store: &Arc<Store>) -> BackendResult<MergedTreeId> {
        match self.base_tree_id.clone() {
            MergedTreeId::Legacy(base_tree_id) => {
                let mut tree_builder = TreeBuilder::new(store.clone(), base_tree_id);
                for (path, values) in self.overrides {
                    let values = values.simplify();
                    match values.into_resolved() {
                        Ok(value) => {
                            tree_builder.set_or_remove(path, value);
                        }
                        Err(values) => {
                            let conflict_id = store.write_conflict(&path, &values)?;
                            tree_builder.set(path, TreeValue::Conflict(conflict_id));
                        }
                    }
                }
                let legacy_id = tree_builder.write_tree();
                if store.use_tree_conflict_format() {
                    let legacy_tree = store.get_tree(&RepoPath::root(), &legacy_id)?;
                    let merged_tree = MergedTree::from_legacy_tree(legacy_tree)?;
                    Ok(merged_tree.id())
                } else {
                    Ok(MergedTreeId::Legacy(legacy_id))
                }
            }
            MergedTreeId::Merge(base_tree_ids) => {
                let new_tree_ids = self.write_merged_trees(base_tree_ids, store)?;
                Ok(MergedTreeId::Merge(new_tree_ids.simplify()))
            }
        }
    }

    fn write_merged_trees(
        self,
        mut base_tree_ids: Merge<TreeId>,
        store: &Arc<Store>,
    ) -> Result<Merge<TreeId>, BackendError> {
        let num_sides = self
            .overrides
            .values()
            .map(|value| value.num_sides())
            .max()
            .unwrap_or(0);
        base_tree_ids.pad_to(num_sides, store.empty_tree_id());
        // Create a single-tree builder for each base tree
        let mut tree_builders =
            base_tree_ids.map(|base_tree_id| TreeBuilder::new(store.clone(), base_tree_id.clone()));
        for (path, values) in self.overrides {
            match values.into_resolved() {
                Ok(value) => {
                    // This path was overridden with a resolved value. Apply that to all
                    // builders.
                    for builder in tree_builders.iter_mut() {
                        builder.set_or_remove(path.clone(), value.clone());
                    }
                }
                Err(mut values) => {
                    values.pad_to(num_sides, &None);
                    // This path was overridden with a conflicted value. Apply each term to
                    // its corresponding builder.
                    for (builder, value) in zip(tree_builders.iter_mut(), values) {
                        builder.set_or_remove(path.clone(), value);
                    }
                }
            }
        }
        // TODO: This can be made more efficient. If there's a single resolved conflict
        // in `dir/file`, we shouldn't have to write the `dir/` and root trees more than
        // once.
        let merge_builder: MergeBuilder<TreeId> = tree_builders
            .into_iter()
            .map(|builder| builder.write_tree())
            .collect();
        Ok(merge_builder.build())
    }
}
