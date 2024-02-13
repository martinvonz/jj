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

//! Generic algorithms for working with merged values, plus specializations for
//! some common types of merged values.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::hash::Hash;
use std::io::Write;
use std::iter::zip;
use std::slice;
use std::sync::Arc;

use itertools::Itertools;
use smallvec::{smallvec_inline, SmallVec};

use crate::backend;
use crate::backend::{BackendError, FileId, TreeId, TreeValue};
use crate::content_hash::{ContentHash, DigestUpdate};
use crate::object_id::ObjectId;
use crate::repo_path::RepoPath;
use crate::store::Store;
use crate::tree::Tree;

/// Attempt to resolve trivial conflicts between the inputs. There must be
/// exactly one more adds than removes.
pub fn trivial_merge<'a, T>(removes: &'a [T], adds: &'a [T]) -> Option<&'a T>
where
    T: Eq + Hash,
{
    assert_eq!(
        adds.len(),
        removes.len() + 1,
        "trivial_merge() requires exactly one more adds than removes"
    );
    trivial_merge_inner(
        itertools::interleave(adds, removes),
        adds.len() + removes.len(),
    )
}

fn trivial_merge_inner<T>(mut values: impl Iterator<Item = T>, values_len: usize) -> Option<T>
where
    T: Eq + Hash,
{
    // Optimize the common cases of 3-way merge and 1-way (non-)merge
    if values_len == 1 {
        let add = values.next().unwrap();
        return Some(add);
    } else if values_len == 3 {
        let (add0, remove, add1) = values.next_tuple().unwrap();
        return if add0 == add1 {
            Some(add0)
        } else if add0 == remove {
            Some(add1)
        } else if add1 == remove {
            Some(add0)
        } else {
            None
        };
    }

    // Number of occurrences of each value, with positive indexes counted as +1 and
    // negative as -1, thereby letting positive and negative terms with the same
    // value (i.e. key in the map) cancel each other.
    let mut counts: HashMap<T, i32> = HashMap::new();
    for (value, n) in zip(values, [1, -1].into_iter().cycle()) {
        counts.entry(value).and_modify(|e| *e += n).or_insert(n);
    }

    // Collect non-zero value. Values with a count of 0 means that they have
    // cancelled out.
    counts.retain(|_, count| *count != 0);
    if counts.len() == 1 {
        // If there is a single value with a count of 1 left, then that is the result.
        let (value, count) = counts.into_iter().next().unwrap();
        assert_eq!(count, 1);
        Some(value)
    } else if counts.len() == 2 {
        // All sides made the same change.
        // This matches what Git and Mercurial do (in the 3-way case at least), but not
        // what Darcs and Pijul do. It means that repeated 3-way merging of multiple
        // trees may give different results depending on the order of merging.
        // TODO: Consider removing this special case, making the algorithm more strict,
        // and maybe add a more lenient version that is used when the user explicitly
        // asks for conflict resolution.
        let ((value1, count1), (value2, count2)) = counts.into_iter().next_tuple().unwrap();
        assert_eq!(count1 + count2, 1);
        if count1 > 0 {
            Some(value1)
        } else {
            Some(value2)
        }
    } else {
        None
    }
}

/// A generic representation of merged values.
///
/// There is exactly one more `adds()` than `removes()`. When interpreted as a
/// series of diffs, the merge's (i+1)-st add is matched with the i-th
/// remove. The zeroth add is considered a diff from the non-existent state.
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Merge<T> {
    /// Alternates between positive and negative terms, starting with positive.
    values: SmallVec<[T; 1]>,
}

impl<T: Debug> Debug for Merge<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        // Format like an enum with two variants to make it less verbose in the common
        // case of a resolved state.
        if let Some(value) = self.as_resolved() {
            f.debug_tuple("Resolved").field(value).finish()
        } else {
            f.debug_tuple("Conflicted").field(&self.values).finish()
        }
    }
}

impl<T> Merge<T> {
    /// Creates a `Merge` from the given values, in which positive and negative
    /// terms alternate.
    pub fn from_vec(values: impl Into<SmallVec<[T; 1]>>) -> Self {
        let values = values.into();
        assert!(
            values.len() & 1 != 0,
            "must have one more adds than removes"
        );
        Merge { values }
    }

