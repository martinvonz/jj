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

#![allow(missing_docs)]

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::io::Write;
use std::sync::Arc;

use itertools::Itertools;

use crate::backend::{BackendError, BackendResult, FileId, ObjectId, TreeId, TreeValue};
use crate::content_hash::ContentHash;
use crate::files::ContentHunk;
use crate::repo_path::RepoPath;
use crate::store::Store;
use crate::tree::Tree;
use crate::{backend, conflicts};

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
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct Merge<T> {
    removes: Vec<T>,
    adds: Vec<T>,
}

impl<T> Merge<T> {
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

    pub fn removes(&self) -> &[T] {
        &self.removes
    }

    pub fn adds(&self) -> &[T] {
        &self.adds
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

    pub fn resolve_trivial(&self) -> Option<&T>
    where
        T: Eq + Hash,
    {
        trivial_merge(&self.removes, &self.adds)
    }

    /// Creates a new merge by applying `f` to each remove and add.
    pub fn map<'a, U>(&'a self, mut f: impl FnMut(&'a T) -> U) -> Merge<U> {
        self.maybe_map(|term| Some(f(term))).unwrap()
    }

    /// Creates a new merge by applying `f` to each remove and add, returning
    /// `None if `f` returns `None` for any of them.
    pub fn maybe_map<'a, U>(&'a self, mut f: impl FnMut(&'a T) -> Option<U>) -> Option<Merge<U>> {
        let removes = self.removes.iter().map(&mut f).collect::<Option<_>>()?;
        let adds = self.adds.iter().map(&mut f).collect::<Option<_>>()?;
        Some(Merge { removes, adds })
    }

    /// Creates a new merge by applying `f` to each remove and add, returning
    /// `Err if `f` returns `Err` for any of them.
    pub fn try_map<'a, U, E>(
        &'a self,
        mut f: impl FnMut(&'a T) -> Result<U, E>,
    ) -> Result<Merge<U>, E> {
        let removes = self.removes.iter().map(&mut f).try_collect()?;
        let adds = self.adds.iter().map(&mut f).try_collect()?;
        Ok(Merge { removes, adds })
    }
}

impl<T> Merge<Option<T>> {
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

impl Merge<TreeId> {
    // Creates a resolved merge for a legacy tree id (same as
    // `Merge::resolved()`).
    // TODO(#1624): delete when all callers have been updated to support tree-level
    // conflicts
    pub fn from_legacy_tree_id(value: TreeId) -> Self {
        Merge {
            removes: vec![],
            adds: vec![value],
        }
    }

    // TODO(#1624): delete when all callers have been updated to support tree-level
    // conflicts
    pub fn as_legacy_tree_id(&self) -> &TreeId {
        self.as_resolved().unwrap()
    }
}

impl Merge<Option<TreeValue>> {
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

    pub fn materialize(
        &self,
        store: &Store,
        path: &RepoPath,
        output: &mut dyn Write,
    ) -> std::io::Result<()> {
        if let Some(file_merge) = self.to_file_merge() {
            let content = file_merge.extract_as_single_hunk(store, path);
            conflicts::materialize_merge_result(&content, output)
        } else {
            // Unless all terms are regular files, we can't do much better than to try to
            // describe the merge.
            self.describe(output)
        }
    }

