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

use std::io::Write;

use itertools::Itertools;

use crate::diff::{find_line_ranges, Diff, DiffHunk};
use crate::files;
use crate::files::{ContentHunk, MergeResult};
use crate::merge::Merge;

const CONFLICT_START_LINE: &[u8] = b"<<<<<<<\n";
const CONFLICT_END_LINE: &[u8] = b">>>>>>>\n";
const CONFLICT_DIFF_LINE: &[u8] = b"%%%%%%%\n";
const CONFLICT_MINUS_LINE: &[u8] = b"-------\n";
const CONFLICT_PLUS_LINE: &[u8] = b"+++++++\n";

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
    single_hunk: &Merge<ContentHunk>,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    let removed_slices = single_hunk
        .removes()
        .iter()
        .map(|hunk| hunk.0.as_slice())
        .collect_vec();
    let added_slices = single_hunk
        .adds()
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
) -> Option<Vec<Merge<ContentHunk>>> {
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
                    hunks.push(Merge::resolved(ContentHunk(resolved_slice.to_vec())));
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

    Merge::new(removes, adds)
}
