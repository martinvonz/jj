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

use std::cmp::{max, Ordering};
use std::collections::{BTreeMap, VecDeque};
use std::iter::zip;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::{iter, vec};

use futures::stream::StreamExt;
use futures::{Future, Stream, TryStreamExt};
use itertools::Itertools;

use crate::backend::{BackendError, BackendResult, ConflictId, MergedTreeId, TreeId, TreeValue};
use crate::matchers::{EverythingMatcher, Matcher};
use crate::merge::{Merge, MergeBuilder, MergedTreeValue};
use crate::repo_path::{RepoPath, RepoPathBuf, RepoPathComponent, RepoPathComponentsIter};
use crate::store::Store;
use crate::tree::{try_resolve_file_conflict, Tree};
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
    Conflict(MergedTreeValue),
}

impl MergedTreeVal<'_> {
    fn to_merge(&self) -> MergedTreeValue {
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
    pub modified: Vec<RepoPathBuf>,
    /// Added files
    pub added: Vec<RepoPathBuf>,
    /// Removed files
    pub removed: Vec<RepoPathBuf>,
}

impl MergedTree {
    /// Creates a new `MergedTree` representing a single tree without conflicts.
    pub fn resolved(tree: Tree) -> Self {
        MergedTree::new(Merge::resolved(tree))
    }

    /// Creates a new `MergedTree` representing a merge of a set of trees. The
    /// individual trees must not have any conflicts.
    pub fn new(trees: Merge<Tree>) -> Self {
        debug_assert!(!trees.iter().any(|t| t.has_conflict()));
        debug_assert!(trees.iter().map(|tree| tree.dir()).all_equal());
        debug_assert!(trees
            .iter()
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

        // Find the number of removes and adds in the most complex conflict.
        let mut max_tree_count = 1;
        let store = tree.store();
        let mut conflicts: Vec<(&RepoPath, MergedTreeValue)> = vec![];
        for (path, conflict_id) in &conflict_ids {
            let conflict = store.read_conflict(path, conflict_id)?;
            max_tree_count = max(max_tree_count, conflict.iter().len());
            conflicts.push((path, conflict));
        }
        let mut tree_builders = Vec::new();
        tree_builders.resize_with(max_tree_count, || store.tree_builder(tree.id().clone()));
        for (path, conflict) in conflicts {
            // If there are fewer terms in this conflict than in some other conflict, we can
            // add canceling removes and adds of any value. The simplest value is an absent
            // value, so we use that.
            let terms_padded = conflict.into_iter().chain(iter::repeat(None));
            for (builder, term) in zip(&mut tree_builders, terms_padded) {
                builder.set_or_remove(path.to_owned(), term);
            }
        }

        let new_trees: Vec<_> = tree_builders
            .into_iter()
            .map(|builder| {
                let tree_id = builder.write_tree();
                store.get_tree(RepoPath::root(), &tree_id)
            })
            .try_collect()?;
        Ok(MergedTree::Merge(Merge::from_vec(new_trees)))
    }

    /// This tree's directory
    pub fn dir(&self) -> &RepoPath {
        match self {
            MergedTree::Legacy(tree) => tree.dir(),
            MergedTree::Merge(conflict) => conflict.first().dir(),
        }
    }

    /// The `Store` associated with this tree.
    pub fn store(&self) -> &Arc<Store> {
        match self {
            MergedTree::Legacy(tree) => tree.store(),
            MergedTree::Merge(trees) => trees.first().store(),
        }
    }

    /// Base names of entries in this directory.
    pub fn names<'a>(&'a self) -> Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> {
        match self {
            MergedTree::Legacy(tree) => Box::new(tree.data().names()),
            MergedTree::Merge(conflict) => Box::new(all_tree_basenames(conflict)),
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
            MergedTree::Merge(trees) => trees_value(trees, basename),
        }
    }

