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

use std::borrow::Borrow;
use std::collections::VecDeque;
use std::iter;
use std::mem;

use bstr::BStr;
use bstr::BString;

use crate::diff::Diff;
use crate::diff::DiffHunk;
use crate::merge::trivial_merge;
use crate::merge::Merge;

/// A diff line which may contain small hunks originating from both sides.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DiffLine<'a> {
    pub line_number: DiffLineNumber,
    pub hunks: Vec<(DiffLineHunkSide, &'a BStr)>,
}

impl DiffLine<'_> {
    pub fn has_left_content(&self) -> bool {
        self.hunks
            .iter()
            .any(|&(side, _)| side != DiffLineHunkSide::Right)
    }

    pub fn has_right_content(&self) -> bool {
        self.hunks
            .iter()
            .any(|&(side, _)| side != DiffLineHunkSide::Left)
    }

    pub fn is_unmodified(&self) -> bool {
        self.hunks
            .iter()
            .all(|&(side, _)| side == DiffLineHunkSide::Both)
    }

    fn take(&mut self) -> Self {
        DiffLine {
            line_number: self.line_number,
            hunks: mem::take(&mut self.hunks),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiffLineNumber {
    pub left: u32,
    pub right: u32,
}

/// Which side a `DiffLine` hunk belongs to?
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineHunkSide {
    Both,
    Left,
    Right,
}

pub struct DiffLineIterator<'a, I> {
    diff_hunks: iter::Fuse<I>,
    current_line: DiffLine<'a>,
    queued_lines: VecDeque<DiffLine<'a>>,
}

impl<'a, I> DiffLineIterator<'a, I>
where
    I: Iterator,
    I::Item: Borrow<DiffHunk<'a>>,
{
    /// Iterates `diff_hunks` by line. Each hunk should have exactly two inputs.
    pub fn new(diff_hunks: I) -> Self {
        let line_number = DiffLineNumber { left: 1, right: 1 };
        Self::with_line_number(diff_hunks, line_number)
    }

    /// Iterates `diff_hunks` by line. Each hunk should have exactly two inputs.
    /// Hunk's line numbers start from the given `line_number`.
    pub fn with_line_number(diff_hunks: I, line_number: DiffLineNumber) -> Self {
        let current_line = DiffLine {
            line_number,
            hunks: vec![],
        };
        DiffLineIterator {
            diff_hunks: diff_hunks.fuse(),
            current_line,
            queued_lines: VecDeque::new(),
        }
    }
}

impl<'a, I> DiffLineIterator<'a, I> {
    /// Returns line number of the next hunk. After all hunks are consumed, this
    /// returns the next line number if the last hunk ends with newline.
    pub fn next_line_number(&self) -> DiffLineNumber {
        let next_line = self.queued_lines.front().unwrap_or(&self.current_line);
        next_line.line_number
    }
}

impl<'a, I> Iterator for DiffLineIterator<'a, I>
where
    I: Iterator,
    I::Item: Borrow<DiffHunk<'a>>,
{
    type Item = DiffLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: Should we attempt to interpret as utf-8 and otherwise break only at
        // newlines?
        while self.queued_lines.is_empty() {
            let Some(hunk) = self.diff_hunks.next() else {
                break;
            };
            match hunk.borrow() {
                DiffHunk::Matching(text) => {
                    let lines = text.split_inclusive(|b| *b == b'\n').map(BStr::new);
                    for line in lines {
                        self.current_line.hunks.push((DiffLineHunkSide::Both, line));
                        if line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.take());
                            self.current_line.line_number.left += 1;
                            self.current_line.line_number.right += 1;
                        }
                    }
                }
                DiffHunk::Different(contents) => {
                    let [left_text, right_text] = contents[..]
                        .try_into()
                        .expect("hunk should have exactly two inputs");
                    let left_lines = left_text.split_inclusive(|b| *b == b'\n').map(BStr::new);
                    for left_line in left_lines {
                        self.current_line
                            .hunks
                            .push((DiffLineHunkSide::Left, left_line));
                        if left_line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.take());
                            self.current_line.line_number.left += 1;
                        }
                    }
                    let right_lines = right_text.split_inclusive(|b| *b == b'\n').map(BStr::new);
                    for right_line in right_lines {
                        self.current_line
                            .hunks
                            .push((DiffLineHunkSide::Right, right_line));
                        if right_line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.take());
                            self.current_line.line_number.right += 1;
                        }
                    }
                }
            }
        }

        if let Some(line) = self.queued_lines.pop_front() {
            return Some(line);
        }

        if !self.current_line.hunks.is_empty() {
            return Some(self.current_line.take());
        }

        None
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum MergeResult {
    Resolved(BString),
    Conflict(Vec<Merge<BString>>),
}

