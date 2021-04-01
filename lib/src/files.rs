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

use crate::diff;

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
    let mut diff_line = DiffLine {
        left_line_number: 1,
        right_line_number: 1,
        has_left_content: false,
        has_right_content: false,
        hunks: vec![],
    };
    for hunk in diff::diff(left, right) {
        match hunk {
            diff::SliceDiff::Unchanged(text) => {
                let lines = text.split_inclusive(|b| *b == b'\n');
                for line in lines {
                    diff_line.has_left_content = true;
                    diff_line.has_right_content = true;
                    diff_line.hunks.push(DiffHunk::Unmodified(line));
                    if line.ends_with(b"\n") {
                        callback(&diff_line);
                        diff_line.left_line_number += 1;
                        diff_line.right_line_number += 1;
                        diff_line.reset_line();
                    }
                }
            }
            diff::SliceDiff::Replaced(left, right) => {
                let left_lines = left.split_inclusive(|b| *b == b'\n');
                for left_line in left_lines {
                    diff_line.has_left_content = true;
                    diff_line.hunks.push(DiffHunk::Removed(left_line));
                    if left_line.ends_with(b"\n") {
                        callback(&diff_line);
                        diff_line.left_line_number += 1;
                        diff_line.reset_line();
                    }
                }
                let right_lines = right.split_inclusive(|b| *b == b'\n');
                for right_line in right_lines {
                    diff_line.has_right_content = true;
                    diff_line.hunks.push(DiffHunk::Added(right_line));
                    if right_line.ends_with(b"\n") {
                        callback(&diff_line);
                        diff_line.right_line_number += 1;
                        diff_line.reset_line();
                    }
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

fn find_sync_regions(base: &[u8], left: &[u8], right: &[u8]) -> Vec<SyncRegion> {
    let base_tokens = crate::diff::find_line_ranges(base);
    let left_tokens = crate::diff::find_line_ranges(left);
    let right_tokens = crate::diff::find_line_ranges(right);

    let left_regions = crate::diff::unchanged_ranges(base, left, &base_tokens, &left_tokens);
    let right_regions = crate::diff::unchanged_ranges(base, right, &base_tokens, &right_tokens);

    let mut left_it = left_regions.iter().peekable();
    let mut right_it = right_regions.iter().peekable();

    let mut regions: Vec<SyncRegion> = vec![];
    while let (Some((left_base_range, left_range)), Some((right_base_range, right_range))) =
        (left_it.peek(), right_it.peek())
    {
        // TODO: if left_base_range and right_base_range at least intersect, use the
        // intersection of the two regions.
        if left_base_range == right_base_range {
            regions.push(SyncRegion {
                base: left_base_range.clone(),
                left: left_range.clone(),
                right: right_range.clone(),
            });
            left_it.next().unwrap();
            right_it.next().unwrap();
        } else if left_base_range.start < right_base_range.start {
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
            find_sync_regions(b"a\nb\nc\n", b"a\nx\nb\nc\n", b"a\nb\ny\nc\n"),
            vec![
                SyncRegion {
                    base: 0..2,
                    left: 0..2,
                    right: 0..2
                },
                SyncRegion {
                    base: 2..4,
                    left: 4..6,
                    right: 2..4
                },
                SyncRegion {
                    base: 4..6,
                    left: 6..8,
                    right: 6..8
                },
                SyncRegion {
                    base: 6..6,
                    left: 8..8,
                    right: 8..8
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