    /// Tries to resolve any conflicts, resolving any conflicts that can be
    /// automatically resolved and leaving the rest unresolved. The returned
    /// conflict will either be resolved or have the same number of sides as
    /// the input.
    pub fn resolve(&self) -> BackendResult<Merge<Tree>> {
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
    pub fn conflicts(&self) -> impl Iterator<Item = (RepoPathBuf, MergedTreeValue)> {
        ConflictIterator::new(self)
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
    pub fn path_value(&self, path: &RepoPath) -> MergedTreeValue {
        assert_eq!(self.dir(), RepoPath::root());
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

    fn sub_tree_recursive(&self, mut components: RepoPathComponentsIter) -> Option<MergedTree> {
        if let Some(first) = components.next() {
            components.try_fold(self.sub_tree(first)?, |tree, name| tree.sub_tree(name))
        } else {
            Some(self.clone())
        }
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

    /// Stream of the differences between this tree and another tree.
    pub fn diff_stream<'matcher>(
        &self,
        other: &MergedTree,
        matcher: &'matcher dyn Matcher,
    ) -> TreeDiffStream<'matcher> {
        let concurrency = self.store().concurrency();
        if concurrency <= 1 {
            Box::pin(futures::stream::iter(TreeDiffIterator::new(
                self.clone(),
                other.clone(),
                matcher,
            )))
        } else {
            Box::pin(TreeDiffStreamImpl::new(
                self.clone(),
                other.clone(),
                matcher,
                concurrency,
            ))
        }
    }

    /// Collects lists of modified, added, and removed files between this tree
    /// and another tree.
    pub fn diff_summary(
        &self,
        other: &MergedTree,
        matcher: &dyn Matcher,
    ) -> BackendResult<DiffSummary> {
        let mut modified = vec![];
        let mut added = vec![];
        let mut removed = vec![];
        for (file, diff) in self.diff(other, matcher) {
            let (before, after) = diff?;
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
        Ok(DiffSummary {
            modified,
            added,
            removed,
        })
    }

    /// Merges this tree with `other`, using `base` as base.
    pub fn merge(&self, base: &MergedTree, other: &MergedTree) -> BackendResult<MergedTree> {
        if let (MergedTree::Legacy(this), MergedTree::Legacy(base), MergedTree::Legacy(other)) =
            (self, base, other)
        {
            let merged_tree = tree::merge_trees(this, base, other)?;
            Ok(MergedTree::legacy(merged_tree))
        } else {
            // Convert legacy trees to merged trees and unwrap to `Merge<Tree>`
            let to_merge = |tree: &MergedTree| -> BackendResult<Merge<Tree>> {
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
            let nested = Merge::from_vec(vec![to_merge(self)?, to_merge(base)?, to_merge(other)?]);
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

/// Type alias for the result from `MergedTree::diff_stream()`. We use a
/// `Stream` instead of an `Iterator` so high-latency backends (e.g. cloud-based
/// ones) can fetch trees asynchronously.
pub type TreeDiffStream<'matcher> = Pin<
    Box<
        dyn Stream<
                Item = (
                    RepoPathBuf,
                    BackendResult<(MergedTreeValue, MergedTreeValue)>,
                ),
            > + 'matcher,
    >,
>;

fn all_tree_basenames(trees: &Merge<Tree>) -> impl Iterator<Item = &RepoPathComponent> {
    trees
        .iter()
        .map(|tree| tree.data().names())
        .kmerge()
        .dedup()
}

fn merged_tree_basenames<'a>(
    tree1: &'a MergedTree,
    tree2: &'a MergedTree,
) -> Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> {
    fn merge_iters<'a>(
        iter1: impl Iterator<Item = &'a RepoPathComponent> + 'a,
        iter2: impl Iterator<Item = &'a RepoPathComponent> + 'a,
    ) -> Box<dyn Iterator<Item = &'a RepoPathComponent> + 'a> {
        Box::new(iter1.merge(iter2).dedup())
    }
    match (&tree1, &tree2) {
        (MergedTree::Legacy(before), MergedTree::Legacy(after)) => {
            merge_iters(before.data().names(), after.data().names())
        }
        (MergedTree::Merge(before), MergedTree::Legacy(after)) => {
            merge_iters(all_tree_basenames(before), after.data().names())
        }
        (MergedTree::Legacy(before), MergedTree::Merge(after)) => {
            merge_iters(before.data().names(), all_tree_basenames(after))
        }
        (MergedTree::Merge(before), MergedTree::Merge(after)) => {
            merge_iters(all_tree_basenames(before), all_tree_basenames(after))
        }
    }
}

fn merged_tree_entry_diff<'a>(
    before: &'a MergedTree,
    after: &'a MergedTree,
) -> impl Iterator<Item = (&'a RepoPathComponent, MergedTreeVal<'a>, MergedTreeVal<'a>)> {
    merged_tree_basenames(before, after).filter_map(|basename| {
        let value_before = before.value(basename);
        let value_after = after.value(basename);
        (value_after != value_before).then_some((basename, value_before, value_after))
    })
}

fn trees_value<'a>(trees: &'a Merge<Tree>, basename: &RepoPathComponent) -> MergedTreeVal<'a> {
    if let Some(tree) = trees.as_resolved() {
        return MergedTreeVal::Resolved(tree.value(basename));
    }
    let value = trees.map(|tree| tree.value(basename));
    if let Some(resolved) = value.resolve_trivial() {
        return MergedTreeVal::Resolved(*resolved);
    }
    MergedTreeVal::Conflict(value.map(|x| x.cloned()))
}

fn merge_trees(merge: &Merge<Tree>) -> BackendResult<Merge<Tree>> {
    if let Some(tree) = merge.resolve_trivial() {
        return Ok(Merge::resolved(tree.clone()));
    }

    let base_tree = merge.first();
    let store = base_tree.store();
    let dir = base_tree.dir();
    // Keep resolved entries in `new_tree` and conflicted entries in `conflicts` to
    // start with. Then we'll create the full trees later, and only if there are
    // any conflicts.
    let mut new_tree = backend::Tree::default();
    let mut conflicts = vec![];
    for basename in all_tree_basenames(merge) {
        let path_merge = merge.map(|tree| tree.value(basename).cloned());
        let path = dir.join(basename);
        let path_merge = merge_tree_values(store, &path, path_merge)?;
        match path_merge.into_resolved() {
            Ok(value) => {
                new_tree.set_or_remove(basename, value);
            }
            Err(path_merge) => {
                conflicts.push((basename, path_merge.into_iter()));
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
        let tree_count = merge.iter().len();
        let mut new_trees = Vec::with_capacity(tree_count);
        for _ in 0..tree_count {
            for (basename, path_conflict) in &mut conflicts {
                new_tree.set_or_remove(basename, path_conflict.next().unwrap());
            }
            let tree = store.write_tree(dir, new_tree.clone())?;
            new_trees.push(tree);
        }
        Ok(Merge::from_vec(new_trees))
    }
}

/// Tries to resolve a conflict between tree values. Returns
/// Ok(Merge::normal(value)) if the conflict was resolved, and
/// Ok(Merge::absent()) if the path should be removed. Returns the
/// conflict unmodified if it cannot be resolved automatically.
fn merge_tree_values(
    store: &Arc<Store>,
    path: &RepoPath,
    values: MergedTreeValue,
) -> BackendResult<MergedTreeValue> {
    if let Some(resolved) = values.resolve_trivial() {
        return Ok(Merge::resolved(resolved.clone()));
    }

    if let Some(trees) = values.to_tree_merge(store, path)? {
        // If all sides are trees or missing, merge the trees recursively, treating
        // missing trees as empty.
        let empty_tree_id = store.empty_tree_id();
        let merged_tree = merge_trees(&trees)?;
        Ok(merged_tree
            .map(|tree| (tree.id() != empty_tree_id).then(|| TreeValue::Tree(tree.id().clone()))))
    } else {
        // Try to resolve file conflicts by merging the file contents. Treats missing
        // files as empty. The values may contain trees canceling each other (notably
        // padded absent trees), so we need to simplify them first.
        let simplified = values.clone().simplify();
        // No fast path for simplified.is_resolved(). If it could be resolved, it would
        // have been caught by values.resolve_trivial() above.
        if let Some(resolved) = try_resolve_file_conflict(store, path, &simplified)? {
            Ok(Merge::normal(resolved))
        } else {
            // Failed to merge the files, or the paths are not files
            Ok(values)
        }
    }
}

/// Recursive iterator over the entries in a tree.
pub struct TreeEntriesIterator<'matcher> {
    stack: Vec<TreeEntriesDirItem>,
    matcher: &'matcher dyn Matcher,
}

struct TreeEntriesDirItem {
    tree: MergedTree,
    entries: Vec<(RepoPathBuf, MergedTreeValue)>,
}

impl TreeEntriesDirItem {
    fn new(tree: MergedTree, matcher: &dyn Matcher) -> Self {
        let mut entries = vec![];
        let dir = tree.dir();
        for name in tree.names() {
            let path = dir.join(name);
            let value = tree.value(name).to_merge();
            if value.is_tree() {
                // TODO: Handle the other cases (specific files and trees)
                if matcher.visit(&path).is_nothing() {
                    continue;
                }
            } else if !matcher.matches(&path) {
                continue;
            }
            entries.push((path, value));
        }
        entries.reverse();
        TreeEntriesDirItem { tree, entries }
    }
}

impl<'matcher> TreeEntriesIterator<'matcher> {
    fn new(tree: MergedTree, matcher: &'matcher dyn Matcher) -> Self {
        Self {
            stack: vec![TreeEntriesDirItem::new(tree, matcher)],
            matcher,
        }
    }
}

impl Iterator for TreeEntriesIterator<'_> {
    type Item = (RepoPathBuf, MergedTreeValue);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            if let Some((path, value)) = top.entries.pop() {
                if value.is_tree() {
                    let tree_merge = value
                        .to_tree_merge(top.tree.store(), &path)
                        .unwrap()
                        .unwrap();
                    let merged_tree = MergedTree::Merge(tree_merge);
                    self.stack
                        .push(TreeEntriesDirItem::new(merged_tree, self.matcher));
                } else {
                    return Some((path, value));
                }
            } else {
                self.stack.pop();
            }
        }
        None
    }
}

