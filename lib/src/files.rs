// Copyright 2020 Google LLC
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

use std::collections::VecDeque;
use std::fmt::{Debug, Error, Formatter};
use std::ops::Range;

use crate::diff;
use crate::diff::{Diff, DiffHunk};

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DiffLine<'a> {
    pub left_line_number: u32,
    pub right_line_number: u32,
    pub has_left_content: bool,
    pub has_right_content: bool,
    pub hunks: Vec<DiffHunk<'a>>,
}

impl DiffLine<'_> {
    fn reset_line(&mut self) {
        self.has_left_content = false;
        self.has_right_content = false;
        self.hunks.clear();
    }

    pub fn is_unmodified(&self) -> bool {
        self.hunks
            .iter()
            .all(|hunk| matches!(hunk, DiffHunk::Matching(_)))
    }
}

pub fn diff<'a>(left: &'a [u8], right: &'a [u8]) -> DiffLineIterator<'a> {
    let diff_hunks = diff::diff(left, right);
    DiffLineIterator::new(diff_hunks)
}

pub struct DiffLineIterator<'a> {
    diff_hunks: Vec<DiffHunk<'a>>,
    current_pos: usize,
    current_line: DiffLine<'a>,
    queued_lines: VecDeque<DiffLine<'a>>,
}

impl<'a> DiffLineIterator<'a> {
    fn new(diff_hunks: Vec<DiffHunk<'a>>) -> Self {
        let current_line = DiffLine {
            left_line_number: 1,
            right_line_number: 1,
            has_left_content: false,
            has_right_content: false,
            hunks: vec![],
        };
        DiffLineIterator {
            diff_hunks,
            current_pos: 0,
            current_line,
            queued_lines: VecDeque::new(),
        }
    }
}

impl<'a> Iterator for DiffLineIterator<'a> {
    type Item = DiffLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: Should we attempt to interpret as utf-8 and otherwise break only at
        // newlines?
        while self.current_pos < self.diff_hunks.len() && self.queued_lines.is_empty() {
            let hunk = &self.diff_hunks[self.current_pos];
            self.current_pos += 1;
            match hunk {
                diff::DiffHunk::Matching(text) => {
                    let lines = text.split_inclusive(|b| *b == b'\n');
                    for line in lines {
                        self.current_line.has_left_content = true;
                        self.current_line.has_right_content = true;
                        self.current_line.hunks.push(DiffHunk::Matching(line));
                        if line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.clone());
                            self.current_line.left_line_number += 1;
                            self.current_line.right_line_number += 1;
                            self.current_line.reset_line();
                        }
                    }
                }
                diff::DiffHunk::Different(contents) => {
                    let left = contents[0];
                    let right = contents[1];
                    let left_lines = left.split_inclusive(|b| *b == b'\n');
                    for left_line in left_lines {
                        self.current_line.has_left_content = true;
                        self.current_line
                            .hunks
                            .push(DiffHunk::Different(vec![left_line, b""]));
                        if left_line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.clone());
                            self.current_line.left_line_number += 1;
                            self.current_line.reset_line();
                        }
                    }
                    let right_lines = right.split_inclusive(|b| *b == b'\n');
                    for right_line in right_lines {
                        self.current_line.has_right_content = true;
                        self.current_line
                            .hunks
                            .push(DiffHunk::Different(vec![b"", right_line]));
                        if right_line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.clone());
                            self.current_line.right_line_number += 1;
                            self.current_line.reset_line();
                        }
                    }
                }
            }
        }

        if let Some(line) = self.queued_lines.pop_front() {
            return Some(line);
        }

        if !self.current_line.hunks.is_empty() {
            let line = self.current_line.clone();
            self.current_line.reset_line();
            return Some(line);
        }

        None
    }
}

#[derive(PartialEq, Eq, Clone)]
pub enum MergeHunk {
    Resolved(Vec<u8>),
    Conflict {
        base: Vec<u8>,
        left: Vec<u8>,
        right: Vec<u8>,
    },
}

