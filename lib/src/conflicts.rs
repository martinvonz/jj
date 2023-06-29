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

use std::hash::Hash;
use std::io::Write;

use itertools::Itertools;

use crate::backend::{BackendResult, FileId, ObjectId, TreeValue};
use crate::diff::{find_line_ranges, Diff, DiffHunk};
use crate::files::{ContentHunk, MergeResult};
use crate::merge::trivial_merge;
use crate::repo_path::RepoPath;
use crate::store::Store;
use crate::{backend, files};

const CONFLICT_START_LINE: &[u8] = b"<<<<<<<\n";
const CONFLICT_END_LINE: &[u8] = b">>>>>>>\n";
const CONFLICT_DIFF_LINE: &[u8] = b"%%%%%%%\n";
const CONFLICT_MINUS_LINE: &[u8] = b"-------\n";
const CONFLICT_PLUS_LINE: &[u8] = b"+++++++\n";

/// A generic representation of conflicting values.
///
/// There is exactly one more `adds()` than `removes()`. When interpreted as a
/// series of diffs, the conflict's (i+1)-st add is matched with the i-th
/// remove. The zeroth add is considered a diff from the non-existent state.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct Conflict<T> {
    removes: Vec<T>,
    adds: Vec<T>,
}

impl<T> Conflict<T> {
    pub fn new(removes: Vec<T>, adds: Vec<T>) -> Self {
        assert_eq!(adds.len(), removes.len() + 1);
        Conflict { removes, adds }
    }

    pub fn resolved(value: T) -> Self {
        Conflict::new(vec![], vec![value])
    }