/// The state for the non-recursive iteration over the conflicted entries in a
/// single directory.
struct ConflictsDirItem {
    entries: Vec<(RepoPathBuf, MergedTreeValue)>,
}

impl From<&Merge<Tree>> for ConflictsDirItem {
    fn from(trees: &Merge<Tree>) -> Self {
        let dir = trees.first().dir();
        if trees.is_resolved() {
            return ConflictsDirItem { entries: vec![] };
        }

        let mut entries = vec![];
        for basename in all_tree_basenames(trees) {
            match trees_value(trees, basename) {
                MergedTreeVal::Resolved(_) => {}
                MergedTreeVal::Conflict(tree_values) => {
                    entries.push((dir.join(basename), tree_values));
                }
            }
        }
        entries.reverse();
        ConflictsDirItem { entries }
    }
}

enum ConflictIterator {
    Legacy {
        store: Arc<Store>,
        conflicts_iter: vec::IntoIter<(RepoPathBuf, ConflictId)>,
    },
    Merge {
        store: Arc<Store>,
        stack: Vec<ConflictsDirItem>,
    },
}

impl ConflictIterator {
    fn new(tree: &MergedTree) -> Self {
        match tree {
            MergedTree::Legacy(tree) => ConflictIterator::Legacy {
                store: tree.store().clone(),
                conflicts_iter: tree.conflicts().into_iter(),
            },
            MergedTree::Merge(trees) => ConflictIterator::Merge {
                store: tree.store().clone(),
                stack: vec![ConflictsDirItem::from(trees)],
            },
        }
    }
}