    /// Creates a new merge object from the given removes and adds.
    pub fn from_removes_adds(
        removes: impl IntoIterator<Item = T>,
        adds: impl IntoIterator<Item = T>,
    ) -> Self {
        let removes = removes.into_iter();
        let mut adds = adds.into_iter();
        let mut values = SmallVec::with_capacity(removes.size_hint().0 * 2 + 1);
        values.push(adds.next().expect("must have at least one add"));
        for diff in removes.zip_longest(adds) {
            let (remove, add) = diff.both().expect("must have one more adds than removes");
            values.extend([remove, add]);
        }
        Merge { values }
    }

    /// Creates a `Merge` with a single resolved value.
    pub fn resolved(value: T) -> Self {
        Merge {
            values: smallvec_inline![value],
        }
    }

    /// Create a `Merge` from a `removes` and `adds`, padding with `None` to
    /// make sure that there is exactly one more `adds` than `removes`.
    pub fn from_legacy_form(
        removes: impl IntoIterator<Item = T>,
        adds: impl IntoIterator<Item = T>,
    ) -> Merge<Option<T>> {
        let removes = removes.into_iter();
        let mut adds = adds.into_iter().fuse();
        let mut values = smallvec_inline![adds.next()];
        for diff in removes.zip_longest(adds) {
            let (remove, add) = diff.map_any(Some, Some).or_default();
            values.extend([remove, add]);
        }
        Merge { values }
    }

    /// The removed values, also called negative terms.
    pub fn removes(&self) -> impl ExactSizeIterator<Item = &T> {
        self.values[1..].iter().step_by(2)
    }

    /// The added values, also called positive terms.
    pub fn adds(&self) -> impl ExactSizeIterator<Item = &T> {
        self.values.iter().step_by(2)
    }

    /// Returns the zeroth added value, which is guaranteed to exist.
    pub fn first(&self) -> &T {
        &self.values[0]
    }

    /// Returns the `index`-th removed value, which is considered belonging to
    /// the `index`-th diff pair.
    pub fn get_remove(&self, index: usize) -> Option<&T> {
        self.values.get(index * 2 + 1)
    }

    /// Returns the `index`-th added value, which is considered belonging to the
    /// `index-1`-th diff pair. The zeroth add is a diff from the non-existent
    /// state.
    pub fn get_add(&self, index: usize) -> Option<&T> {
        self.values.get(index * 2)
    }

    /// Removes the specified "removed"/"added" values. The removed slots are
    /// replaced by the last "removed"/"added" values.
    pub fn swap_remove(&mut self, remove_index: usize, add_index: usize) -> (T, T) {
        // Swap with the last "added" and "removed" values in order.
        let add = self.values.swap_remove(add_index * 2);
        let remove = self.values.swap_remove(remove_index * 2 + 1);
        (remove, add)
    }

    /// The number of positive terms in the conflict.
    pub fn num_sides(&self) -> usize {
        self.values.len() / 2 + 1
    }

    /// Whether this merge is resolved. Does not resolve trivial merges.
    pub fn is_resolved(&self) -> bool {
        self.values.len() == 1
    }

    /// Returns the resolved value, if this merge is resolved. Does not
    /// resolve trivial merges.
    pub fn as_resolved(&self) -> Option<&T> {
        if let [value] = &self.values[..] {
            Some(value)
        } else {
            None
        }
    }

    /// Returns the resolved value, if this merge is resolved. Otherwise returns
    /// the merge itself as an `Err`. Does not resolve trivial merges.
    pub fn into_resolved(mut self) -> Result<T, Merge<T>> {
        if self.values.len() == 1 {
            Ok(self.values.pop().unwrap())
        } else {
            Err(self)
        }
    }

    /// Simplify the merge by joining diffs like A->B and B->C into A->C.
    /// Also drops trivial diffs like A->A.
    pub fn simplify(mut self) -> Self
    where
        T: PartialEq,
    {
        let mut add_index = 0;
        while add_index < self.values.len() {
            let add = &self.values[add_index];
            let mut removes = self.values.iter().enumerate().skip(1).step_by(2);
            if let Some((remove_index, _)) = removes.find(|&(_, remove)| remove == add) {
                // Align the current "add" value to the `remove_index/2`-th diff, then
                // delete the diff pair.
                self.values.swap(remove_index + 1, add_index);
                self.values.drain(remove_index..remove_index + 2);
            } else {
                add_index += 2;
            }
        }
        self
    }

    /// If this merge can be trivially resolved, returns the value it resolves
    /// to.
    pub fn resolve_trivial(&self) -> Option<&T>
    where
        T: Eq + Hash,
    {
        trivial_merge_inner(self.values.iter(), self.values.len())
    }