    pub fn to_file_merge(&self) -> Option<Merge<Option<FileId>>> {
        self.maybe_map(|term| match term {
            None => Some(None),
            Some(TreeValue::File {
                id,
                executable: false,
            }) => Some(Some(id.clone())),
            _ => None,
        })
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

    /// Returns `None` if there are no conflict markers in `content`.
    pub fn update_from_content(
        &self,
        store: &Store,
        path: &RepoPath,
        content: &[u8],
    ) -> BackendResult<Option<Merge<Option<TreeValue>>>> {
        // TODO: Check that the conflict only involves files and convert it to a
        // `Merge<Option<FileId>>` so we can remove the wildcard pattern in the loops
        // further down.

        // First check if the new content is unchanged compared to the old content. If
        // it is, we don't need parse the content or write any new objects to the
        // store. This is also a way of making sure that unchanged tree/file
        // conflicts (for example) are not converted to regular files in the working
        // copy.
        let mut old_content = Vec::with_capacity(content.len());
        self.materialize(store, path, &mut old_content).unwrap();
        if content == old_content {
            return Ok(Some(self.clone()));
        }

        let mut removed_content = vec![vec![]; self.removes().len()];
        let mut added_content = vec![vec![]; self.adds().len()];
        let Some(hunks) =
            conflicts::parse_conflict(content, self.removes().len(), self.adds().len())
        else {
            // Either there are no self markers of they don't have the expected arity
            return Ok(None);
        };
        for hunk in hunks {
            if let Some(slice) = hunk.as_resolved() {
                for buf in &mut removed_content {
                    buf.extend_from_slice(&slice.0);
                }
                for buf in &mut added_content {
                    buf.extend_from_slice(&slice.0);
                }
            } else {
                let (removes, adds) = hunk.take();
                for (i, buf) in removes.into_iter().enumerate() {
                    removed_content[i].extend(buf.0);
                }
                for (i, buf) in adds.into_iter().enumerate() {
                    added_content[i].extend(buf.0);
                }
            }
        }
        // Now write the new files contents we found by parsing the file
        // with conflict markers. Update the Merge object with the new
        // FileIds.
        let mut new_removes = vec![];
        for (i, buf) in removed_content.iter().enumerate() {
            match &self.removes()[i] {
                Some(TreeValue::File { id: _, executable }) => {
                    let file_id = store.write_file(path, &mut buf.as_slice())?;
                    let new_value = TreeValue::File {
                        id: file_id,
                        executable: *executable,
                    };
                    new_removes.push(Some(new_value));
                }
                None if buf.is_empty() => {
                    // The missing side of a conflict is still represented by
                    // the empty string we materialized it as
                    new_removes.push(None);
                }
                _ => {
                    // The user edited a non-file side. This should never happen. We consider the
                    // conflict resolved for now.
                    return Ok(None);
                }
            }
        }
        let mut new_adds = vec![];
        for (i, buf) in added_content.iter().enumerate() {
            match &self.adds()[i] {
                Some(TreeValue::File { id: _, executable }) => {
                    let file_id = store.write_file(path, &mut buf.as_slice())?;
                    let new_value = TreeValue::File {
                        id: file_id,
                        executable: *executable,
                    };
                    new_adds.push(Some(new_value));
                }
                None if buf.is_empty() => {
                    // The missing side of a conflict is still represented by
                    // the empty string we materialized it as => nothing to do
                    new_adds.push(None);
                }
                _ => {
                    // The user edited a non-file side. This should never happen. We consider the
                    // conflict resolved for now.
                    return Ok(None);
                }
            }
        }
        Ok(Some(Merge::new(new_removes, new_adds)))
    }
}

impl Merge<Option<FileId>> {
    pub fn extract_as_single_hunk(&self, store: &Store, path: &RepoPath) -> Merge<ContentHunk> {
        self.map(|term| get_file_contents(store, path, term))
    }
}

impl<T> Merge<Option<T>>
where
    T: Borrow<TreeValue>,
{
    /// If every non-`None` term of a `Merge<Option<TreeValue>>`
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

fn get_file_contents(store: &Store, path: &RepoPath, term: &Option<FileId>) -> ContentHunk {
    match term {
        Some(id) => {
            let mut content = vec![];
            store
                .read_file(path, id)
                .unwrap()
                .read_to_end(&mut content)
                .unwrap();
            ContentHunk(content)
        }
        // If the conflict had removed the file on one side, we pretend that the file
        // was empty there.
        None => ContentHunk(vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