impl Iterator for ConflictIterator {
    type Item = (RepoPathBuf, MergedTreeValue);

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
            ConflictIterator::Merge { store, stack } => {
                while let Some(top) = stack.last_mut() {
                    if let Some((path, tree_values)) = top.entries.pop() {
                        // TODO: propagate errors
                        if let Some(trees) = tree_values.to_tree_merge(store, &path).unwrap() {
                            // If all sides are trees or missing, descend into the merged tree
                            stack.push(ConflictsDirItem::from(&trees));
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

/// Iterator over the differences between two trees.
pub struct TreeDiffIterator<'matcher> {
    stack: Vec<TreeDiffItem>,
    matcher: &'matcher dyn Matcher,
}

struct TreeDiffDirItem {
    tree1: MergedTree,
    tree2: MergedTree,
    entries: Vec<(RepoPathBuf, MergedTreeValue, MergedTreeValue)>,
}

enum TreeDiffItem {
    Dir(TreeDiffDirItem),
    // This is used for making sure that when a directory gets replaced by a file, we
    // yield the value for the addition of the file after we yield the values
    // for removing files in the directory.
    File(RepoPathBuf, MergedTreeValue, MergedTreeValue),
}

impl<'matcher> TreeDiffIterator<'matcher> {
    /// Creates a iterator over the differences between two trees. Generally
    /// prefer `MergedTree::diff()` of calling this directly.
    pub fn new(tree1: MergedTree, tree2: MergedTree, matcher: &'matcher dyn Matcher) -> Self {
        let root_dir = RepoPath::root();
        let mut stack = Vec::new();
        if !matcher.visit(root_dir).is_nothing() {
            stack.push(TreeDiffItem::Dir(TreeDiffDirItem::from_trees(
                root_dir, tree1, tree2, matcher,
            )));
        };
        Self { stack, matcher }
    }

    fn single_tree(
        store: &Arc<Store>,
        dir: &RepoPath,
        value: Option<&TreeValue>,
    ) -> BackendResult<Tree> {
        match value {
            Some(TreeValue::Tree(tree_id)) => store.get_tree(dir, tree_id),
            _ => Ok(Tree::null(store.clone(), dir.to_owned())),
        }
    }

    /// Gets the given tree if `value` is a tree, otherwise an empty tree.
    fn tree(
        tree: &MergedTree,
        dir: &RepoPath,
        values: &MergedTreeValue,
    ) -> BackendResult<MergedTree> {
        let trees = if values.is_tree() {
            values.try_map(|value| Self::single_tree(tree.store(), dir, value.as_ref()))?
        } else {
            Merge::resolved(Tree::null(tree.store().clone(), dir.to_owned()))
        };
        // Maintain the type of tree, so we resolve `TreeValue::Conflict` as necessary
        // in the subtree
        match tree {
            MergedTree::Legacy(_) => Ok(MergedTree::Legacy(trees.into_resolved().unwrap())),
            MergedTree::Merge(_) => Ok(MergedTree::Merge(trees)),
        }
    }
}

impl TreeDiffDirItem {
    fn from_trees(
        dir: &RepoPath,
        tree1: MergedTree,
        tree2: MergedTree,
        matcher: &dyn Matcher,
    ) -> Self {
        let mut entries = vec![];
        for (name, before, after) in merged_tree_entry_diff(&tree1, &tree2) {
            let path = dir.join(name);
            let before = before.to_merge();
            let after = after.to_merge();
            let tree_before = before.is_tree();
            let tree_after = after.is_tree();
            // Check if trees and files match, but only if either side is a tree or a file
            // (don't query the matcher unnecessarily).
            let tree_matches = (tree_before || tree_after) && !matcher.visit(&path).is_nothing();
            let file_matches = (!tree_before || !tree_after) && matcher.matches(&path);

            // Replace trees or files that don't match by `Merge::absent()`
            let before = if (tree_before && tree_matches) || (!tree_before && file_matches) {
                before
            } else {
                Merge::absent()
            };
            let after = if (tree_after && tree_matches) || (!tree_after && file_matches) {
                after
            } else {
                Merge::absent()
            };
            if before.is_absent() && after.is_absent() {
                continue;
            }
            entries.push((path, before, after));
        }
        entries.reverse();
        Self {
            tree1,
            tree2,
            entries,
        }
    }
}

impl Iterator for TreeDiffIterator<'_> {
    type Item = (
        RepoPathBuf,
        BackendResult<(MergedTreeValue, MergedTreeValue)>,
    );

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(top) = self.stack.last_mut() {
            let (dir, (path, before, after)) = match top {
                TreeDiffItem::Dir(dir) => match dir.entries.pop() {
                    Some(entry) => (dir, entry),
                    None => {
                        self.stack.pop().unwrap();
                        continue;
                    }
                },
                TreeDiffItem::File(..) => {
                    if let TreeDiffItem::File(path, before, after) = self.stack.pop().unwrap() {
                        return Some((path, Ok((before, after))));
                    } else {
                        unreachable!();
                    }
                }
            };

            let tree_before = before.is_tree();
            let tree_after = after.is_tree();
            let post_subdir = if tree_before || tree_after {
                let before_tree = match Self::tree(&dir.tree1, &path, &before) {
                    Ok(tree) => tree,
                    Err(err) => return Some((path, Err(err))),
                };
                let after_tree = match Self::tree(&dir.tree2, &path, &after) {
                    Ok(tree) => tree,
                    Err(err) => return Some((path, Err(err))),
                };
                let subdir =
                    TreeDiffDirItem::from_trees(&path, before_tree, after_tree, self.matcher);
                self.stack.push(TreeDiffItem::Dir(subdir));
                self.stack.len() - 1
            } else {
                self.stack.len()
            };
            if !tree_before && tree_after {
                if before.is_present() {
                    return Some((path, Ok((before, Merge::absent()))));
                }
            } else if tree_before && !tree_after {
                if after.is_present() {
                    self.stack.insert(
                        post_subdir,
                        TreeDiffItem::File(path, Merge::absent(), after),
                    );
                }
            } else if !tree_before && !tree_after {
                return Some((path, Ok((before, after))));
            }
        }
        None
    }
}

/// Stream of differences between two trees.
pub struct TreeDiffStreamImpl<'matcher> {
    matcher: &'matcher dyn Matcher,
    legacy_format_before: bool,
    legacy_format_after: bool,
    /// Pairs of tree values that may or may not be ready to emit, sorted in the
    /// order we want to emit them. If either side is a tree, there will be
    /// a corresponding entry in `pending_trees`.
    items: BTreeMap<DiffStreamKey, BackendResult<(MergedTreeValue, MergedTreeValue)>>,
    // TODO: Is it better to combine this and `items` into a single map?
    #[allow(clippy::type_complexity)]
    pending_trees: VecDeque<(
        RepoPathBuf,
        Pin<Box<dyn Future<Output = BackendResult<(MergedTree, MergedTree)>> + 'matcher>>,
    )>,
    /// The maximum number of trees to request concurrently. However, we do the
    /// accounting per path, so for there will often be twice as many pending
    /// `Backend::read_tree()` calls - for the "before" and "after" sides. For
    /// conflicts, there will be even more.
    max_concurrent_reads: usize,
    /// The maximum number of items in `items`. However, we will always add the
    /// full differences from a particular pair of trees, so it may temporarily
    /// go over the limit (until we emit those items). It may also go over the
    /// limit because we have a file item that's blocked by pending subdirectory
    /// items.
    max_queued_items: usize,
}

/// A wrapper around `RepoPath` that allows us to optionally sort files after
/// directories that have the file as a prefix.
#[derive(PartialEq, Eq, Clone, Debug)]
struct DiffStreamKey {
    path: RepoPathBuf,
    file_after_dir: bool,
}

impl DiffStreamKey {
    fn normal(path: RepoPathBuf) -> Self {
        DiffStreamKey {
            path,
            file_after_dir: false,
        }
    }