    /// Pads this merge with to the specified number of sides with the specified
    /// value. No-op if the requested size is not larger than the current size.
    pub fn pad_to(&mut self, num_sides: usize, value: &T)
    where
        T: Clone,
    {
        if num_sides <= self.num_sides() {
            return;
        }
        self.values.resize(num_sides * 2 - 1, value.clone());
    }

    /// Returns an iterator over references to the terms. The items will
    /// alternate between positive and negative terms, starting with
    /// positive (since there's one more of those).
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.values.iter()
    }

    /// A version of `Merge::iter()` that iterates over mutable references.
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> {
        self.values.iter_mut()
    }

    /// Creates a new merge by applying `f` to each remove and add.
    pub fn map<'a, U>(&'a self, f: impl FnMut(&'a T) -> U) -> Merge<U> {
        let values = self.values.iter().map(f).collect();
        Merge { values }
    }

    /// Creates a new merge by applying `f` to each remove and add, returning
    /// `None if `f` returns `None` for any of them.
    pub fn maybe_map<'a, U>(&'a self, f: impl FnMut(&'a T) -> Option<U>) -> Option<Merge<U>> {
        let values = self.values.iter().map(f).collect::<Option<_>>()?;
        Some(Merge { values })
    }

    /// Creates a new merge by applying `f` to each remove and add, returning
    /// `Err if `f` returns `Err` for any of them.
    pub fn try_map<'a, U, E>(
        &'a self,
        f: impl FnMut(&'a T) -> Result<U, E>,
    ) -> Result<Merge<U>, E> {
        let values = self.values.iter().map(f).try_collect()?;
        Ok(Merge { values })
    }
}

/// Helper for consuming items from an iterator and then creating a `Merge`.
///
/// By not collecting directly into `Merge`, we can avoid creating invalid
/// instances of it. If we had `Merge::from_iter()` we would need to allow it to
/// accept iterators of any length (including 0). We couldn't make it panic on
/// even lengths because we can get passed such iterators from e.g.
/// `Option::from_iter()`. By collecting into `MergeBuilder` instead, we move
/// the checking until after `from_iter()` (to `MergeBuilder::build()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeBuilder<T> {
    values: SmallVec<[T; 1]>,
}

impl<T> Default for MergeBuilder<T> {
    fn default() -> Self {
        Self {
            values: Default::default(),
        }
    }
}

impl<T> MergeBuilder<T> {
    /// Requires that exactly one more "adds" than "removes" have been added to
    /// this builder.
    pub fn build(self) -> Merge<T> {
        Merge::from_vec(self.values)
    }
}

impl<T> IntoIterator for Merge<T> {
    type Item = T;
    type IntoIter = smallvec::IntoIter<[T; 1]>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

impl<T> FromIterator<T> for MergeBuilder<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut builder = MergeBuilder::default();
        builder.extend(iter);
        builder
    }
}

impl<T> Extend<T> for MergeBuilder<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.values.extend(iter)
    }
}

impl<T> Merge<Option<T>> {
    /// Creates a resolved merge with a value of `None`.
    pub fn absent() -> Self {
        Self::resolved(None)
    }

    /// Creates a resolved merge with a value of `Some(value)`.
    pub fn normal(value: T) -> Self {
        Self::resolved(Some(value))
    }

    /// Whether this represents a resolved value of `None`.
    pub fn is_absent(&self) -> bool {
        matches!(self.as_resolved(), Some(None))
    }

    /// The opposite of `is_absent()`.
    pub fn is_present(&self) -> bool {
        !self.is_absent()
    }

    /// Returns the value if this is present and non-conflicting.
    pub fn as_normal(&self) -> Option<&T> {
        self.as_resolved()?.as_ref()
    }

    /// Creates lists of `removes` and `adds` from a `Merge` by dropping
    /// `None` values. Note that the conversion is lossy: the order of `None`
    /// values is not preserved when converting back to a `Merge`.
    pub fn into_legacy_form(self) -> (Vec<T>, Vec<T>) {
        // Allocate the maximum size assuming there would be few `None`s.
        let mut removes = Vec::with_capacity(self.values.len() / 2);
        let mut adds = Vec::with_capacity(self.values.len() / 2 + 1);
        let mut values = self.values.into_iter();
        adds.extend(values.next().unwrap());
        while let Some(remove) = values.next() {
            removes.extend(remove);
            adds.extend(values.next().unwrap());
        }
        (removes, adds)
    }
}

