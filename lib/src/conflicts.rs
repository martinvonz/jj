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

#![allow(missing_docs)]

use std::io::{Read, Write};
use std::iter::zip;

use futures::StreamExt;
use itertools::Itertools;
use regex::bytes::Regex;

use crate::backend::{BackendError, BackendResult, CommitId, FileId, SymlinkId, TreeId, TreeValue};
use crate::diff::{find_line_ranges, Diff, DiffHunk};
use crate::files;
use crate::files::{ContentHunk, MergeResult};
use crate::merge::{Merge, MergeBuilder, MergedTreeValue};
use crate::repo_path::RepoPath;
use crate::store::Store;

const CONFLICT_START_LINE: &[u8] = b"<<<<<<<";
const CONFLICT_END_LINE: &[u8] = b">>>>>>>";
const CONFLICT_DIFF_LINE: &[u8] = b"%%%%%%%";
const CONFLICT_MINUS_LINE: &[u8] = b"-------";
const CONFLICT_PLUS_LINE: &[u8] = b"+++++++";
const CONFLICT_START_LINE_CHAR: u8 = CONFLICT_START_LINE[0];
const CONFLICT_END_LINE_CHAR: u8 = CONFLICT_END_LINE[0];
const CONFLICT_DIFF_LINE_CHAR: u8 = CONFLICT_DIFF_LINE[0];
const CONFLICT_MINUS_LINE_CHAR: u8 = CONFLICT_MINUS_LINE[0];
const CONFLICT_PLUS_LINE_CHAR: u8 = CONFLICT_PLUS_LINE[0];

/// A conflict marker is one of the separators, optionally followed by a space
/// and some text.
// TODO: All the `{7}` could be replaced with `{7,}` to allow longer
// separators. This could be useful to make it possible to allow conflict
// markers inside the text of the conflicts.
static CONFLICT_MARKER_REGEX: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
    Regex::new(
        r"(<{7}|>{7}|%{7}|\-{7}|\+{7})( .*)?
",
    )
    .unwrap()
});

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