    fn file_after_dir(path: RepoPathBuf) -> Self {
        DiffStreamKey {
            path,
            file_after_dir: true,
        }
    }
}

impl PartialOrd for DiffStreamKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DiffStreamKey {
    fn cmp(&self, other: &Self) -> Ordering {
        if self == other {
            Ordering::Equal
        } else if self.file_after_dir && other.path.starts_with(&self.path) {
            Ordering::Greater
        } else if other.file_after_dir && self.path.starts_with(&other.path) {
            Ordering::Less
        } else {
            self.path.cmp(&other.path)
        }
    }
}

impl<'matcher> TreeDiffStreamImpl<'matcher> {
    /// Creates a iterator over the differences between two trees. Generally
    /// prefer `MergedTree::diff_stream()` of calling this directly.
    pub fn new(
        tree1: MergedTree,
        tree2: MergedTree,
        matcher: &'matcher dyn Matcher,
        max_concurrent_reads: usize,
    ) -> Self {
        let mut stream = Self {
            matcher,
            legacy_format_before: matches!(tree1, MergedTree::Legacy(_)),
            legacy_format_after: matches!(tree2, MergedTree::Legacy(_)),
            items: BTreeMap::new(),
            pending_trees: VecDeque::new(),
            max_concurrent_reads,
            max_queued_items: 10000,
        };
        stream.add_dir_diff_items(RepoPathBuf::root(), Ok((tree1, tree2)));
        stream
    }