impl<T> Merge<Merge<T>> {
    /// Flattens a nested merge into a regular merge.
    ///
    /// Let's say we have a 3-way merge of 3-way merges like this:
    ///
    /// 4 5   7 8
    ///  3     6
    ///    1 2
    ///     0
    ///
    /// Flattening that results in this 9-way merge:
    ///
    /// 4 5 0 7 8
    ///  3 2 1 6
    pub fn flatten(self) -> Merge<T> {
        let mut outer_values = self.values.into_iter();
        let mut result = outer_values.next().unwrap();
        while let Some(mut remove) = outer_values.next() {
            // Add removes reversed, and with the first element moved last, so we preserve
            // the diffs
            remove.values.rotate_left(1);
            for i in 0..remove.values.len() / 2 {
                remove.values.swap(i * 2, i * 2 + 1);
            }
            result.values.extend(remove.values);
            let add = outer_values.next().unwrap();
            result.values.extend(add.values);
        }
        result
    }
}

impl<T: ContentHash> ContentHash for Merge<T> {
    fn hash(&self, state: &mut impl DigestUpdate) {
        self.values.hash(state)
    }
}

/// The value at a given path in a commit. It depends on the context whether it
/// can be absent (`Merge::is_absent()`). For example, when getting the value at
/// a specific path, it may be, but when iterating over entries in a tree, it
/// shouldn't be.
pub type MergedTreeValue = Merge<Option<TreeValue>>;

impl MergedTreeValue {
    /// Create a `Merge` from a `backend::Conflict`, padding with `None` to
    /// make sure that there is exactly one more `adds()` than `removes()`.
    pub fn from_backend_conflict(conflict: backend::Conflict) -> Self {
        let removes = conflict.removes.into_iter().map(|term| term.value);
        let adds = conflict.adds.into_iter().map(|term| term.value);
        Merge::from_legacy_form(removes, adds)
    }

    /// Creates a `backend::Conflict` from a `Merge` by dropping `None`
    /// values. Note that the conversion is lossy: the order of `None` values is
    /// not preserved when converting back to a `Merge`.
    pub fn into_backend_conflict(self) -> backend::Conflict {
        let (removes, adds) = self.into_legacy_form();
        let removes = removes
            .into_iter()
            .map(|value| backend::ConflictTerm { value })
            .collect();
        let adds = adds
            .into_iter()
            .map(|value| backend::ConflictTerm { value })
            .collect();
        backend::Conflict { removes, adds }
    }

    /// Whether this merge should be recursed into when doing directory walks.
    pub fn is_tree(&self) -> bool {
        self.is_present()
            && self
                .iter()
                .all(|value| matches!(value, Some(TreeValue::Tree(_)) | None))
    }

    /// If this merge contains only files or absent entries, returns a merge of
    /// the `FileId`s`. The executable bits will be ignored. Use
    /// `Merge::with_new_file_ids()` to produce a new merge with the original
    /// executable bits preserved.
    pub fn to_file_merge(&self) -> Option<Merge<Option<FileId>>> {
        self.maybe_map(|term| match term {
            None => Some(None),
            Some(TreeValue::File { id, executable: _ }) => Some(Some(id.clone())),
            _ => None,
        })
    }

    /// Creates a new merge with the file ids from the given merge. In other
    /// words, only the executable bits from `self` will be preserved.
    pub fn with_new_file_ids(&self, file_ids: &Merge<Option<FileId>>) -> Self {
        assert_eq!(self.values.len(), file_ids.values.len());
        let values = zip(self.iter(), file_ids.iter())
            .map(|(tree_value, file_id)| {
                if let Some(TreeValue::File { id: _, executable }) = tree_value {
                    Some(TreeValue::File {
                        id: file_id.as_ref().unwrap().clone(),
                        executable: *executable,
                    })
                } else {
                    assert!(tree_value.is_none());
                    assert!(file_id.is_none());
                    None
                }
            })
            .collect();
        Merge { values }
    }

    /// Give a summary description of the conflict's "removes" and "adds"
    pub fn describe(&self, file: &mut dyn Write) -> std::io::Result<()> {
        file.write_all(b"Conflict:\n")?;
        for term in self.removes().flatten() {
            file.write_all(format!("  Removing {}\n", describe_conflict_term(term)).as_bytes())?;
        }
        for term in self.adds().flatten() {
            file.write_all(format!("  Adding {}\n", describe_conflict_term(term)).as_bytes())?;
        }
        Ok(())
    }
}