pub fn merge<T: AsRef<[u8]>>(slices: &Merge<T>) -> MergeResult {
    // TODO: Using the first remove as base (first in the inputs) is how it's
    // usually done for 3-way conflicts. Are there better heuristics when there are
    // more than 3 parts?
    let num_diffs = slices.removes().len();
    let diff_inputs = slices.removes().chain(slices.adds());
    merge_hunks(&Diff::by_line(diff_inputs), num_diffs)
}

fn merge_hunks(diff: &Diff, num_diffs: usize) -> MergeResult {
    let mut resolved_hunk = BString::new(vec![]);
    let mut merge_hunks: Vec<Merge<BString>> = vec![];
    for diff_hunk in diff.hunks() {
        match diff_hunk {
            DiffHunk::Matching(content) => {
                resolved_hunk.extend_from_slice(content);
            }
            DiffHunk::Different(parts) => {
                if let Some(resolved) = trivial_merge(&parts[..num_diffs], &parts[num_diffs..]) {
                    resolved_hunk.extend_from_slice(resolved);
                } else {
                    if !resolved_hunk.is_empty() {
                        merge_hunks.push(Merge::resolved(resolved_hunk));
                        resolved_hunk = BString::new(vec![]);
                    }
                    merge_hunks.push(Merge::from_removes_adds(
                        parts[..num_diffs].iter().copied().map(BString::from),
                        parts[num_diffs..].iter().copied().map(BString::from),
                    ));
                }
            }
        }
    }

    if merge_hunks.is_empty() {
        MergeResult::Resolved(resolved_hunk)
    } else {
        if !resolved_hunk.is_empty() {
            merge_hunks.push(Merge::resolved(resolved_hunk));
        }
        MergeResult::Conflict(merge_hunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(data: &[u8]) -> BString {
        data.into()
    }

    fn merge(removes: &[&[u8]], adds: &[&[u8]]) -> MergeResult {
        super::merge(&Merge::from_removes_adds(removes, adds))
    }

    #[test]
    fn test_diff_line_iterator_line_numbers() {
        let mut line_iter = DiffLineIterator::with_line_number(
            [DiffHunk::different(["a\nb", "c\nd\n"])].into_iter(),
            DiffLineNumber { left: 1, right: 10 },
        );
        // Nothing queued
        assert_eq!(
            line_iter.next_line_number(),
            DiffLineNumber { left: 1, right: 10 }
        );
        assert_eq!(
            line_iter.next().unwrap(),
            DiffLine {
                line_number: DiffLineNumber { left: 1, right: 10 },
                hunks: vec![(DiffLineHunkSide::Left, "a\n".as_ref())],
            }
        );
        // Multiple lines queued
        assert_eq!(
            line_iter.next_line_number(),
            DiffLineNumber { left: 2, right: 10 }
        );
        assert_eq!(
            line_iter.next().unwrap(),
            DiffLine {
                line_number: DiffLineNumber { left: 2, right: 10 },
                hunks: vec![
                    (DiffLineHunkSide::Left, "b".as_ref()),
                    (DiffLineHunkSide::Right, "c\n".as_ref()),
                ],
            }
        );
        // Single line queued
        assert_eq!(
            line_iter.next_line_number(),
            DiffLineNumber { left: 2, right: 11 }
        );
        assert_eq!(
            line_iter.next().unwrap(),
            DiffLine {
                line_number: DiffLineNumber { left: 2, right: 11 },
                hunks: vec![(DiffLineHunkSide::Right, "d\n".as_ref())],
            }
        );
        // No more lines: left remains 2 as it lacks newline
        assert_eq!(
            line_iter.next_line_number(),
            DiffLineNumber { left: 2, right: 12 }
        );
        assert!(line_iter.next().is_none());
        assert_eq!(
            line_iter.next_line_number(),
            DiffLineNumber { left: 2, right: 12 }
        );
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
        // same block. You might expect the last block would be deduplicated. However,
        // the changes in the first side can be parsed as follows:
        // ```
        //  a {
        // -    p
        // +    q
        // +}
        // +
        // +b {
        // +    x
        //  }
        // ```
        // Therefore, the first side modifies the block `a { .. }`, and the second side
        // adds `b { .. }`. Git and Mercurial both duplicate the block in the result.
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

b {
    x
}
"
            ))
        );
    }
}