    async fn single_tree(
        store: &Arc<Store>,
        dir: &RepoPath,
        value: Option<&TreeValue>,
    ) -> BackendResult<Tree> {
        match value {
            Some(TreeValue::Tree(tree_id)) => store.get_tree_async(dir, tree_id).await,
            _ => Ok(Tree::null(store.clone(), dir.to_owned())),
        }
    }

    /// Gets the given tree if `value` is a tree, otherwise an empty tree.
    async fn tree(
        store: Arc<Store>,
        legacy_format: bool,
        dir: RepoPathBuf,
        values: MergedTreeValue,
    ) -> BackendResult<MergedTree> {
        let trees = if values.is_tree() {
            let builder: MergeBuilder<Tree> = futures::stream::iter(values.iter())
                .then(|value| Self::single_tree(&store, &dir, value.as_ref()))
                .try_collect()
                .await?;
            builder.build()
        } else {
            Merge::resolved(Tree::null(store, dir.clone()))
        };
        // Maintain the type of tree, so we resolve `TreeValue::Conflict` as necessary
        // in the subtree
        if legacy_format {
            Ok(MergedTree::Legacy(trees.into_resolved().unwrap()))
        } else {
            Ok(MergedTree::Merge(trees))
        }
    }

    fn add_dir_diff_items(
        &mut self,
        dir: RepoPathBuf,
        tree_diff: BackendResult<(MergedTree, MergedTree)>,
    ) {
        let (tree1, tree2) = match tree_diff {
            Ok(trees) => trees,
            Err(err) => {
                self.items.insert(DiffStreamKey::normal(dir), Err(err));
                return;
            }
        };

        for (basename, value_before, value_after) in merged_tree_entry_diff(&tree1, &tree2) {
            let path = dir.join(basename);
            let before = value_before.to_merge();
            let after = value_after.to_merge();
            let tree_before = before.is_tree();
            let tree_after = after.is_tree();
            // Check if trees and files match, but only if either side is a tree or a file
            // (don't query the matcher unnecessarily).
            let tree_matches =
                (tree_before || tree_after) && !self.matcher.visit(&path).is_nothing();
            let file_matches = (!tree_before || !tree_after) && self.matcher.matches(&path);

            // Replace trees or files that don't match by `Merge::absent()`
            let before = if (tree_before && tree_matches) || (!tree_before && file_matches) {
                before
            } else {
                Merge::absent()
            };
            let after = if (tree_after && tree_matches) || (!tree_after && file_matches) {
                after
            } else {
                Merge::absent()
            };
            if before.is_absent() && after.is_absent() {
                continue;
            }

            // If the path was a tree on either side of the diff, read those trees.
            if tree_matches {
                let before_tree_future = Self::tree(
                    tree1.store().clone(),
                    self.legacy_format_before,
                    path.clone(),
                    before.clone(),
                );
                let after_tree_future = Self::tree(
                    tree2.store().clone(),
                    self.legacy_format_after,
                    path.clone(),
                    after.clone(),
                );
                let both_trees_future =
                    async { futures::try_join!(before_tree_future, after_tree_future) };
                self.pending_trees
                    .push_back((path.clone(), Box::pin(both_trees_future)));
            }

            self.items
                .insert(DiffStreamKey::normal(path), Ok((before, after)));
        }
    }