    /// Create a `Conflict` from a `removes` and `adds`, padding with `None` to
    /// make sure that there is exactly one more `adds` than `removes`.
    pub fn from_legacy_form(removes: Vec<T>, adds: Vec<T>) -> Conflict<Option<T>> {
        let mut removes = removes.into_iter().map(Some).collect_vec();
        let mut adds = adds.into_iter().map(Some).collect_vec();
        while removes.len() + 1 < adds.len() {
            removes.push(None);
        }
        while adds.len() < removes.len() + 1 {
            adds.push(None);
        }
        Conflict::new(removes, adds)
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

    /// Returns the resolved value, if this conflict is resolved. Does not
    /// resolve trivial conflicts.
    pub fn as_resolved(&self) -> Option<&T> {
        if let [value] = &self.adds[..] {
            Some(value)
        } else {
            None
        }
    }

    /// Simplify the conflict by joining diffs like A->B and B->C into A->C.
    /// Also drops trivial diffs like A->A.
    pub fn simplify(mut self) -> Self
    where
        T: PartialEq,
    {
        let mut add_index = 0;
        while add_index < self.adds.len() {
            let add = &self.adds[add_index];
            let mut modified = false;
            for (remove_index, remove) in self.removes.iter().enumerate() {
                if remove == add {
                    // Move the value to the `add_index-1`th diff, then delete the `remove_index`th
                    // diff.
                    self.adds.swap(remove_index + 1, add_index);
                    self.removes.remove(remove_index);
                    self.adds.remove(remove_index + 1);
                    modified = true;
                    break;
                }
            }
            if !modified {
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

    /// Creates a new conflict by applying `f` to each remove and add.
    pub fn map<'a, U>(&'a self, mut f: impl FnMut(&'a T) -> U) -> Conflict<U> {
        self.maybe_map(|term| Some(f(term))).unwrap()
    }

    /// Creates a new conflict by applying `f` to each remove and add, returning
    /// `None if `f` returns `None` for any of them.
    pub fn maybe_map<'a, U>(
        &'a self,
        mut f: impl FnMut(&'a T) -> Option<U>,
    ) -> Option<Conflict<U>> {
        let removes = self.removes.iter().map(&mut f).collect::<Option<_>>()?;
        let adds = self.adds.iter().map(&mut f).collect::<Option<_>>()?;
        Some(Conflict { removes, adds })
    }

    /// Creates a new conflict by applying `f` to each remove and add, returning
    /// `Err if `f` returns `Err` for any of them.
    pub fn try_map<'a, U, E>(
        &'a self,
        mut f: impl FnMut(&'a T) -> Result<U, E>,
    ) -> Result<Conflict<U>, E> {
        let removes = self.removes.iter().map(&mut f).try_collect()?;
        let adds = self.adds.iter().map(&mut f).try_collect()?;
        Ok(Conflict { removes, adds })
    }
}

impl<T> Conflict<Option<T>> {
    /// Creates lists of `removes` and `adds` from a `Conflict` by dropping
    /// `None` values. Note that the conversion is lossy: the order of `None`
    /// values is not preserved when converting back to a `Conflict`.
    pub fn into_legacy_form(self) -> (Vec<T>, Vec<T>) {
        (
            self.removes.into_iter().flatten().collect(),
            self.adds.into_iter().flatten().collect(),
        )
    }
}

impl Conflict<Option<TreeValue>> {
    /// Create a `Conflict` from a `backend::Conflict`, padding with `None` to
    /// make sure that there is exactly one more `adds()` than `removes()`.
    pub fn from_backend_conflict(conflict: backend::Conflict) -> Self {
        let removes = conflict
            .removes
            .into_iter()
            .map(|term| term.value)
            .collect();
        let adds = conflict.adds.into_iter().map(|term| term.value).collect();
        Conflict::from_legacy_form(removes, adds)
    }

    /// Creates a `backend::Conflict` from a `Conflict` by dropping `None`
    /// values. Note that the conversion is lossy: the order of `None` values is
    /// not preserved when converting back to a `Conflict`.
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
        if let Some(file_conflict) = self.to_file_conflict() {
            let content = file_conflict.extract_as_single_hunk(store, path);
            materialize_merge_result(&content, output)
        } else {
            // Unless all terms are regular files, we can't do much better than to try to
            // describe the conflict.
            self.describe(output)
        }
    }

    pub fn to_file_conflict(&self) -> Option<Conflict<Option<FileId>>> {
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
    ) -> BackendResult<Option<Conflict<Option<TreeValue>>>> {
        // TODO: Check that the conflict only involves files and convert it to a
        // `Conflict<Option<FileId>>` so we can remove the wildcard pattern in the loops
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
        // TODO: Change to let-else once our MSRV is above 1.65
        let hunks =
            if let Some(hunks) = parse_conflict(content, self.removes().len(), self.adds().len()) {
                hunks
            } else {
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
        // with conflict markers. Update the Conflict object with the new
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
        Ok(Some(Conflict::new(new_removes, new_adds)))
    }
}

impl Conflict<Option<FileId>> {
    pub fn extract_as_single_hunk(&self, store: &Store, path: &RepoPath) -> Conflict<ContentHunk> {
        self.map(|term| get_file_contents(store, path, term))
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

fn write_diff_hunks(hunks: &[DiffHunk], file: &mut dyn Write) -> std::io::Result<()> {
    for hunk in hunks {
        match hunk {
            DiffHunk::Matching(content) => {
                for line in content.split_inclusive(|b| *b == b'\n') {
                    file.write_all(b" ")?;
                    file.write_all(line)?;
                }
            }
            DiffHunk::Different(content) => {
                for line in content[0].split_inclusive(|b| *b == b'\n') {
                    file.write_all(b"-")?;
                    file.write_all(line)?;
                }
                for line in content[1].split_inclusive(|b| *b == b'\n') {
                    file.write_all(b"+")?;
                    file.write_all(line)?;
                }
            }
        }
    }
    Ok(())
}

pub fn materialize_merge_result(
    single_hunk: &Conflict<ContentHunk>,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    let removed_slices = single_hunk
        .removes
        .iter()
        .map(|hunk| hunk.0.as_slice())
        .collect_vec();
    let added_slices = single_hunk
        .adds
        .iter()
        .map(|hunk| hunk.0.as_slice())
        .collect_vec();
    let merge_result = files::merge(&removed_slices, &added_slices);
    match merge_result {
        MergeResult::Resolved(content) => {
            output.write_all(&content.0)?;
        }
        MergeResult::Conflict(hunks) => {
            for hunk in hunks {
                if let Some(content) = hunk.as_resolved() {
                    output.write_all(&content.0)?;
                } else {
                    output.write_all(CONFLICT_START_LINE)?;
                    let mut add_index = 0;
                    for left in hunk.removes() {
                        let right1 = if let Some(right1) = hunk.adds().get(add_index) {
                            right1
                        } else {
                            // If we have no more positive terms, emit the remaining negative
                            // terms as snapshots.
                            output.write_all(CONFLICT_MINUS_LINE)?;
                            output.write_all(&left.0)?;
                            continue;
                        };
                        let diff1 = Diff::for_tokenizer(&[&left.0, &right1.0], &find_line_ranges)
                            .hunks()
                            .collect_vec();
                        // Check if the diff against the next positive term is better. Since
                        // we want to preserve the order of the terms, we don't match against
                        // any later positive terms.
                        if let Some(right2) = hunk.adds().get(add_index + 1) {
                            let diff2 =
                                Diff::for_tokenizer(&[&left.0, &right2.0], &find_line_ranges)
                                    .hunks()
                                    .collect_vec();
                            if diff_size(&diff2) < diff_size(&diff1) {
                                // If the next positive term is a better match, emit
                                // the current positive term as a snapshot and the next
                                // positive term as a diff.
                                output.write_all(CONFLICT_PLUS_LINE)?;
                                output.write_all(&right1.0)?;
                                output.write_all(CONFLICT_DIFF_LINE)?;
                                write_diff_hunks(&diff2, output)?;
                                add_index += 2;
                                continue;
                            }
                        }

                        output.write_all(CONFLICT_DIFF_LINE)?;
                        write_diff_hunks(&diff1, output)?;
                        add_index += 1;
                    }

                    //  Emit the remaining positive terms as snapshots.
                    for slice in &hunk.adds()[add_index..] {
                        output.write_all(CONFLICT_PLUS_LINE)?;
                        output.write_all(&slice.0)?;
                    }
                    output.write_all(CONFLICT_END_LINE)?;
                }
            }
        }
    }
    Ok(())
}

fn diff_size(hunks: &[DiffHunk]) -> usize {
    hunks
        .iter()
        .map(|hunk| match hunk {
            DiffHunk::Matching(_) => 0,
            DiffHunk::Different(slices) => slices.iter().map(|slice| slice.len()).sum(),
        })
        .sum()
}

/// Parses conflict markers from a slice. Returns None if there were no valid
/// conflict markers. The caller has to provide the expected number of removed
/// and added inputs to the conflicts. Conflict markers that are otherwise valid
/// will be considered invalid if they don't have the expected arity.
// TODO: "parse" is not usually the opposite of "materialize", so maybe we
// should rename them to "serialize" and "deserialize"?
pub fn parse_conflict(
    input: &[u8],
    num_removes: usize,
    num_adds: usize,
) -> Option<Vec<Conflict<ContentHunk>>> {
    if input.is_empty() {
        return None;
    }
    let mut hunks = vec![];
    let mut pos = 0;
    let mut resolved_start = 0;
    let mut conflict_start = None;
    for line in input.split_inclusive(|b| *b == b'\n') {
        if line == CONFLICT_START_LINE {
            conflict_start = Some(pos);
        } else if conflict_start.is_some() && line == CONFLICT_END_LINE {
            let conflict_body = &input[conflict_start.unwrap() + CONFLICT_START_LINE.len()..pos];
            let hunk = parse_conflict_hunk(conflict_body);
            if hunk.removes().len() == num_removes && hunk.adds().len() == num_adds {
                let resolved_slice = &input[resolved_start..conflict_start.unwrap()];
                if !resolved_slice.is_empty() {
                    hunks.push(Conflict::resolved(ContentHunk(resolved_slice.to_vec())));
                }
                hunks.push(hunk);
                resolved_start = pos + line.len();
            }
            conflict_start = None;
        }
        pos += line.len();
    }

    if hunks.is_empty() {
        None
    } else {
        if resolved_start < input.len() {
            hunks.push(Conflict::resolved(ContentHunk(
                input[resolved_start..].to_vec(),
            )));
        }
        Some(hunks)
    }
}

fn parse_conflict_hunk(input: &[u8]) -> Conflict<ContentHunk> {
    enum State {
        Diff,
        Minus,
        Plus,
        Unknown,
    }
    let mut state = State::Unknown;
    let mut removes = vec![];
    let mut adds = vec![];
    for line in input.split_inclusive(|b| *b == b'\n') {
        match line {
            CONFLICT_DIFF_LINE => {
                state = State::Diff;
                removes.push(ContentHunk(vec![]));
                adds.push(ContentHunk(vec![]));
                continue;
            }
            CONFLICT_MINUS_LINE => {
                state = State::Minus;
                removes.push(ContentHunk(vec![]));
                continue;
            }
            CONFLICT_PLUS_LINE => {
                state = State::Plus;
                adds.push(ContentHunk(vec![]));
                continue;
            }
            _ => {}
        };
        match state {
            State::Diff => {
                if let Some(rest) = line.strip_prefix(b"-") {
                    removes.last_mut().unwrap().0.extend_from_slice(rest);
                } else if let Some(rest) = line.strip_prefix(b"+") {
                    adds.last_mut().unwrap().0.extend_from_slice(rest);
                } else if let Some(rest) = line.strip_prefix(b" ") {
                    removes.last_mut().unwrap().0.extend_from_slice(rest);
                    adds.last_mut().unwrap().0.extend_from_slice(rest);
                } else {
                    // Doesn't look like a conflict
                    return Conflict::resolved(ContentHunk(vec![]));
                }
            }
            State::Minus => {
                removes.last_mut().unwrap().0.extend_from_slice(line);
            }
            State::Plus => {
                adds.last_mut().unwrap().0.extend_from_slice(line);
            }
            State::Unknown => {
                // Doesn't look like a conflict
                return Conflict::resolved(ContentHunk(vec![]));
            }
        }
    }

    Conflict::new(removes, adds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_form_conversion() {
        fn test_equivalent<T>(legacy_form: (Vec<T>, Vec<T>), conflict: Conflict<Option<T>>)
        where
            T: Clone + PartialEq + std::fmt::Debug,
        {
            assert_eq!(conflict.clone().into_legacy_form(), legacy_form);
            assert_eq!(
                Conflict::from_legacy_form(legacy_form.0, legacy_form.1),
                conflict
            );
        }
        // Non-conflict
        test_equivalent((vec![], vec![0]), Conflict::new(vec![], vec![Some(0)]));
        // Regular 3-way conflict
        test_equivalent(
            (vec![0], vec![1, 2]),
            Conflict::new(vec![Some(0)], vec![Some(1), Some(2)]),
        );
        // Modify/delete conflict
        test_equivalent(
            (vec![0], vec![1]),
            Conflict::new(vec![Some(0)], vec![Some(1), None]),
        );
        // Add/add conflict
        test_equivalent(
            (vec![], vec![0, 1]),
            Conflict::new(vec![None], vec![Some(0), Some(1)]),
        );
        // 5-way conflict
        test_equivalent(
            (vec![0, 1], vec![2, 3, 4]),
            Conflict::new(vec![Some(0), Some(1)], vec![Some(2), Some(3), Some(4)]),
        );
        // 5-way delete/delete conflict
        test_equivalent(
            (vec![0, 1], vec![]),
            Conflict::new(vec![Some(0), Some(1)], vec![None, None, None]),
        );
    }

    #[test]
    fn test_as_resolved() {
        assert_eq!(Conflict::new(vec![], vec![0]).as_resolved(), Some(&0));
        // Even a trivially resolvable conflict is not resolved
        assert_eq!(Conflict::new(vec![0], vec![0, 1]).as_resolved(), None);
    }

    #[test]
    fn test_simplify() {
        fn c(removes: &[u32], adds: &[u32]) -> Conflict<u32> {
            Conflict::new(removes.to_vec(), adds.to_vec())
        }
        // 1-way "conflict"
        assert_eq!(c(&[], &[0]).simplify(), c(&[], &[0]));
        // 3-way conflict
        assert_eq!(c(&[0], &[0, 0]).simplify(), c(&[], &[0]));
        assert_eq!(c(&[0], &[0, 1]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0], &[1, 0]).simplify(), c(&[], &[1]));
        assert_eq!(c(&[0], &[1, 1]).simplify(), c(&[0], &[1, 1]));
        assert_eq!(c(&[0], &[1, 2]).simplify(), c(&[0], &[1, 2]));
        // Irreducible 5-way conflict
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
    fn test_conflict_invariants() {
        fn check_invariants(removes: &[u32], adds: &[u32]) {
            let conflict = Conflict::new(removes.to_vec(), adds.to_vec());
            // `simplify()` is idempotent
            assert_eq!(
                conflict.clone().simplify().simplify(),
                conflict.clone().simplify(),
                "simplify() not idempotent for {conflict:?}"
            );
            // `resolve_trivial()` is unaffected by `simplify()`
            assert_eq!(
                conflict.clone().simplify().resolve_trivial(),
                conflict.resolve_trivial(),
                "simplify() changed result of resolve_trivial() for {conflict:?}"
            );
        }
        // 1-way "conflict"
        check_invariants(&[], &[0]);
        for i in 0..=1 {
            for j in 0..=i + 1 {
                // 3-way conflict
                check_invariants(&[0], &[i, j]);
                for k in 0..=j + 1 {
                    for l in 0..=k + 1 {
                        // 5-way conflict
                        check_invariants(&[0, i], &[j, k, l]);
                    }
                }
            }
        }
    }

    #[test]
    fn test_map() {
        fn increment(i: &i32) -> i32 {
            i + 1
        }
        fn c(removes: &[i32], adds: &[i32]) -> Conflict<i32> {
            Conflict::new(removes.to_vec(), adds.to_vec())
        }
        // 1-way conflict
        assert_eq!(c(&[], &[1]).map(increment), c(&[], &[2]));
        // 3-way conflict
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
        fn c(removes: &[i32], adds: &[i32]) -> Conflict<i32> {
            Conflict::new(removes.to_vec(), adds.to_vec())
        }
        // 1-way conflict
        assert_eq!(c(&[], &[1]).maybe_map(sqrt), Some(c(&[], &[1])));
        assert_eq!(c(&[], &[-1]).maybe_map(sqrt), None);
        // 3-way conflict
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
        fn c(removes: &[i32], adds: &[i32]) -> Conflict<i32> {
            Conflict::new(removes.to_vec(), adds.to_vec())
        }
        // 1-way conflict
        assert_eq!(c(&[], &[1]).try_map(sqrt), Ok(c(&[], &[1])));
        assert_eq!(c(&[], &[-1]).try_map(sqrt), Err(()));
        // 3-way conflict
        assert_eq!(c(&[1], &[4, 9]).try_map(sqrt), Ok(c(&[1], &[2, 3])));
        assert_eq!(c(&[-1], &[4, 9]).try_map(sqrt), Err(()));
        assert_eq!(c(&[1], &[-4, 9]).try_map(sqrt), Err(()));
    }
}
