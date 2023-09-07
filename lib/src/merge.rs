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
use std::sync::Arc;

use itertools::Itertools;

use crate::backend;
use crate::backend::{BackendError, FileId, ObjectId, TreeId, TreeValue};
use crate::content_hash::ContentHash;
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

    // Optimize the common cases of 3-way merge and 1-way (non-)merge
    if adds.len() == 1 {
        return Some(&adds[0]);
    } else if adds.len() == 2 {
        return if adds[0] == adds[1] {
            Some(&adds[0])
        } else if adds[0] == removes[0] {
            Some(&adds[1])
        } else if adds[1] == removes[0] {
            Some(&adds[0])
        } else {
            None
        };
    }

    // Number of occurrences of each value, with positive indexes counted as +1 and
    // negative as -1, thereby letting positive and negative terms with the same
    // value (i.e. key in the map) cancel each other.
    let mut counts: HashMap<&T, i32> = HashMap::new();
    for value in adds.iter() {
        counts.entry(value).and_modify(|e| *e += 1).or_insert(1);
    }
    for value in removes.iter() {
        counts.entry(value).and_modify(|e| *e -= 1).or_insert(-1);
    }

    // Collect non-zero value. Values with a count of 0 means that they have
    // cancelled out.
    let counts = counts
        .into_iter()
        .filter(|&(_, count)| count != 0)
        .collect_vec();
    match counts[..] {
        [(value, 1)] => {
            // If there is a single value with a count of 1 left, then that is the result.
            Some(value)
        }
        [(value1, count1), (value2, count2)] => {
            // All sides made the same change.
            // This matches what Git and Mercurial do (in the 3-way case at least), but not
            // what Darcs and Pijul do. It means that repeated 3-way merging of multiple
            // trees may give different results depending on the order of merging.
            // TODO: Consider removing this special case, making the algorithm more strict,
            // and maybe add a more lenient version that is used when the user explicitly
            // asks for conflict resolution.
            assert_eq!(count1 + count2, 1);
            if count1 > 0 {
                Some(value1)
            } else {
                Some(value2)
            }
        }
        _ => None,
    }
}

/// A generic representation of merged values.
///
/// There is exactly one more `adds()` than `removes()`. When interpreted as a
/// series of diffs, the merge's (i+1)-st add is matched with the i-th
/// remove. The zeroth add is considered a diff from the non-existent state.
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Merge<T> {
    removes: Vec<T>,
    adds: Vec<T>,
}

impl<T: Debug> Debug for Merge<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        // Format like an enum with two variants to make it less verbose in the common
        // case of a resolved state.
        if self.removes.is_empty() {
            f.debug_tuple("Resolved").field(&self.adds[0]).finish()
        } else {
            f.debug_struct("Conflicted")
                .field("removes", &self.removes)
                .field("adds", &self.adds)
                .finish()
        }
    }
}

impl<T> Merge<T> {
    /// Creates a new merge object from the given removes and adds.
    pub fn new(removes: Vec<T>, adds: Vec<T>) -> Self {
        assert_eq!(adds.len(), removes.len() + 1);
        Merge { removes, adds }
    }

    /// Creates a `Merge` with a single resolved value.
    pub fn resolved(value: T) -> Self {
        Merge::new(vec![], vec![value])
    }

    /// Create a `Merge` from a `removes` and `adds`, padding with `None` to
    /// make sure that there is exactly one more `adds` than `removes`.
    pub fn from_legacy_form(
        removes: impl IntoIterator<Item = T>,
        adds: impl IntoIterator<Item = T>,
    ) -> Merge<Option<T>> {
        let mut removes = removes.into_iter().map(Some).collect_vec();
        let mut adds = adds.into_iter().map(Some).collect_vec();
        while removes.len() + 1 < adds.len() {
            removes.push(None);
        }
        while adds.len() < removes.len() + 1 {
            adds.push(None);
        }
        Merge::new(removes, adds)
    }

