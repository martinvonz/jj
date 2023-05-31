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

use std::cmp::max;
use std::io::Write;

use itertools::Itertools;

use crate::backend::{BackendResult, ConflictId, ObjectId, TreeValue};
use crate::diff::{find_line_ranges, Diff, DiffHunk};
use crate::files::{ConflictHunk, MergeHunk, MergeResult};
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
/// There is exactly one more `adds()` than `removes()`.
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

    pub fn removes(&self) -> &[T] {
        &self.removes
    }

    pub fn adds(&self) -> &[T] {
        &self.adds
    }

    pub fn set_remove(&mut self, i: usize, value: T) {
        self.removes[i] = value;
    }

    pub fn set_add(&mut self, i: usize, value: T) {
        self.adds[i] = value;
    }
}

impl Conflict<Option<TreeValue>> {
    /// Create a `Conflict` from a `backend::Conflict`, padding with `None` to
    /// make sure that there is exactly one more `adds()` than `removes()`.
    pub fn from_backend_conflict(conflict: &backend::Conflict) -> Self {
        let mut removes = conflict
            .removes
            .iter()
            .map(|term| Some(term.value.clone()))
            .collect_vec();
        let mut adds = conflict
            .adds
            .iter()
            .map(|term| Some(term.value.clone()))
            .collect_vec();
        let num_diffs = max(removes.len() + 1, adds.len()) - 1;
        removes.resize(num_diffs, None);
        adds.resize(num_diffs + 1, None);
        Conflict { removes, adds }
    }