impl<T> Merge<Option<T>>
where
    T: Borrow<TreeValue>,
{
    /// If every non-`None` term of a `MergedTreeValue`
    /// is a `TreeValue::Tree`, this converts it to
    /// a `Merge<Tree>`, with empty trees instead of
    /// any `None` terms. Otherwise, returns `None`.
    pub fn to_tree_merge(
        &self,
        store: &Arc<Store>,
        dir: &RepoPath,
    ) -> Result<Option<Merge<Tree>>, BackendError> {
        let tree_id_merge = self.maybe_map(|term| match term {
            None => Some(None),
            Some(value) => {
                if let TreeValue::Tree(id) = value.borrow() {
                    Some(Some(id))
                } else {
                    None
                }
            }
        });
        if let Some(tree_id_merge) = tree_id_merge {
            let get_tree = |id: &Option<&TreeId>| -> Result<Tree, BackendError> {
                if let Some(id) = id {
                    store.get_tree(dir, id)
                } else {
                    Ok(Tree::null(store.clone(), dir.to_owned()))
                }
            };
            Ok(Some(tree_id_merge.try_map(get_tree)?))
        } else {
            Ok(None)
        }
    }
}

fn describe_conflict_term(value: &TreeValue) -> String {
    match value {
        TreeValue::File {
            id,
            executable: false,
        } => {
            format!("file with id {}", id.hex())
        }
        TreeValue::File {
            id,
            executable: true,
        } => {
            format!("executable file with id {}", id.hex())
        }
        TreeValue::Symlink(id) => {
            format!("symlink with id {}", id.hex())
        }
        TreeValue::Tree(id) => {
            format!("tree with id {}", id.hex())
        }
        TreeValue::GitSubmodule(id) => {
            format!("Git submodule with id {}", id.hex())
        }
        TreeValue::Conflict(id) => {
            format!("Conflict with id {}", id.hex())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c<T: Clone>(removes: &[T], adds: &[T]) -> Merge<T> {
        Merge::from_removes_adds(removes.to_vec(), adds.to_vec())
    }

    #[test]
    fn test_trivial_merge() {
        assert_eq!(trivial_merge(&[], &[0]), Some(&0));
        assert_eq!(trivial_merge(&[0], &[0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0], &[0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0], &[1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0], &[1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0], &[1, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[0, 0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 0], &[0, 0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[0, 1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[0, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[0, 1, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 0, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 0, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 1, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 0]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 1]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[0, 0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[0, 0, 1]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[0, 0, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[0, 1, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[0, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[0, 1, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 0]), None);
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 1]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[1, 0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[1, 0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[1, 0, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[1, 1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[1, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[1, 1, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 0]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 1]), None);
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 0]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 1]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 0]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 1]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 0]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 1]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 0]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 1]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 4]), None);
    }

    #[test]
    fn test_legacy_form_conversion() {
        fn test_equivalent<T>(legacy_form: (Vec<T>, Vec<T>), merge: Merge<Option<T>>)
        where
            T: Clone + PartialEq + std::fmt::Debug,
        {
            assert_eq!(merge.clone().into_legacy_form(), legacy_form);
            assert_eq!(Merge::from_legacy_form(legacy_form.0, legacy_form.1), merge);
        }
        // Non-conflict
        test_equivalent(
            (vec![], vec![0]),
            Merge::from_removes_adds(vec![], vec![Some(0)]),
        );
        // Regular 3-way conflict
        test_equivalent(
            (vec![0], vec![1, 2]),
            Merge::from_removes_adds(vec![Some(0)], vec![Some(1), Some(2)]),
        );
        // Modify/delete conflict
        test_equivalent(
            (vec![0], vec![1]),
            Merge::from_removes_adds(vec![Some(0)], vec![Some(1), None]),
        );
        // Add/add conflict
        test_equivalent(
            (vec![], vec![0, 1]),
            Merge::from_removes_adds(vec![None], vec![Some(0), Some(1)]),
        );
        // 5-way conflict
        test_equivalent(
            (vec![0, 1], vec![2, 3, 4]),
            Merge::from_removes_adds(vec![Some(0), Some(1)], vec![Some(2), Some(3), Some(4)]),
        );
        // 5-way delete/delete conflict
        test_equivalent(
            (vec![0, 1], vec![]),
            Merge::from_removes_adds(vec![Some(0), Some(1)], vec![None, None, None]),
        );
    }

    #[test]
    fn test_as_resolved() {
        assert_eq!(
            Merge::from_removes_adds(vec![], vec![0]).as_resolved(),
            Some(&0)
        );
        // Even a trivially resolvable merge is not resolved
        assert_eq!(
            Merge::from_removes_adds(vec![0], vec![0, 1]).as_resolved(),
            None
        );
    }

    #[test]
    fn test_simplify() {
        // 1-way merge
        assert_eq!(c(&[], &[0]).simplify(), c(&[], &[0]));
        // 3-way merge
        assert_eq!(c(&[0], &[0, 0]).simplify(), c(&[], &[0]));
        assert_eq!(c(&[0], &[0, 1]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0], &[1, 0]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0], &[1, 1]).simplify(), c(&[0], &[1, 1]));
        assert_eq!(c(&[0], &[1, 2]).simplify(), c(&[0], &[1, 2]));
        // 5-way merge
        assert_eq!(c(&[0, 0], &[0, 0, 0]).simplify(), c(&[], &[0]));
        assert_eq!(c(&[0, 0], &[0, 0, 1]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0, 0], &[0, 1, 0]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0, 0], &[0, 1, 1]).simplify(), c(&[0], &[1, 1]));
        assert_eq!(c(&[0, 0], &[0, 1, 2]).simplify(), c(&[0], &[1, 2]));
        assert_eq!(c(&[0, 0], &[1, 0, 0]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0, 0], &[1, 0, 1]).simplify(), c(&[0], &[1, 1]));
        assert_eq!(c(&[0, 0], &[1, 0, 2]).simplify(), c(&[0], &[1, 2]));
        assert_eq!(c(&[0, 0], &[1, 1, 0]).simplify(), c(&[0], &[1, 1]));
        assert_eq!(c(&[0, 0], &[1, 1, 1]).simplify(), c(&[0, 0], &[1, 1, 1]));
        assert_eq!(c(&[0, 0], &[1, 1, 2]).simplify(), c(&[0, 0], &[1, 1, 2]));
        assert_eq!(c(&[0, 0], &[1, 2, 0]).simplify(), c(&[0], &[1, 2]));
        assert_eq!(c(&[0, 0], &[1, 2, 1]).simplify(), c(&[0, 0], &[1, 2, 1]));
        assert_eq!(c(&[0, 0], &[1, 2, 2]).simplify(), c(&[0, 0], &[1, 2, 2]));
        assert_eq!(c(&[0, 0], &[1, 2, 3]).simplify(), c(&[0, 0], &[1, 2, 3]));
        assert_eq!(c(&[0, 1], &[0, 0, 0]).simplify(), c(&[1], &[0, 0]));
        assert_eq!(c(&[0, 1], &[0, 0, 1]).simplify(), c(&[], &[0]));
        assert_eq!(c(&[0, 1], &[0, 0, 2]).simplify(), c(&[1], &[0, 2]));
        assert_eq!(c(&[0, 1], &[0, 1, 0]).simplify(), c(&[], &[0]));
        assert_eq!(c(&[0, 1], &[0, 1, 1]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0, 1], &[0, 1, 2]).simplify(), c(&[], &[2]));
        assert_eq!(c(&[0, 1], &[0, 2, 0]).simplify(), c(&[1], &[2, 0]));
        assert_eq!(c(&[0, 1], &[0, 2, 1]).simplify(), c(&[], &[2]));
        assert_eq!(c(&[0, 1], &[0, 2, 2]).simplify(), c(&[1], &[2, 2]));
        assert_eq!(c(&[0, 1], &[0, 2, 3]).simplify(), c(&[1], &[2, 3]));
        assert_eq!(c(&[0, 1], &[1, 0, 0]).simplify(), c(&[], &[0]));
        assert_eq!(c(&[0, 1], &[1, 0, 1]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0, 1], &[1, 0, 2]).simplify(), c(&[], &[2]));
        assert_eq!(c(&[0, 1], &[1, 1, 0]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0, 1], &[1, 1, 1]).simplify(), c(&[0], &[1, 1]));
        assert_eq!(c(&[0, 1], &[1, 1, 2]).simplify(), c(&[0], &[2, 1]));
        assert_eq!(c(&[0, 1], &[1, 2, 0]).simplify(), c(&[], &[2]));
        assert_eq!(c(&[0, 1], &[1, 2, 1]).simplify(), c(&[0], &[1, 2]));
        assert_eq!(c(&[0, 1], &[1, 2, 2]).simplify(), c(&[0], &[2, 2]));
        assert_eq!(c(&[0, 1], &[1, 2, 3]).simplify(), c(&[0], &[3, 2]));
        assert_eq!(c(&[0, 1], &[2, 0, 0]).simplify(), c(&[1], &[2, 0]));
        assert_eq!(c(&[0, 1], &[2, 0, 1]).simplify(), c(&[], &[2]));
        assert_eq!(c(&[0, 1], &[2, 0, 2]).simplify(), c(&[1], &[2, 2]));
        assert_eq!(c(&[0, 1], &[2, 0, 3]).simplify(), c(&[1], &[2, 3]));
        assert_eq!(c(&[0, 1], &[2, 1, 0]).simplify(), c(&[], &[2]));
        assert_eq!(c(&[0, 1], &[2, 1, 1]).simplify(), c(&[0], &[2, 1]));
        assert_eq!(c(&[0, 1], &[2, 1, 2]).simplify(), c(&[0], &[2, 2]));
        assert_eq!(c(&[0, 1], &[2, 1, 3]).simplify(), c(&[0], &[2, 3]));
        assert_eq!(c(&[0, 1], &[2, 2, 0]).simplify(), c(&[1], &[2, 2]));
        assert_eq!(c(&[0, 1], &[2, 2, 1]).simplify(), c(&[0], &[2, 2]));
        assert_eq!(c(&[0, 1], &[2, 2, 2]).simplify(), c(&[0, 1], &[2, 2, 2]));
        assert_eq!(c(&[0, 1], &[2, 2, 3]).simplify(), c(&[0, 1], &[2, 2, 3]));
        assert_eq!(c(&[0, 1], &[2, 3, 0]).simplify(), c(&[1], &[2, 3]));
        assert_eq!(c(&[0, 1], &[2, 3, 1]).simplify(), c(&[0], &[2, 3]));
        assert_eq!(c(&[0, 1], &[2, 3, 2]).simplify(), c(&[0, 1], &[2, 3, 2]));
        assert_eq!(c(&[0, 1], &[2, 3, 3]).simplify(), c(&[0, 1], &[2, 3, 3]));
        assert_eq!(c(&[0, 1], &[2, 3, 4]).simplify(), c(&[0, 1], &[2, 3, 4]));
        assert_eq!(
            c(&[0, 1, 2], &[3, 4, 5, 0]).simplify(),
            c(&[1, 2], &[3, 5, 4])
        );
    }

    #[test]
    fn test_merge_invariants() {
        fn check_invariants(removes: &[u32], adds: &[u32]) {
            let merge = Merge::from_removes_adds(removes.to_vec(), adds.to_vec());
            // `simplify()` is idempotent
            assert_eq!(
                merge.clone().simplify().simplify(),
                merge.clone().simplify(),
                "simplify() not idempotent for {merge:?}"
            );
            // `resolve_trivial()` is unaffected by `simplify()`
            assert_eq!(
                merge.clone().simplify().resolve_trivial(),
                merge.resolve_trivial(),
                "simplify() changed result of resolve_trivial() for {merge:?}"
            );
        }
        // 1-way merge
        check_invariants(&[], &[0]);
        for i in 0..=1 {
            for j in 0..=i + 1 {
                // 3-way merge
                check_invariants(&[0], &[i, j]);
                for k in 0..=j + 1 {
                    for l in 0..=k + 1 {
                        // 5-way merge
                        check_invariants(&[0, i], &[j, k, l]);
                    }
                }
            }
        }
    }

    #[test]
    fn test_swap_remove() {
        let mut x = c(&[1, 3, 5], &[0, 2, 4, 6]);
        assert_eq!(x.swap_remove(0, 1), (1, 2));
        assert_eq!(x, c(&[5, 3], &[0, 6, 4]));
        assert_eq!(x.swap_remove(1, 0), (3, 0));
        assert_eq!(x, c(&[5], &[4, 6]));
        assert_eq!(x.swap_remove(0, 1), (5, 6));
        assert_eq!(x, c(&[], &[4]));
    }

    #[test]
    fn test_pad_to() {
        let mut x = c(&[], &[1]);
        x.pad_to(3, &2);
        assert_eq!(x, c(&[2, 2], &[1, 2, 2]));
        // No change if the requested size is smaller
        x.pad_to(1, &3);
        assert_eq!(x, c(&[2, 2], &[1, 2, 2]));
    }

    #[test]
    fn test_iter() {
        // 1-way merge
        assert_eq!(c(&[], &[1]).iter().collect_vec(), vec![&1]);
        // 5-way merge
        assert_eq!(
            c(&[1, 2], &[3, 4, 5]).iter().collect_vec(),
            vec![&3, &1, &4, &2, &5]
        );
    }

    #[test]
    fn test_from_iter() {
        // 1-way merge
        assert_eq!(MergeBuilder::from_iter([1]).build(), c(&[], &[1]));
        // 5-way merge
        assert_eq!(
            MergeBuilder::from_iter([1, 2, 3, 4, 5]).build(),
            c(&[2, 4], &[1, 3, 5])
        );
    }

    #[test]
    #[should_panic]
    fn test_from_iter_empty() {
        MergeBuilder::from_iter([1; 0]).build();
    }

    #[test]
    #[should_panic]
    fn test_from_iter_even() {
        MergeBuilder::from_iter([1, 2]).build();
    }

    #[test]
    fn test_extend() {
        // 1-way merge
        let mut builder: MergeBuilder<i32> = Default::default();
        builder.extend([1]);
        assert_eq!(builder.build(), c(&[], &[1]));
        // 5-way merge
        let mut builder: MergeBuilder<i32> = Default::default();
        builder.extend([1, 2]);
        builder.extend([3, 4, 5]);
        assert_eq!(builder.build(), c(&[2, 4], &[1, 3, 5]));
    }

    #[test]
    fn test_map() {
        fn increment(i: &i32) -> i32 {
            i + 1
        }
        // 1-way merge
        assert_eq!(c(&[], &[1]).map(increment), c(&[], &[2]));
        // 3-way merge
        assert_eq!(c(&[1], &[3, 5]).map(increment), c(&[2], &[4, 6]));
    }

    #[test]
    fn test_maybe_map() {
        fn sqrt(i: &i32) -> Option<i32> {
            if *i >= 0 {
                Some((*i as f64).sqrt() as i32)
            } else {
                None
            }
        }
        // 1-way merge
        assert_eq!(c(&[], &[1]).maybe_map(sqrt), Some(c(&[], &[1])));
        assert_eq!(c(&[], &[-1]).maybe_map(sqrt), None);
        // 3-way merge
        assert_eq!(c(&[1], &[4, 9]).maybe_map(sqrt), Some(c(&[1], &[2, 3])));
        assert_eq!(c(&[-1], &[4, 9]).maybe_map(sqrt), None);
        assert_eq!(c(&[1], &[-4, 9]).maybe_map(sqrt), None);
    }

    #[test]
    fn test_try_map() {
        fn sqrt(i: &i32) -> Result<i32, ()> {
            if *i >= 0 {
                Ok((*i as f64).sqrt() as i32)
            } else {
                Err(())
            }
        }
        // 1-way merge
        assert_eq!(c(&[], &[1]).try_map(sqrt), Ok(c(&[], &[1])));
        assert_eq!(c(&[], &[-1]).try_map(sqrt), Err(()));
        // 3-way merge
        assert_eq!(c(&[1], &[4, 9]).try_map(sqrt), Ok(c(&[1], &[2, 3])));
        assert_eq!(c(&[-1], &[4, 9]).try_map(sqrt), Err(()));
        assert_eq!(c(&[1], &[-4, 9]).try_map(sqrt), Err(()));
    }

    #[test]
    fn test_flatten() {
        // 1-way merge of 1-way merge
        assert_eq!(c(&[], &[c(&[], &[0])]).flatten(), c(&[], &[0]));
        // 1-way merge of 3-way merge
        assert_eq!(c(&[], &[c(&[0], &[1, 2])]).flatten(), c(&[0], &[1, 2]));
        // 3-way merge of 1-way merges
        assert_eq!(
            c(&[c(&[], &[0])], &[c(&[], &[1]), c(&[], &[2])]).flatten(),
            c(&[0], &[1, 2])
        );
        // 3-way merge of 3-way merges
        assert_eq!(
            c(&[c(&[0], &[1, 2])], &[c(&[3], &[4, 5]), c(&[6], &[7, 8])]).flatten(),
            c(&[3, 2, 1, 6], &[4, 5, 0, 7, 8])
        );
    }
}