    /// Returns the removes and adds as a pair.
    pub fn take(self) -> (Vec<T>, Vec<T>) {
        (self.removes, self.adds)
    }

    /// The removed values, also called negative terms.
    pub fn removes(&self) -> &[T] {
        &self.removes
    }

    /// The removed values, also called positive terms.
    pub fn adds(&self) -> &[T] {
        &self.adds
    }

    /// The number of positive terms in the conflict.
    pub fn num_sides(&self) -> usize {
        self.adds.len()
    }

    /// Whether this merge is resolved. Does not resolve trivial merges.
    pub fn is_resolved(&self) -> bool {
        self.removes.is_empty()
    }

    /// Returns the resolved value, if this merge is resolved. Does not
    /// resolve trivial merges.
    pub fn as_resolved(&self) -> Option<&T> {
        if let [value] = &self.adds[..] {
            Some(value)
        } else {
            None
        }
    }

    /// Returns the resolved value, if this merge is resolved. Otherwise returns
    /// the merge itself as an `Err`. Does not resolve trivial merges.
    pub fn into_resolved(mut self) -> Result<T, Merge<T>> {
        if self.removes.is_empty() {
            Ok(self.adds.pop().unwrap())
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
        while add_index < self.adds.len() {
            let add = &self.adds[add_index];
            if let Some(remove_index) = self.removes.iter().position(|remove| remove == add) {
                // Move the value to the `add_index-1`th diff, then delete the `remove_index`th
                // diff.
                self.adds.swap(remove_index + 1, add_index);
                self.removes.remove(remove_index);
                self.adds.remove(remove_index + 1);
            } else {
                add_index += 1;
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
        trivial_merge(&self.removes, &self.adds)
    }

    /// Pads this merge with to the specified number of sides with the specified
    /// value. No-op if the requested size is not larger than the current size.
    pub fn pad_to(&mut self, num_sides: usize, value: &T)
    where
        T: Clone,
    {
        for _ in self.num_sides()..num_sides {
            self.removes.push(value.clone());
            self.adds.push(value.clone());
        }
    }

    /// Returns an iterator over references to the terms. The items will
    /// alternate between positive and negative terms, starting with
    /// positive (since there's one more of those).
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        itertools::interleave(&self.adds, &self.removes)
    }

    /// A version of `Merge::iter()` that iterates over mutable references.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        itertools::interleave(self.adds.iter_mut(), self.removes.iter_mut())
    }

    /// Creates a new merge by applying `f` to each remove and add.
    pub fn map<'a, U>(&'a self, f: impl FnMut(&'a T) -> U) -> Merge<U> {
        let builder: MergeBuilder<U> = self.iter().map(f).collect();
        builder.build()
    }

    /// Creates a new merge by applying `f` to each remove and add, returning
    /// `None if `f` returns `None` for any of them.
    pub fn maybe_map<'a, U>(&'a self, f: impl FnMut(&'a T) -> Option<U>) -> Option<Merge<U>> {
        let builder: Option<MergeBuilder<U>> = self.iter().map(f).collect();
        builder.map(MergeBuilder::build)
    }

    /// Creates a new merge by applying `f` to each remove and add, returning
    /// `Err if `f` returns `Err` for any of them.
    pub fn try_map<'a, U, E>(
        &'a self,
        f: impl FnMut(&'a T) -> Result<U, E>,
    ) -> Result<Merge<U>, E> {
        let builder: MergeBuilder<U> = self.iter().map(f).try_collect()?;
        Ok(builder.build())
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
    removes: Vec<T>,
    adds: Vec<T>,
}

impl<T> Default for MergeBuilder<T> {
    fn default() -> Self {
        Self {
            removes: Default::default(),
            adds: Default::default(),
        }
    }
}

impl<T> MergeBuilder<T> {
    /// Requires that exactly one more "adds" than "removes" have been added to
    /// this builder.
    pub fn build(self) -> Merge<T> {
        Merge::new(self.removes, self.adds)
    }
}

impl<T> IntoIterator for Merge<T> {
    type Item = T;
    type IntoIter = itertools::Interleave<std::vec::IntoIter<T>, std::vec::IntoIter<T>>;

    fn into_iter(self) -> Self::IntoIter {
        itertools::interleave(self.adds, self.removes)
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
        let (mut curr, mut next) = if self.adds.len() != self.removes.len() {
            (&mut self.removes, &mut self.adds)
        } else {
            (&mut self.adds, &mut self.removes)
        };
        for item in iter {
            curr.push(item);
            std::mem::swap(&mut curr, &mut next);
        }
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

    /// Creates lists of `removes` and `adds` from a `Merge` by dropping
    /// `None` values. Note that the conversion is lossy: the order of `None`
    /// values is not preserved when converting back to a `Merge`.
    pub fn into_legacy_form(self) -> (Vec<T>, Vec<T>) {
        (
            self.removes.into_iter().flatten().collect(),
            self.adds.into_iter().flatten().collect(),
        )
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
    pub fn flatten(mut self) -> Merge<T> {
        self.removes.reverse();
        self.adds.reverse();
        let mut result = self.adds.pop().unwrap();
        while let Some(mut remove) = self.removes.pop() {
            // Add removes reversed, and with the first element moved last, so we preserve
            // the diffs
            let first_add = remove.adds.remove(0);
            result.removes.extend(remove.adds);
            result.removes.push(first_add);
            result.adds.extend(remove.removes);
            let add = self.adds.pop().unwrap();
            result.removes.extend(add.removes);
            result.adds.extend(add.adds);
        }
        assert!(self.adds.is_empty());
        result
    }
}

impl<T: ContentHash> ContentHash for Merge<T> {
    fn hash(&self, state: &mut impl digest::Update) {
        self.removes().hash(state);
        self.adds().hash(state);
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
        assert_eq!(self.removes.len(), file_ids.removes.len());
        let builder: MergeBuilder<Option<TreeValue>> = zip(self.iter(), file_ids.iter())
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
        builder.build()
    }

    /// Give a summary description of the conflict's "removes" and "adds"
    pub fn describe(&self, file: &mut dyn Write) -> std::io::Result<()> {
        file.write_all(b"Conflict:\n")?;
        for term in self.removes().iter().flatten() {
            file.write_all(format!("  Removing {}\n", describe_conflict_term(term)).as_bytes())?;
        }
        for term in self.adds().iter().flatten() {
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
                    Ok(Tree::null(store.clone(), dir.clone()))
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
        Merge::new(removes.to_vec(), adds.to_vec())
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
        test_equivalent((vec![], vec![0]), Merge::new(vec![], vec![Some(0)]));
        // Regular 3-way conflict
        test_equivalent(
            (vec![0], vec![1, 2]),
            Merge::new(vec![Some(0)], vec![Some(1), Some(2)]),
        );
        // Modify/delete conflict
        test_equivalent(
            (vec![0], vec![1]),
            Merge::new(vec![Some(0)], vec![Some(1), None]),
        );
        // Add/add conflict
        test_equivalent(
            (vec![], vec![0, 1]),
            Merge::new(vec![None], vec![Some(0), Some(1)]),
        );
        // 5-way conflict
        test_equivalent(
            (vec![0, 1], vec![2, 3, 4]),
            Merge::new(vec![Some(0), Some(1)], vec![Some(2), Some(3), Some(4)]),
        );
        // 5-way delete/delete conflict
        test_equivalent(
            (vec![0, 1], vec![]),
            Merge::new(vec![Some(0), Some(1)], vec![None, None, None]),
        );
    }

    #[test]
    fn test_as_resolved() {
        assert_eq!(Merge::new(vec![], vec![0]).as_resolved(), Some(&0));
        // Even a trivially resolvable merge is not resolved
        assert_eq!(Merge::new(vec![0], vec![0, 1]).as_resolved(), None);
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
            let merge = Merge::new(removes.to_vec(), adds.to_vec());
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