    /// Creates a `backend::Conflict` from a `Conflict` by dropping `None`
    /// values. Note that the conflict is lossy: the order of `None` values is
    /// not preserved when converting back to a `Conflict`.
    pub fn to_backend_conflict(&self) -> backend::Conflict {
        let removes = self
            .removes
            .iter()
            .flatten()
            .map(|value| backend::ConflictTerm {
                value: value.clone(),
            })
            .collect_vec();
        let adds = self
            .adds
            .iter()
            .flatten()
            .map(|value| backend::ConflictTerm {
                value: value.clone(),
            })
            .collect_vec();
        backend::Conflict { removes, adds }
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

/// Give a summary description of a conflict's "removes" and "adds"
pub fn describe_conflict(
    conflict: &Conflict<Option<TreeValue>>,
    file: &mut dyn Write,
) -> std::io::Result<()> {
    file.write_all(b"Conflict:\n")?;
    for term in conflict.removes().iter().flatten() {
        file.write_all(format!("  Removing {}\n", describe_conflict_term(term)).as_bytes())?;
    }
    for term in conflict.adds().iter().flatten() {
        file.write_all(format!("  Adding {}\n", describe_conflict_term(term)).as_bytes())?;
    }
    Ok(())
}

fn file_terms(terms: &[Option<TreeValue>]) -> Vec<Option<&TreeValue>> {
    terms
        .iter()
        .filter_map(|term| match term {
            Some(
                value @ TreeValue::File {
                    executable: false, ..
                },
            ) => Some(Some(value)),
            None => Some(None),
            _ => None,
        })
        .collect_vec()
}

fn get_file_contents(store: &Store, path: &RepoPath, term: Option<&TreeValue>) -> Vec<u8> {
    match term {
        Some(TreeValue::File {
            id,
            executable: false,
        }) => {
            let mut content = vec![];
            store
                .read_file(path, id)
                .unwrap()
                .read_to_end(&mut content)
                .unwrap();
            content
        }
        // If the conflict had removed the file on one side, we pretend that the file
        // was empty there.
        None => vec![],
        _ => panic!("unexpectedly got a non-file conflict term"),
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

pub fn materialize_conflict(
    store: &Store,
    path: &RepoPath,
    conflict: &Conflict<Option<TreeValue>>,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    match extract_file_conflict_as_single_hunk(store, path, conflict) {
        None => {
            // Unless all terms are regular files, we can't do much better than to try to
            // describe the conflict.
            describe_conflict(conflict, output)
        }
        Some(content) => materialize_merge_result(&content, output),
    }
}

/// Only works if all terms of the conflict are regular, non-executable files
pub fn extract_file_conflict_as_single_hunk(
    store: &Store,
    path: &RepoPath,
    conflict: &Conflict<Option<TreeValue>>,
) -> Option<ConflictHunk> {
    let file_removes = file_terms(conflict.removes());
    let file_adds = file_terms(conflict.adds());
    if file_removes.len() != conflict.removes().len() || file_adds.len() != conflict.adds().len() {
        return None;
    }
    let removes_content = file_removes
        .iter()
        .map(|term| get_file_contents(store, path, *term))
        .collect_vec();
    let adds_content = file_adds
        .iter()
        .map(|term| get_file_contents(store, path, *term))
        .collect_vec();

    Some(ConflictHunk {
        removes: removes_content,
        adds: adds_content,
    })
}

pub fn materialize_merge_result(
    single_hunk: &ConflictHunk,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    let removed_slices = single_hunk.removes.iter().map(Vec::as_slice).collect_vec();
    let added_slices = single_hunk.adds.iter().map(Vec::as_slice).collect_vec();
    let merge_result = files::merge(&removed_slices, &added_slices);
    match merge_result {
        MergeResult::Resolved(content) => {
            output.write_all(&content)?;
        }
        MergeResult::Conflict(hunks) => {
            for hunk in hunks {
                match hunk {
                    MergeHunk::Resolved(content) => {
                        output.write_all(&content)?;
                    }
                    MergeHunk::Conflict(ConflictHunk { removes, adds }) => {
                        output.write_all(CONFLICT_START_LINE)?;
                        let mut add_index = 0;
                        for left in &removes {
                            let right1 = if let Some(right1) = adds.get(add_index) {
                                right1
                            } else {
                                // If we have no more positive terms, emit the remaining negative
                                // terms as snapshots.
                                output.write_all(CONFLICT_MINUS_LINE)?;
                                output.write_all(left)?;
                                continue;
                            };
                            let diff1 = Diff::for_tokenizer(&[left, right1], &find_line_ranges)
                                .hunks()
                                .collect_vec();
                            // Check if the diff against the next positive term is better. Since
                            // we want to preserve the order of the terms, we don't match against
                            // any later positive terms.
                            if let Some(right2) = adds.get(add_index + 1) {
                                let diff2 = Diff::for_tokenizer(&[left, right2], &find_line_ranges)
                                    .hunks()
                                    .collect_vec();
                                if diff_size(&diff2) < diff_size(&diff1) {
                                    // If the next positive term is a better match, emit
                                    // the current positive term as a snapshot and the next
                                    // positive term as a diff.
                                    output.write_all(CONFLICT_PLUS_LINE)?;
                                    output.write_all(right1)?;
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
                        for slice in &adds[add_index..] {
                            output.write_all(CONFLICT_PLUS_LINE)?;
                            output.write_all(slice)?;
                        }
                        output.write_all(CONFLICT_END_LINE)?;
                    }
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
pub fn parse_conflict(input: &[u8], num_removes: usize, num_adds: usize) -> Option<Vec<MergeHunk>> {
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
            match &hunk {
                MergeHunk::Conflict(ConflictHunk { removes, adds })
                    if removes.len() == num_removes && adds.len() == num_adds =>
                {
                    let resolved_slice = &input[resolved_start..conflict_start.unwrap()];
                    if !resolved_slice.is_empty() {
                        hunks.push(MergeHunk::Resolved(resolved_slice.to_vec()));
                    }
                    hunks.push(hunk);
                    resolved_start = pos + line.len();
                }
                _ => {}
            }
            conflict_start = None;
        }
        pos += line.len();
    }

    if hunks.is_empty() {
        None
    } else {
        if resolved_start < input.len() {
            hunks.push(MergeHunk::Resolved(input[resolved_start..].to_vec()));
        }
        Some(hunks)
    }
}

fn parse_conflict_hunk(input: &[u8]) -> MergeHunk {
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
                removes.push(vec![]);
                adds.push(vec![]);
                continue;
            }
            CONFLICT_MINUS_LINE => {
                state = State::Minus;
                removes.push(vec![]);
                continue;
            }
            CONFLICT_PLUS_LINE => {
                state = State::Plus;
                adds.push(vec![]);
                continue;
            }
            _ => {}
        };
        match state {
            State::Diff => {
                if let Some(rest) = line.strip_prefix(b"-") {
                    removes.last_mut().unwrap().extend_from_slice(rest);
                } else if let Some(rest) = line.strip_prefix(b"+") {
                    adds.last_mut().unwrap().extend_from_slice(rest);
                } else if let Some(rest) = line.strip_prefix(b" ") {
                    removes.last_mut().unwrap().extend_from_slice(rest);
                    adds.last_mut().unwrap().extend_from_slice(rest);
                } else {
                    // Doesn't look like a conflict
                    return MergeHunk::Resolved(vec![]);
                }
            }
            State::Minus => {
                removes.last_mut().unwrap().extend_from_slice(line);
            }
            State::Plus => {
                adds.last_mut().unwrap().extend_from_slice(line);
            }
            State::Unknown => {
                // Doesn't look like a conflict
                return MergeHunk::Resolved(vec![]);
            }
        }
    }

    MergeHunk::Conflict(ConflictHunk { removes, adds })
}

/// Returns `None` if there are no conflict markers in `content`.
pub fn update_conflict_from_content(
    store: &Store,
    path: &RepoPath,
    conflict_id: &ConflictId,
    content: &[u8],
) -> BackendResult<Option<ConflictId>> {
    let mut conflict = store.read_conflict(path, conflict_id)?;
    // TODO: Check that the conflict only involves files and convert it to a
    // `Conflict<Option<FileId>>` so we can remove the wildcard pattern in the loops
    // further down.

    // First check if the new content is unchanged compared to the old content. If
    // it is, we don't need parse the content or write any new objects to the
    // store. This is also a way of making sure that unchanged tree/file
    // conflicts (for example) are not converted to regular files in the working
    // copy.
    let mut old_content = Vec::with_capacity(content.len());
    materialize_conflict(store, path, &conflict, &mut old_content).unwrap();
    if content == old_content {
        return Ok(Some(conflict_id.clone()));
    }

    let mut removed_content = vec![vec![]; conflict.removes().len()];
    let mut added_content = vec![vec![]; conflict.adds().len()];
    // TODO: Change to let-else once our MSRV is above 1.65
    let hunks = if let Some(hunks) =
        parse_conflict(content, conflict.removes().len(), conflict.adds().len())
    {
        hunks
    } else {
        // Either there are no conflict markers of they don't have the expected arity
        return Ok(None);
    };
    for hunk in hunks {
        match hunk {
            MergeHunk::Resolved(slice) => {
                for buf in &mut removed_content {
                    buf.extend_from_slice(&slice);
                }
                for buf in &mut added_content {
                    buf.extend_from_slice(&slice);
                }
            }
            MergeHunk::Conflict(ConflictHunk { removes, adds }) => {
                for (i, buf) in removes.iter().enumerate() {
                    removed_content[i].extend_from_slice(buf);
                }
                for (i, buf) in adds.iter().enumerate() {
                    added_content[i].extend_from_slice(buf);
                }
            }
        }
    }
    // Now write the new files contents we found by parsing the file
    // with conflict markers. Update the Conflict object with the new
    // FileIds.
    for (i, buf) in removed_content.iter().enumerate() {
        match &conflict.removes()[i] {
            Some(TreeValue::File { id: _, executable }) => {
                let file_id = store.write_file(path, &mut buf.as_slice())?;
                let new_value = TreeValue::File {
                    id: file_id,
                    executable: *executable,
                };
                conflict.set_remove(i, Some(new_value));
            }
            None if buf.is_empty() => {
                // The missing side of a conflict is still represented by
                // the empty string we materialized it as => nothing to do
            }
            _ => {
                // The user edited a non-file side. This should never happen. We consider the
                // conflict resolved for now.
                return Ok(None);
            }
        }
    }
    for (i, buf) in added_content.iter().enumerate() {
        match &conflict.adds()[i] {
            Some(TreeValue::File { id: _, executable }) => {
                let file_id = store.write_file(path, &mut buf.as_slice())?;
                let new_value = TreeValue::File {
                    id: file_id,
                    executable: *executable,
                };
                conflict.set_add(i, Some(new_value));
            }
            None if buf.is_empty() => {
                // The missing side of a conflict is still represented by
                // the empty string we materialized it as => nothing to do
            }
            _ => {
                // The user edited a non-file side. This should never happen. We consider the
                // conflict resolved for now.
                return Ok(None);
            }
        }
    }
    let new_conflict_id = store.write_conflict(path, &conflict)?;
    Ok(Some(new_conflict_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{FileId, ObjectId};

    #[test]
    fn test_backend_conflict_conversion() {
        fn value(hex: &str) -> TreeValue {
            TreeValue::File {
                id: FileId::from_hex(hex),
                executable: false,
            }
        }
        fn term(hex: &str) -> backend::ConflictTerm {
            backend::ConflictTerm { value: value(hex) }
        }

        let test_roundtrip = |backend_conflict: &backend::Conflict| {
            let conflict = Conflict::from_backend_conflict(backend_conflict);
            assert_eq!(conflict.to_backend_conflict(), *backend_conflict);
            assert_eq!(
                Conflict::from_backend_conflict(&conflict.to_backend_conflict()),
                conflict
            );
        };

        // Regular 3-way conflict
        let backend_conflict = backend::Conflict {
            removes: vec![term("11")],
            adds: vec![term("22"), term("33")],
        };
        assert_eq!(
            Conflict::from_backend_conflict(&backend_conflict),
            Conflict {
                removes: vec![Some(value("11"))],
                adds: vec![Some(value("22")), Some(value("33"))],
            }
        );
        test_roundtrip(&backend_conflict);
        // Modify/delete conflict
        let backend_conflict = backend::Conflict {
            removes: vec![term("11")],
            adds: vec![term("22")],
        };
        assert_eq!(
            Conflict::from_backend_conflict(&backend_conflict),
            Conflict {
                removes: vec![Some(value("11"))],
                adds: vec![Some(value("22")), None],
            }
        );
        test_roundtrip(&backend_conflict);
        // Add/add conflict
        let backend_conflict = backend::Conflict {
            removes: vec![],
            adds: vec![term("11"), term("22")],
        };
        assert_eq!(
            Conflict::from_backend_conflict(&backend_conflict),
            Conflict {
                removes: vec![None],
                adds: vec![Some(value("11")), Some(value("22"))],
            }
        );
        test_roundtrip(&backend_conflict);
        // 5-way conflict
        let backend_conflict = backend::Conflict {
            removes: vec![term("11"), term("22")],
            adds: vec![term("33"), term("44"), term("55")],
        };
        assert_eq!(
            Conflict::from_backend_conflict(&backend_conflict),
            Conflict {
                removes: vec![Some(value("11")), Some(value("22"))],
                adds: vec![Some(value("33")), Some(value("44")), Some(value("55"))],
            }
        );
        test_roundtrip(&backend_conflict);
        // 5-way delete/delete conflict
        let backend_conflict = backend::Conflict {
            removes: vec![term("11"), term("22")],
            adds: vec![],
        };
        assert_eq!(
            Conflict::from_backend_conflict(&backend_conflict),
            Conflict {
                removes: vec![Some(value("11")), Some(value("22"))],
                adds: vec![None, None, None],
            }
        );
        test_roundtrip(&backend_conflict);
    }
}
