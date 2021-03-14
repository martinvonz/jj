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

use std::fmt::{Debug, Error, Formatter};
use std::ops::Range;

use diff::slice as diff_slice;

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

/// A region where the base and two sides match.
#[derive(Debug, PartialEq, Eq, Clone)]
struct SyncRegion {
    base: Range<usize>,
    left: Range<usize>,
    right: Range<usize>,
}

fn diff_result_lengths(diff: &diff::Result<&&[u8]>) -> (usize, usize) {
    match diff {
        diff::Result::Left(&left) => (left.len(), 0),
        diff::Result::Both(&left, &right) => (left.len(), right.len()),
        diff::Result::Right(&right) => (0, right.len()),
    }
}

fn unmodified_regions(
    left_tokens: &[&[u8]],
    right_tokens: &[&[u8]],
) -> Vec<(Range<usize>, Range<usize>)> {
    let diffs = diff_slice(left_tokens, right_tokens);
    let mut left_pos = 0;
    let mut right_pos = 0;
    let mut regions = Vec::new();
    for diff in diffs {
        let (left_len, right_len) = diff_result_lengths(&diff);
        match diff {
            diff::Result::Both(&left, &right) if left == right => regions.push((
                left_pos..left_pos + left_len,
                right_pos..right_pos + right_len,
            )),
            _ => {}
        }
        left_pos += left_len;
        right_pos += right_len;
    }
    regions
}

fn find_sync_regions(base: &[u8], left: &[u8], right: &[u8]) -> Vec<SyncRegion> {
    let base_tokens = tokenize(base);
    let left_tokens = tokenize(left);
    let right_tokens = tokenize(right);

    let left_regions = unmodified_regions(&base_tokens, &left_tokens);
    let right_regions = unmodified_regions(&base_tokens, &right_tokens);

    let mut left_it = left_regions.iter().peekable();
    let mut right_it = right_regions.iter().peekable();

    let mut regions: Vec<SyncRegion> = vec![];
    while let (Some((left_base_region, left_region)), Some((right_base_region, right_region))) =
        (left_it.peek(), right_it.peek())
    {
        // TODO: if left_base_region and right_base_region at least intersect, use the
        // intersection of the two regions.
        if left_base_region == right_base_region {
            regions.push(SyncRegion {
                base: left_base_region.clone(),
                left: left_region.clone(),
                right: right_region.clone(),
            });
            left_it.next().unwrap();
            right_it.next().unwrap();
        } else if left_base_region.start < right_base_region.start {
            left_it.next().unwrap();
        } else {
            right_it.next().unwrap();
        }
    }

    regions.push(SyncRegion {
        base: (base.len()..base.len()),
        left: (left.len()..left.len()),
        right: (right.len()..right.len()),
    });
    regions
}

pub fn merge(base: &[u8], left: &[u8], right: &[u8]) -> MergeResult {
    let mut previous_region = SyncRegion {
        base: 0..0,
        left: 0..0,
        right: 0..0,
    };
    let mut hunk: Vec<u8> = vec![];
    let mut hunks: Vec<MergeHunk> = vec![];
    // Find regions that match between base, left, and right. Emit the unchanged
    // regions as is. For the potentially conflicting regions between them, use
    // one side if the other is changed. If all three sides are different, emit
    // a conflict.
    for sync_region in find_sync_regions(base, left, right) {
        let base_conflict_slice = &base[previous_region.base.end..sync_region.base.start];
        let left_conflict_slice = &left[previous_region.left.end..sync_region.left.start];
        let right_conflict_slice = &right[previous_region.right.end..sync_region.right.start];
        if left_conflict_slice == base_conflict_slice || left_conflict_slice == right_conflict_slice
        {
            hunk.extend(right_conflict_slice);
        } else if right_conflict_slice == base_conflict_slice {
            hunk.extend(left_conflict_slice);
        } else {
            if !hunk.is_empty() {
                hunks.push(MergeHunk::Resolved(hunk));
                hunk = vec![];
            }
            hunks.push(MergeHunk::Conflict {
                base: base_conflict_slice.to_vec(),
                left: left_conflict_slice.to_vec(),
                right: right_conflict_slice.to_vec(),
            });
        }
        hunk.extend(base[sync_region.base.clone()].to_vec());
        previous_region = sync_region;
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
    fn test_find_sync_regions() {
        assert_eq!(
            find_sync_regions(b"", b"", b""),
            vec![SyncRegion {
                base: 0..0,
                left: 0..0,
                right: 0..0,
            }]
        );

        assert_eq!(
            find_sync_regions(b"a b c", b"a x b c", b"a b y c"),
            vec![
                SyncRegion {
                    base: 0..1,
                    left: 0..1,
                    right: 0..1
                },
                SyncRegion {
                    base: 1..2,
                    left: 1..2,
                    right: 1..2
                },
                SyncRegion {
                    base: 2..3,
                    left: 4..5,
                    right: 2..3
                },
                SyncRegion {
                    base: 3..4,
                    left: 5..6,
                    right: 3..4
                },
                SyncRegion {
                    base: 4..5,
                    left: 6..7,
                    right: 6..7
                },
                SyncRegion {
                    base: 5..5,
                    left: 7..7,
                    right: 7..7
                }
            ]
        );
    }

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
                MergeHunk::Resolved(b"a".to_vec()),
                MergeHunk::Conflict {
                    base: b"".to_vec(),
                    left: b" b".to_vec(),
                    right: b" c".to_vec()
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
