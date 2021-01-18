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

use diff::slice as diff_slice;
use std::fmt::{Debug, Error, Formatter};

fn is_word_byte(a: u8) -> bool {
    a.is_ascii_alphanumeric() || a == b'_'
}

fn is_same_word(a: u8, b: u8) -> bool {
    // Don't allow utf-8 code points to be split into separate words
    (is_word_byte(a) && is_word_byte(b)) || a & 0x80 != 0
}

fn tokenize(data: &[u8]) -> Vec<&[u8]> {
    // TODO: Fix this code to not be so inefficient, and to allow the word
    // delimiter to be configured.
    let mut output = vec![];
    let mut word_start_pos = 0;
    let mut maybe_prev: Option<u8> = None;
    for (i, b) in data.iter().enumerate() {
        let b = *b;
        if let Some(prev) = maybe_prev {
            if !is_same_word(prev, b) {
                output.push(&data[word_start_pos..i]);
                word_start_pos = i;
            }
        }
        maybe_prev = Some(b);
    }
    if word_start_pos < data.len() {
        output.push(&data[word_start_pos..]);
    }
    output
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum DiffHunk<'a> {
    Unmodified(&'a [u8]),
    Added(&'a [u8]),
    Removed(&'a [u8]),
}

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
            .all(|hunk| matches!(hunk, DiffHunk::Unmodified(_)))
    }
}

pub fn diff<'a>(left: &'a [u8], right: &'a [u8], callback: &mut impl FnMut(&DiffLine<'a>)) {
    // TODO: Should we attempt to interpret as utf-8 and otherwise break only at
    // newlines?
    let left_tokens = tokenize(left);
    let right_tokens = tokenize(right);
    let result = diff_slice(&left_tokens, &right_tokens);
    let mut diff_line = DiffLine {
        left_line_number: 1,
        right_line_number: 1,
        has_left_content: false,
        has_right_content: false,
        hunks: vec![],
    };
    for hunk in result {
        match hunk {
            diff::Result::Both(left, right) => {
                assert!(left == right);
                diff_line.has_left_content = true;
                diff_line.has_right_content = true;
                diff_line.hunks.push(DiffHunk::Unmodified(left));
                if left == &[b'\n'] {
                    callback(&diff_line);
                    diff_line.left_line_number += 1;
                    diff_line.right_line_number += 1;
                    diff_line.reset_line();
                }
            }
            diff::Result::Left(left) => {
                diff_line.has_left_content = true;
                diff_line.hunks.push(DiffHunk::Removed(left));
                if left == &[b'\n'] {
                    callback(&diff_line);
                    diff_line.left_line_number += 1;
                    diff_line.reset_line();
                }
            }
            diff::Result::Right(right) => {
                diff_line.has_right_content = true;
                diff_line.hunks.push(DiffHunk::Added(right));
                if right == &[b'\n'] {
                    callback(&diff_line);
                    diff_line.right_line_number += 1;
                    diff_line.reset_line();
                }
            }
        }
    }
    if !diff_line.hunks.is_empty() {
        callback(&diff_line);
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

/// Returns None if the merge fails
pub fn merge(base: &[u8], left: &[u8], right: &[u8]) -> MergeResult {
    let base_tokens = tokenize(base);
    let left_tokens = tokenize(left);
    let right_tokens = tokenize(right);

    let left_diff = diff_slice(&base_tokens, &left_tokens);
    let right_diff = diff_slice(&base_tokens, &right_tokens);

    let mut hunk: Vec<u8> = vec![];
    let mut hunks: Vec<MergeHunk> = vec![];
    let mut left_it = left_diff.iter();
    let mut right_it = right_diff.iter();

    let mut left_hunk = left_it.next();
    let mut right_hunk = right_it.next();
    loop {
        match (left_hunk, right_hunk) {
            (None, None) => {
                break;
            }
            (Some(diff::Result::Both(left_data_before, left_data_after)), _)
                if left_data_before == left_data_after =>
            {
                // Left unmodified
                match right_hunk.unwrap() {
                    diff::Result::Both(right_data_before, right_data_after) => {
                        // Left unmodified, right modified
                        assert_eq!(left_data_before, right_data_before);
                        hunk.append(&mut right_data_after.to_vec());
                        left_hunk = left_it.next();
                        right_hunk = right_it.next();
                    }
                    diff::Result::Left(right_data_before) => {
                        // Left unmodified, right deleted
                        assert_eq!(left_data_before, right_data_before);
                        left_hunk = left_it.next();
                        right_hunk = right_it.next();
                    }
                    diff::Result::Right(right_data_after) => {
                        // Left unmodified, right inserted
                        hunk.append(&mut right_data_after.to_vec());
                        right_hunk = right_it.next();
                    }
                }
            }
            (_, Some(diff::Result::Both(right_data_before, right_data_after)))
                if right_data_before == right_data_after =>
            {
                // Right unmodified
                match left_hunk.unwrap() {
                    diff::Result::Both(left_data_before, left_data_after) => {
                        // Right unmodified, left modified
                        assert_eq!(left_data_before, right_data_before);
                        hunk.append(&mut left_data_after.to_vec());
                        left_hunk = left_it.next();
                        right_hunk = right_it.next();
                    }
                    diff::Result::Left(left_data_before) => {
                        // Right unmodified, left deleted
                        assert_eq!(left_data_before, right_data_before);
                        left_hunk = left_it.next();
                        right_hunk = right_it.next();
                    }
                    diff::Result::Right(left_data_after) => {
                        // Right unmodified, left inserted
                        hunk.append(&mut left_data_after.to_vec());
                        left_hunk = left_it.next();
                    }
                }
            }
            (
                Some(diff::Result::Left(left_data_before)),
                Some(diff::Result::Left(right_data_before)),
            ) => {
                // Both deleted the same
                assert_eq!(left_data_before, right_data_before);
                left_hunk = left_it.next();
                right_hunk = right_it.next();
            }
            (
                Some(diff::Result::Right(left_data_after)),
                Some(diff::Result::Right(right_data_after)),
            ) => {
                if left_data_after == right_data_after {
                    // Both inserted the same
                    hunk.append(&mut left_data_after.to_vec());
                } else {
                    // Each side inserted different
                    if !hunk.is_empty() {
                        hunks.push(MergeHunk::Resolved(hunk));
                    }
                    hunks.push(MergeHunk::Conflict {
                        base: vec![],
                        left: left_data_after.to_vec(),
                        right: right_data_after.to_vec(),
                    });
                    hunk = vec![];
                }
                left_hunk = left_it.next();
                right_hunk = right_it.next();
            }
            (Some(diff::Result::Right(left_data_after)), None) => {
                // Left inserted at EOF
                hunk.append(&mut left_data_after.to_vec());
                left_hunk = left_it.next();
            }
            (None, Some(diff::Result::Right(right_data_after))) => {
                // Right inserted at EOF
                hunk.append(&mut right_data_after.to_vec());
                right_hunk = right_it.next();
            }
            _ => {
                panic!("unhandled merge case: {:?}, {:?}", left_hunk, right_hunk);
            }
        }
    }
    if hunks.is_empty() {
        MergeResult::Resolved(hunk)
    } else {
        if !hunk.is_empty() {
            hunks.push(MergeHunk::Resolved(hunk));
        }
        MergeResult::Conflict(hunks)
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
            merge(b"a", b"a b", b"a c"),
            MergeResult::Conflict(vec![
                MergeHunk::Resolved(b"a ".to_vec()),
                MergeHunk::Conflict {
                    base: b"".to_vec(),
                    left: b"b".to_vec(),
                    right: b"c".to_vec()
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
        // TODO: It seems like the a->b transition get reported as [Left(a),Right(b)]
        // instead       of [Both(a,b)], so there is unexpectedly no conflict
        // here
        assert_eq!(merge(b"a", b"", b"b"), MergeResult::Resolved(b"b".to_vec()));
        assert_eq!(merge(b"a", b"b", b""), MergeResult::Resolved(b"b".to_vec()));
        assert_eq!(
            merge(b"a", b"b", b"c"),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                base: b"".to_vec(),
                left: b"b".to_vec(),
                right: b"c".to_vec()
            }])
        );
    }
}