    fn poll_tree_futures(&mut self, cx: &mut Context<'_>) {
        let mut pending_index = 0;
        while pending_index < self.pending_trees.len()
            && (pending_index < self.max_concurrent_reads
                || self.items.len() < self.max_queued_items)
        {
            let (_, future) = &mut self.pending_trees[pending_index];
            if let Poll::Ready(tree_diff) = future.as_mut().poll(cx) {
                let (dir, _) = self.pending_trees.remove(pending_index).unwrap();
                let key = DiffStreamKey::normal(dir);
                // Whenever we add an entry to `self.pending_trees`, we also add an Ok() entry
                // to `self.items`.
                let (before, after) = self.items.remove(&key).unwrap().unwrap();
                // If this was a transition from file to tree or vice versa, add back an item
                // for just the removal/addition of the file.
                if before.is_present() && !before.is_tree() {
                    self.items
                        .insert(key.clone(), Ok((before, Merge::absent())));
                } else if after.is_present() && !after.is_tree() {
                    self.items.insert(
                        DiffStreamKey::file_after_dir(key.path.clone()),
                        Ok((Merge::absent(), after)),
                    );
                }
                self.add_dir_diff_items(key.path, tree_diff);
            } else {
                pending_index += 1;
            }
        }
    }
}

impl Stream for TreeDiffStreamImpl<'_> {
    type Item = (
        RepoPathBuf,
        BackendResult<(MergedTreeValue, MergedTreeValue)>,
    );

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Go through all pending tree futures and poll them.
        self.poll_tree_futures(cx);

