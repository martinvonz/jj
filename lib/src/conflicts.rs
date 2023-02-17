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
use std::io::{Cursor, Write};

use itertools::Itertools;

use crate::backend::{BackendResult, Conflict, ConflictId, ConflictTerm, ObjectId, TreeValue};
use crate::diff::{find_line_ranges, Diff, DiffHunk};
use crate::files;
use crate::files::{ConflictHunk, MergeHunk, MergeResult};
use crate::repo_path::RepoPath;
use crate::store::Store;

const CONFLICT_START_LINE: &[u8] = b"<<<<<<<\n";
const CONFLICT_END_LINE: &[u8] = b">>>>>>>\n";
const CONFLICT_DIFF_LINE: &[u8] = b"%%%%%%%\n";
const CONFLICT_MINUS_LINE: &[u8] = b"-------\n";
const CONFLICT_PLUS_LINE: &[u8] = b"+++++++\n";

fn describe_conflict_term(term: &ConflictTerm) -> String {
    match &term.value {
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

/// Give a summary description of a conflict's "adds" and "removes"
pub fn describe_conflict(conflict: &Conflict, file: &mut dyn Write) -> std::io::Result<()> {
    file.write_all(b"Conflict:\n")?;
    for term in &conflict.terms {
        let action = if term.negative { "Removing" } else { "Adding" };
        file.write_all(format!("  {action} {}\n", describe_conflict_term(term)).as_bytes())?;
    }
    Ok(())
}

fn file_terms(terms: &[ConflictTerm]) -> Vec<&ConflictTerm> {
    terms
        .iter()
        .filter(|term| {
            matches!(
                term.value,
                TreeValue::File {
                    executable: false,
                    ..
                }
            )
        })
        .collect_vec()
}

fn get_file_contents(store: &Store, path: &RepoPath, term: &ConflictTerm) -> Vec<u8> {
    if let TreeValue::File {
        id,
        executable: false,
    } = &term.value
    {
        let mut content: Vec<u8> = vec![];
        store
            .read_file(path, id)
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        content
    } else {
        panic!("unexpectedly got a non-file conflict term");
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
    conflict: &Conflict,
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
    conflict: &Conflict,
) -> Option<ConflictHunk> {
    let file_terms = file_terms(&conflict.terms);
    if file_terms.len() != conflict.terms.len() {
        return None;
    }
    let mut added_content = file_terms
        .iter()
        .filter(|term| !term.negative)
        .map(|term| get_file_contents(store, path, term))
        .collect_vec();
    let mut removed_content = file_terms
        .iter()
        .filter(|term| term.negative)
        .map(|term| get_file_contents(store, path, term))
        .collect_vec();
    // If the conflict had removed the file on one side, we pretend that the file
    // was empty there.
    let l = max(added_content.len(), removed_content.len() + 1);
    added_content.resize(l, vec![]);
    removed_content.resize(l - 1, vec![]);

    Some(ConflictHunk {
        removes: removed_content,
        adds: added_content,
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

pub fn conflict_to_materialized_value(
    store: &Store,
    path: &RepoPath,
    conflict: &Conflict,
) -> TreeValue {
    let mut buf = vec![];
    materialize_conflict(store, path, conflict, &mut buf).unwrap();
    let file_id = store.write_file(path, &mut Cursor::new(&buf)).unwrap();
    TreeValue::File {
        id: file_id,
        executable: false,
    }
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

    let mut reconstructed_content = vec![vec![]; conflict.terms.len()];
    if let Some(hunks) = parse_conflict(content, conflict.num_negative(), conflict.num_positive()) {
        for hunk in hunks {
            match hunk {
                MergeHunk::Resolved(slice) => {
                    for buf in &mut reconstructed_content {
                        buf.extend_from_slice(&slice);
                    }
                }
                MergeHunk::Conflict(ConflictHunk { removes, adds }) => {
                    assert_eq!(conflict.terms.len(), removes.len() + adds.len());
                    let mut removes_iter = removes.iter();
                    let mut adds_iter = adds.iter();
                    for (i, term) in conflict.terms.iter().enumerate() {
                        if term.negative {
                            reconstructed_content[i]
                                .extend_from_slice(removes_iter.next().unwrap());
                        } else {
                            reconstructed_content[i].extend_from_slice(adds_iter.next().unwrap());
                        }
                    }
                }
            }
        }
        // Now write the new files contents we found by parsing the file
        // with conflict markers. Update the Conflict object with the new
        // FileIds.
        for (i, term) in conflict.terms.iter_mut().enumerate() {
            let file_id = store.write_file(path, &mut Cursor::new(&reconstructed_content[i]))?;
            if let TreeValue::File { id, executable: _ } = &mut term.value {
                *id = file_id;
            } else {
                // TODO: This can actually happen. We should check earlier
                // that the we only attempt to parse the conflicts if it's a
                // file-only conflict.
                panic!("Found conflict markers in merge of non-files");
            }
        }
        let new_conflict_id = store.write_conflict(path, &conflict)?;
        Ok(Some(new_conflict_id))
    } else {
        Ok(None)
    }
}
