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

use std::collections::VecDeque;
use std::fmt::{Debug, Error, Formatter};

use itertools::Itertools;

use crate::diff;
use crate::diff::{Diff, DiffHunk};
use crate::merge::{trivial_merge, Merge};

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
                    let left_lines = contents[0].split_inclusive(|b| *b == b'\n');
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
                    let right_lines = contents[1].split_inclusive(|b| *b == b'\n');
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
pub struct ContentHunk(pub Vec<u8>);

impl Debug for ContentHunk {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        String::from_utf8_lossy(&self.0).fmt(f)
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum MergeResult {
    Resolved(ContentHunk),
    Conflict(Vec<Merge<ContentHunk>>),
}

pub fn merge(slices: &Merge<&[u8]>) -> MergeResult {
    // TODO: Using the first remove as base (first in the inputs) is how it's
    // usually done for 3-way conflicts. Are there better heuristics when there are
    // more than 3 parts?
    let num_diffs = slices.removes().len();
    let diff_inputs = slices.removes().chain(slices.adds()).copied().collect_vec();

    let diff = Diff::for_tokenizer(&diff_inputs, &diff::find_line_ranges);
    let mut resolved_hunk = ContentHunk(vec![]);
    let mut merge_hunks: Vec<Merge<ContentHunk>> = vec![];
    for diff_hunk in diff.hunks() {
        match diff_hunk {
            DiffHunk::Matching(content) => {
                resolved_hunk.0.extend(content);
            }
            DiffHunk::Different(parts) => {
                if let Some(resolved) = trivial_merge(&parts[..num_diffs], &parts[num_diffs..]) {
                    resolved_hunk.0.extend(*resolved);
                } else {
                    if !resolved_hunk.0.is_empty() {
                        merge_hunks.push(Merge::resolved(resolved_hunk));
                        resolved_hunk = ContentHunk(vec![]);
                    }
                    merge_hunks.push(Merge::from_removes_adds(
                        parts[..num_diffs]
                            .iter()
                            .map(|part| ContentHunk(part.to_vec())),
                        parts[num_diffs..]
                            .iter()
                            .map(|part| ContentHunk(part.to_vec())),
                    ));
                }
            }
        }
    }

    if merge_hunks.is_empty() {
        MergeResult::Resolved(resolved_hunk)
    } else {
        if !resolved_hunk.0.is_empty() {
            merge_hunks.push(Merge::resolved(resolved_hunk));
        }
        MergeResult::Conflict(merge_hunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(data: &[u8]) -> ContentHunk {
        ContentHunk(data.to_vec())
    }

    fn merge(removes: &[&[u8]], adds: &[&[u8]]) -> MergeResult {
        super::merge(&Merge::from_removes_adds(removes.to_vec(), adds.to_vec()))
    }

    #[test]
    fn test_merge_single_hunk() {
        // Unchanged and empty on all sides
        assert_eq!(merge(&[b""], &[b"", b""]), MergeResult::Resolved(hunk(b"")));
        // Unchanged on all sides
        assert_eq!(
            merge(&[b"a"], &[b"a", b"a"]),
            MergeResult::Resolved(hunk(b"a"))
        );
        // One side removed, one side unchanged
        assert_eq!(
            merge(&[b"a\n"], &[b"", b"a\n"]),
            MergeResult::Resolved(hunk(b""))
        );
        // One side unchanged, one side removed
        assert_eq!(
            merge(&[b"a\n"], &[b"a\n", b""]),
            MergeResult::Resolved(hunk(b""))
        );
        // Both sides removed same line
        assert_eq!(
            merge(&[b"a\n"], &[b"", b""]),
            MergeResult::Resolved(hunk(b""))
        );
        // One side modified, one side unchanged
        assert_eq!(
            merge(&[b"a"], &[b"a b", b"a"]),
            MergeResult::Resolved(hunk(b"a b"))
        );
        // One side unchanged, one side modified
        assert_eq!(
            merge(&[b"a"], &[b"a", b"a b"]),
            MergeResult::Resolved(hunk(b"a b"))
        );
        // All sides added same content
        assert_eq!(
            merge(&[b"", b""], &[b"a\n", b"a\n", b"a\n"]),
            MergeResult::Resolved(hunk(b"a\n"))
        );
        // One side modified, two sides added
        assert_eq!(
            merge(&[b"a", b""], &[b"b", b"b", b"b"]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a"), hunk(b"")],
                vec![hunk(b"b"), hunk(b"b"), hunk(b"b")]
            )])
        );
        // All sides removed same content
        assert_eq!(
            merge(&[b"a\n", b"a\n", b"a\n"], &[b"", b"", b"", b""]),
            MergeResult::Resolved(hunk(b""))
        );
        // One side modified, two sides removed
        assert_eq!(
            merge(&[b"a\n", b"a\n"], &[b"b\n", b"", b""]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a\n"), hunk(b"a\n")],
                vec![hunk(b"b\n"), hunk(b""), hunk(b"")]
            )])
        );
        // Three sides made the same change
        assert_eq!(
            merge(&[b"a", b"a"], &[b"b", b"b", b"b"]),
            MergeResult::Resolved(hunk(b"b"))
        );
        // One side removed, one side modified
        assert_eq!(
            merge(&[b"a\n"], &[b"", b"b\n"]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a\n")],
                vec![hunk(b""), hunk(b"b\n")]
            )])
        );
        // One side modified, one side removed
        assert_eq!(
            merge(&[b"a\n"], &[b"b\n", b""]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a\n")],
                vec![hunk(b"b\n"), hunk(b"")]
            )])
        );
        // Two sides modified in different ways
        assert_eq!(
            merge(&[b"a"], &[b"b", b"c"]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a")],
                vec![hunk(b"b"), hunk(b"c")]
            )])
        );
        // Two of three sides don't change, third side changes
        assert_eq!(
            merge(&[b"a", b"a"], &[b"a", b"", b"a"]),
            MergeResult::Resolved(hunk(b""))
        );
        // One side unchanged, two other sides make the same change
        assert_eq!(
            merge(&[b"a", b"a"], &[b"b", b"a", b"b"]),
            MergeResult::Resolved(hunk(b"b"))
        );
        // One side unchanged, two other sides make the different change
        assert_eq!(
            merge(&[b"a", b"a"], &[b"b", b"a", b"c"]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a"), hunk(b"a")],
                vec![hunk(b"b"), hunk(b"a"), hunk(b"c")]
            )])
        );
        // Merge of an unresolved conflict and another branch, where the other branch
        // undid the change from one of the inputs to the unresolved conflict in the
        // first.
        assert_eq!(
            merge(&[b"a", b"b"], &[b"b", b"a", b"c"]),
            MergeResult::Resolved(hunk(b"c"))
        );
        // Merge of an unresolved conflict and another branch.
        assert_eq!(
            merge(&[b"a", b"b"], &[b"c", b"d", b"e"]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a"), hunk(b"b")],
                vec![hunk(b"c"), hunk(b"d"), hunk(b"e")]
            )])
        );
        // Two sides made the same change, third side made a different change
        assert_eq!(
            merge(&[b"a", b"b"], &[b"c", b"c", b"c"]),
            MergeResult::Conflict(vec![Merge::from_removes_adds(
                vec![hunk(b"a"), hunk(b"b")],
                vec![hunk(b"c"), hunk(b"c"), hunk(b"c")]
            )])
        );
    }

    #[test]
    fn test_merge_multi_hunk() {
        // Two sides left one line unchanged, and added conflicting additional lines
        assert_eq!(
            merge(&[b"a\n"], &[b"a\nb\n", b"a\nc\n"]),
            MergeResult::Conflict(vec![
                Merge::resolved(hunk(b"a\n")),
                Merge::from_removes_adds(vec![hunk(b"")], vec![hunk(b"b\n"), hunk(b"c\n")])
            ])
        );
        // Two sides changed different lines: no conflict
        assert_eq!(
            merge(&[b"a\nb\nc\n"], &[b"a2\nb\nc\n", b"a\nb\nc2\n"]),
            MergeResult::Resolved(hunk(b"a2\nb\nc2\n"))
        );
        // Conflict with non-conflicting lines around
        assert_eq!(
            merge(&[b"a\nb\nc\n"], &[b"a\nb1\nc\n", b"a\nb2\nc\n"]),
            MergeResult::Conflict(vec![
                Merge::resolved(hunk(b"a\n")),
                Merge::from_removes_adds(vec![hunk(b"b\n")], vec![hunk(b"b1\n"), hunk(b"b2\n")]),
                Merge::resolved(hunk(b"c\n"))
            ])
        );
        // One side changes a line and adds a block after. The other side just adds the
        // same block. This currently behaves as one would reasonably hope, but
        // it's likely that it will change if when we fix
        // https://github.com/martinvonz/jj/issues/761. Git and Mercurial both duplicate
        // the block in the result.
        assert_eq!(
            merge(
                &[b"\
a {
    p
}
"],
                &[
                    b"\
a {
    q
}

b {
    x
}
",
                    b"\
a {
    p
}

b {
    x
}
"
                ],
            ),
            MergeResult::Resolved(hunk(
                b"\
a {
    q
}

b {
    x
}
"
            ))
        );
    }
}