async fn get_file_contents(store: &Store, path: &RepoPath, term: &Option<FileId>) -> ContentHunk {
    match term {
        Some(id) => {
            let mut content = vec![];
            store
                .read_file_async(path, id)
                .await
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

pub async fn extract_as_single_hunk(
    merge: &Merge<Option<FileId>>,
    store: &Store,
    path: &RepoPath,
) -> Merge<ContentHunk> {
    let builder: MergeBuilder<ContentHunk> = futures::stream::iter(merge.iter())
        .then(|term| get_file_contents(store, path, term))
        .collect()
        .await;
    builder.build()
}

pub async fn materialize(
    conflict: &MergedTreeValue,
    store: &Store,
    path: &RepoPath,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    if let Some(file_merge) = conflict.to_file_merge() {
        let content = extract_as_single_hunk(&file_merge, store, path).await;
        materialize_merge_result(&content, output)
    } else {
        // Unless all terms are regular files, we can't do much better than to try to
        // describe the merge.
        conflict.describe(output)
    }
}

/// A type similar to `MergedTreeValue` but with associated data to include in
/// e.g. the working copy or in a diff.
pub enum MaterializedTreeValue {
    Absent,
    AccessDenied(Box<dyn std::error::Error + Send + Sync>),
    File {
        id: FileId,
        executable: bool,
        reader: Box<dyn Read>,
    },
    Symlink {
        id: SymlinkId,
        target: String,
    },
    Conflict {
        id: MergedTreeValue,
        contents: Vec<u8>,
        executable: bool,
    },
    GitSubmodule(CommitId),
    Tree(TreeId),
}

impl MaterializedTreeValue {
    pub fn is_absent(&self) -> bool {
        matches!(self, MaterializedTreeValue::Absent)
    }

    pub fn is_present(&self) -> bool {
        !self.is_absent()
    }
}

/// Reads the data associated with a `MergedTreeValue` so it can be written to
/// e.g. the working copy or diff.
pub async fn materialize_tree_value(
    store: &Store,
    path: &RepoPath,
    value: MergedTreeValue,
) -> BackendResult<MaterializedTreeValue> {
    match materialize_tree_value_no_access_denied(store, path, value).await {
        Err(BackendError::ReadAccessDenied { source, .. }) => {
            Ok(MaterializedTreeValue::AccessDenied(source))
        }
        result => result,
    }
}

async fn materialize_tree_value_no_access_denied(
    store: &Store,
    path: &RepoPath,
    value: MergedTreeValue,
) -> BackendResult<MaterializedTreeValue> {
    match value.into_resolved() {
        Ok(None) => Ok(MaterializedTreeValue::Absent),
        Ok(Some(TreeValue::File { id, executable })) => {
            let reader = store.read_file_async(path, &id).await?;
            Ok(MaterializedTreeValue::File {
                id,
                executable,
                reader,
            })
        }
        Ok(Some(TreeValue::Symlink(id))) => {
            let target = store.read_symlink_async(path, &id).await?;
            Ok(MaterializedTreeValue::Symlink { id, target })
        }
        Ok(Some(TreeValue::GitSubmodule(id))) => Ok(MaterializedTreeValue::GitSubmodule(id)),
        Ok(Some(TreeValue::Tree(id))) => Ok(MaterializedTreeValue::Tree(id)),
        Ok(Some(TreeValue::Conflict(_))) => {
            panic!("cannot materialize legacy conflict object at path {path:?}");
        }
        Err(conflict) => {
            let mut contents = vec![];
            materialize(&conflict, store, path, &mut contents)
                .await
                .expect("Failed to materialize conflict to in-memory buffer");
            let executable = if let Some(merge) = conflict.to_executable_merge() {
                merge.resolve_trivial().copied().unwrap_or_default()
            } else {
                false
            };
            Ok(MaterializedTreeValue::Conflict {
                id: conflict,
                contents,
                executable,
            })
        }
    }
}

pub fn materialize_merge_result(
    single_hunk: &Merge<ContentHunk>,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    let slices = single_hunk.map(|content| content.0.as_slice());
    let merge_result = files::merge(&slices);
    match merge_result {
        MergeResult::Resolved(content) => {
            output.write_all(&content.0)?;
        }
        MergeResult::Conflict(hunks) => {
            let num_conflicts = hunks
                .iter()
                .filter(|hunk| hunk.as_resolved().is_none())
                .count();
            let mut conflict_index = 0;
            for hunk in hunks {
                if let Some(content) = hunk.as_resolved() {
                    output.write_all(&content.0)?;
                } else {
                    conflict_index += 1;
                    output.write_all(CONFLICT_START_LINE)?;
                    output.write_all(
                        format!(" Conflict {conflict_index} of {num_conflicts}\n").as_bytes(),
                    )?;
                    let mut add_index = 0;
                    for (base_index, left) in hunk.removes().enumerate() {
                        // The vast majority of conflicts one actually tries to
                        // resolve manually have 1 base.
                        let base_str = if hunk.removes().len() == 1 {
                            "base".to_string()
                        } else {
                            format!("base #{}", base_index + 1)
                        };

                        let right1 = if let Some(right1) = hunk.get_add(add_index) {
                            right1
                        } else {
                            // If we have no more positive terms, emit the remaining negative
                            // terms as snapshots.
                            output.write_all(CONFLICT_MINUS_LINE)?;
                            output.write_all(format!(" Contents of {base_str}\n").as_bytes())?;
                            output.write_all(&left.0)?;
                            continue;
                        };
                        let diff1 = Diff::for_tokenizer(&[&left.0, &right1.0], &find_line_ranges)
                            .hunks()
                            .collect_vec();
                        // Check if the diff against the next positive term is better. Since
                        // we want to preserve the order of the terms, we don't match against
                        // any later positive terms.
                        if let Some(right2) = hunk.get_add(add_index + 1) {
                            let diff2 =
                                Diff::for_tokenizer(&[&left.0, &right2.0], &find_line_ranges)
                                    .hunks()
                                    .collect_vec();
                            if diff_size(&diff2) < diff_size(&diff1) {
                                // If the next positive term is a better match, emit
                                // the current positive term as a snapshot and the next
                                // positive term as a diff.
                                output.write_all(CONFLICT_PLUS_LINE)?;
                                output.write_all(
                                    format!(" Contents of side #{}\n", add_index + 1).as_bytes(),
                                )?;
                                output.write_all(&right1.0)?;
                                output.write_all(CONFLICT_DIFF_LINE)?;
                                output.write_all(
                                    format!(
                                        " Changes from {base_str} to side #{}\n",
                                        add_index + 2
                                    )
                                    .as_bytes(),
                                )?;
                                write_diff_hunks(&diff2, output)?;
                                add_index += 2;
                                continue;
                            }
                        }

                        output.write_all(CONFLICT_DIFF_LINE)?;
                        output.write_all(
                            format!(" Changes from {base_str} to side #{}\n", add_index + 1)
                                .as_bytes(),
                        )?;
                        write_diff_hunks(&diff1, output)?;
                        add_index += 1;
                    }

                    //  Emit the remaining positive terms as snapshots.
                    for (add_index, slice) in hunk.adds().enumerate().skip(add_index) {
                        output.write_all(CONFLICT_PLUS_LINE)?;
                        output.write_all(
                            format!(" Contents of side #{}\n", add_index + 1).as_bytes(),
                        )?;
                        output.write_all(&slice.0)?;
                    }
                    output.write_all(CONFLICT_END_LINE)?;
                    output.write_all(
                        format!(" Conflict {conflict_index} of {num_conflicts} ends\n").as_bytes(),
                    )?;
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
/// conflict markers. The caller has to provide the expected number of merge
/// sides (adds). Conflict markers that are otherwise valid will be considered
/// invalid if they don't have the expected arity.
// TODO: "parse" is not usually the opposite of "materialize", so maybe we
// should rename them to "serialize" and "deserialize"?
pub fn parse_conflict(input: &[u8], num_sides: usize) -> Option<Vec<Merge<ContentHunk>>> {
    if input.is_empty() {
        return None;
    }
    let mut hunks = vec![];
    let mut pos = 0;
    let mut resolved_start = 0;
    let mut conflict_start = None;
    let mut conflict_start_len = 0;
    for line in input.split_inclusive(|b| *b == b'\n') {
        if CONFLICT_MARKER_REGEX.is_match_at(line, 0) {
            if line[0] == CONFLICT_START_LINE_CHAR {
                conflict_start = Some(pos);
                conflict_start_len = line.len();
            } else if conflict_start.is_some() && line[0] == CONFLICT_END_LINE_CHAR {
                let conflict_body = &input[conflict_start.unwrap() + conflict_start_len..pos];
                let hunk = parse_conflict_hunk(conflict_body);
                if hunk.num_sides() == num_sides {
                    let resolved_slice = &input[resolved_start..conflict_start.unwrap()];
                    if !resolved_slice.is_empty() {
                        hunks.push(Merge::resolved(ContentHunk(resolved_slice.to_vec())));
                    }
                    hunks.push(hunk);
                    resolved_start = pos + line.len();
                }
                conflict_start = None;
            }
        }
        pos += line.len();
    }

    if hunks.is_empty() {
        None
    } else {
        if resolved_start < input.len() {
            hunks.push(Merge::resolved(ContentHunk(
                input[resolved_start..].to_vec(),
            )));
        }
        Some(hunks)
    }
}

fn parse_conflict_hunk(input: &[u8]) -> Merge<ContentHunk> {
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
        if CONFLICT_MARKER_REGEX.is_match_at(line, 0) {
            match line[0] {
                CONFLICT_DIFF_LINE_CHAR => {
                    state = State::Diff;
                    removes.push(ContentHunk(vec![]));
                    adds.push(ContentHunk(vec![]));
                    continue;
                }
                CONFLICT_MINUS_LINE_CHAR => {
                    state = State::Minus;
                    removes.push(ContentHunk(vec![]));
                    continue;
                }
                CONFLICT_PLUS_LINE_CHAR => {
                    state = State::Plus;
                    adds.push(ContentHunk(vec![]));
                    continue;
                }
                _ => {}
            }
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
                    return Merge::resolved(ContentHunk(vec![]));
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
                return Merge::resolved(ContentHunk(vec![]));
            }
        }
    }

    Merge::from_removes_adds(removes, adds)
}

/// Parses conflict markers in `content` and returns an updated version of
/// `file_ids` with the new contents. If no (valid) conflict markers remain, a
/// single resolves `FileId` will be returned.
pub async fn update_from_content(
    file_ids: &Merge<Option<FileId>>,
    store: &Store,
    path: &RepoPath,
    content: &[u8],
) -> BackendResult<Merge<Option<FileId>>> {
    // First check if the new content is unchanged compared to the old content. If
    // it is, we don't need parse the content or write any new objects to the
    // store. This is also a way of making sure that unchanged tree/file
    // conflicts (for example) are not converted to regular files in the working
    // copy.
    let mut old_content = Vec::with_capacity(content.len());
    let merge_hunk = extract_as_single_hunk(file_ids, store, path).await;
    materialize_merge_result(&merge_hunk, &mut old_content).unwrap();
    if content == old_content {
        return Ok(file_ids.clone());
    }

    let Some(hunks) = parse_conflict(content, file_ids.num_sides()) else {
        // Either there are no self markers of they don't have the expected arity
        let file_id = store.write_file(path, &mut &content[..])?;
        return Ok(Merge::normal(file_id));
    };
    let mut contents = file_ids.map(|_| vec![]);
    for hunk in hunks {
        if let Some(slice) = hunk.as_resolved() {
            for content in contents.iter_mut() {
                content.extend_from_slice(&slice.0);
            }
        } else {
            for (content, slice) in zip(contents.iter_mut(), hunk.into_iter()) {
                content.extend(slice.0);
            }
        }
    }

    // If the user edited the empty placeholder for an absent side, we consider the
    // conflict resolved.
    if zip(contents.iter(), file_ids.iter())
        .any(|(content, file_id)| file_id.is_none() && !content.is_empty())
    {
        let file_id = store.write_file(path, &mut &content[..])?;
        return Ok(Merge::normal(file_id));
    }

    // Now write the new files contents we found by parsing the file with conflict
    // markers. Update the Merge object with the new FileIds.
    let builder: BackendResult<MergeBuilder<Option<FileId>>> =
        zip(contents.iter(), file_ids.iter())
            .map(|(content, file_id)| {
                match file_id {
                    Some(_) => {
                        let file_id = store.write_file(path, &mut content.as_slice())?;
                        Ok(Some(file_id))
                    }
                    None => {
                        // The missing side of a conflict is still represented by
                        // the empty string we materialized it as
                        Ok(None)
                    }
                }
            })
            .collect();
    Ok(builder?.build())
}