impl Debug for MergeHunk {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            MergeHunk::Resolved(data) => f
                .debug_tuple("Resolved")
                .field(&String::from_utf8_lossy(data))
                .finish(),
            MergeHunk::Conflict { base, left, right } => f
                .debug_struct("Conflict")
                .field("base", &String::from_utf8_lossy(base))
                .field("left", &String::from_utf8_lossy(left))
                .field("right", &String::from_utf8_lossy(right))
                .finish(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum MergeResult {
    Resolved(Vec<u8>),
    Conflict(Vec<MergeHunk>),
}

/// A region where the base and two sides match.
#[derive(Debug, PartialEq, Eq, Clone)]
struct SyncRegion {
    base: Range<usize>,
    left: Range<usize>,
    right: Range<usize>,
}

// TODO: Update callers to use diff::Diff directly instead.
pub fn merge(base: &[u8], left: &[u8], right: &[u8]) -> MergeResult {
    let diff = Diff::for_tokenizer(&[base, left, right], &diff::find_line_ranges);
    let mut resolved_hunk: Vec<u8> = vec![];
    let mut merge_hunks: Vec<MergeHunk> = vec![];
    for diff_hunk in diff.hunks() {
        match diff_hunk {
            DiffHunk::Matching(content) => {
                resolved_hunk.extend(content);
            }
            DiffHunk::Different(content) => {
                let base_content = content[0];
                let left_content = content[1];
                let right_content = content[2];
                if left_content == base_content || left_content == right_content {
                    resolved_hunk.extend(right_content);
                } else if right_content == base_content {
                    resolved_hunk.extend(left_content);
                } else {
                    if !resolved_hunk.is_empty() {
                        merge_hunks.push(MergeHunk::Resolved(resolved_hunk));
                        resolved_hunk = vec![];
                    }
                    merge_hunks.push(MergeHunk::Conflict {
                        base: base_content.to_vec(),
                        left: left_content.to_vec(),
                        right: right_content.to_vec(),
                    });
                }
            }
        }
    }

    if merge_hunks.is_empty() {
        MergeResult::Resolved(resolved_hunk)
    } else {
        if !resolved_hunk.is_empty() {
            merge_hunks.push(MergeHunk::Resolved(resolved_hunk));
        }
        MergeResult::Conflict(merge_hunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge() {
        assert_eq!(merge(b"", b"", b""), MergeResult::Resolved(b"".to_vec()));
        assert_eq!(
            merge(b"a", b"a", b"a"),
            MergeResult::Resolved(b"a".to_vec())
        );
        assert_eq!(merge(b"a", b"", b"a"), MergeResult::Resolved(b"".to_vec()));
        assert_eq!(merge(b"a", b"a", b""), MergeResult::Resolved(b"".to_vec()));
        assert_eq!(merge(b"a", b"", b""), MergeResult::Resolved(b"".to_vec()));
        assert_eq!(
            merge(b"a", b"a b", b"a"),
            MergeResult::Resolved(b"a b".to_vec())
        );
        assert_eq!(
            merge(b"a", b"a", b"a b"),
            MergeResult::Resolved(b"a b".to_vec())
        );
        assert_eq!(
            merge(b"a\n", b"a\nb\n", b"a\nc\n"),
            MergeResult::Conflict(vec![
                MergeHunk::Resolved(b"a\n".to_vec()),
                MergeHunk::Conflict {
                    base: b"".to_vec(),
                    left: b"b\n".to_vec(),
                    right: b"c\n".to_vec()
                }
            ])
        );
        assert_eq!(
            merge(b"a", b"b", b"a"),
            MergeResult::Resolved(b"b".to_vec())
        );
        assert_eq!(
            merge(b"a", b"a", b"b"),
            MergeResult::Resolved(b"b".to_vec())
        );
        assert_eq!(
            merge(b"a", b"", b"b"),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                base: b"a".to_vec(),
                left: b"".to_vec(),
                right: b"b".to_vec()
            }])
        );
        assert_eq!(
            merge(b"a", b"b", b""),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                base: b"a".to_vec(),
                left: b"b".to_vec(),
                right: b"".to_vec()
            }])
        );
        assert_eq!(
            merge(b"a", b"b", b"c"),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                base: b"a".to_vec(),
                left: b"b".to_vec(),
                right: b"c".to_vec()
            }])
        );
    }
}