        // Now emit the first file, or the first tree that completed with an error
        if let Some(entry) = self.items.first_entry() {
            match entry.get() {
                Err(_) => {
                    // File or tree with error
                    let (key, result) = entry.remove_entry();
                    Poll::Ready(Some((key.path, result)))
                }
                Ok((before, after)) if !before.is_tree() && !after.is_tree() => {
                    // A diff with no trees involved
                    let (key, result) = entry.remove_entry();
                    Poll::Ready(Some((key.path, result)))
                }
                _ => {
                    // The first entry has a tree on at least one side (before or after). We need to
                    // wait for that future to complete.
                    assert!(!self.pending_trees.is_empty());
                    Poll::Pending
                }
            }
        } else {
            Poll::Ready(None)
        }
    }
}

/// Helps with writing trees with conflicts. You start by creating an instance
/// of this type with one or more base trees. You then add overrides on top. The
/// overrides may be conflicts. Then you can write the result as a legacy tree
/// (allowing path-level conflicts) or as multiple conflict-free trees.
pub struct MergedTreeBuilder {
    base_tree_id: MergedTreeId,
    overrides: BTreeMap<RepoPathBuf, MergedTreeValue>,
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
    pub fn set_or_remove(&mut self, path: RepoPathBuf, values: MergedTreeValue) {
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
                    let legacy_tree = store.get_tree(RepoPath::root(), &legacy_id)?;
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
