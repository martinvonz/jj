// Copyright 2021 The Jujutsu Authors
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

use std::cmp::{max, min, Ordering};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use std::ops::Range;
use std::slice;

use itertools::Itertools;

pub fn find_line_ranges(text: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = vec![];
    let mut start = 0;
    loop {
        match text[start..].iter().position(|b| *b == b'\n') {
            None => {
                break;
            }
            Some(i) => {
                ranges.push(start..start + i + 1);
                start += i + 1;
            }
        }
    }
    if start < text.len() {
        ranges.push(start..text.len());
    }
    ranges
}

fn is_word_byte(b: u8) -> bool {
    // TODO: Make this configurable (probably higher up in the call stack)
    matches!(
        b,
        // Count 0x80..0xff as word bytes so multi-byte UTF-8 chars are
        // treated as a single unit.
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'\x80'..=b'\xff'
    )
}

pub fn find_word_ranges(text: &[u8]) -> Vec<Range<usize>> {
    let mut word_ranges = vec![];
    let mut word_start_pos = 0;
    let mut in_word = false;
    for (i, b) in text.iter().enumerate() {
        if in_word && !is_word_byte(*b) {
            in_word = false;
            word_ranges.push(word_start_pos..i);
            word_start_pos = i;
        } else if !in_word && is_word_byte(*b) {
            in_word = true;
            word_start_pos = i;
        }
    }
    if in_word && word_start_pos < text.len() {
        word_ranges.push(word_start_pos..text.len());
    }
    word_ranges
}

pub fn find_nonword_ranges(text: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = vec![];
    for (i, b) in text.iter().enumerate() {
        if !is_word_byte(*b) {
            ranges.push(i..i + 1);
        }
    }
    ranges
}

struct Histogram<'a> {
    word_to_positions: HashMap<&'a [u8], Vec<usize>>,
    count_to_words: BTreeMap<usize, Vec<&'a [u8]>>,
}

impl Histogram<'_> {
    fn calculate<'a>(
        text: &'a [u8],
        ranges: &[Range<usize>],
        max_occurrences: usize,
    ) -> Histogram<'a> {
        let mut word_to_positions: HashMap<&[u8], Vec<usize>> = HashMap::new();
        for (i, range) in ranges.iter().enumerate() {
            let positions = word_to_positions.entry(&text[range.clone()]).or_default();
            // Allow one more than max_occurrences, so we can later skip those with more
            // than max_occurrences
            if positions.len() <= max_occurrences {
                positions.push(i);
            }
        }
        let mut count_to_words: BTreeMap<usize, Vec<&[u8]>> = BTreeMap::new();
        for (word, ranges) in &word_to_positions {
            count_to_words.entry(ranges.len()).or_default().push(word);
        }
        Histogram {
            word_to_positions,
            count_to_words,
        }
    }
}

/// Finds the LCS given a array where the value of `input[i]` indicates that
/// the position of element `i` in the right array is at position `input[i]` in
/// the left array.
///
/// For example (some have multiple valid outputs):
///
/// [0,1,2] => [(0,0),(1,1),(2,2)]
/// [2,1,0] => [(0,2)]
/// [0,1,4,2,3,5,6] => [(0,0),(1,1),(2,3),(3,4),(5,5),(6,6)]
/// [0,1,4,3,2,5,6] => [(0,0),(1,1),(4,2),(5,5),(6,6)]
fn find_lcs(input: &[usize]) -> Vec<(usize, usize)> {
    if input.is_empty() {
        return vec![];
    }

    let mut chain = vec![(0, 0, 0); input.len()];
    let mut global_longest = 0;
    let mut global_longest_right_pos = 0;
    for (right_pos, &left_pos) in input.iter().enumerate() {
        let mut longest_from_here = 1;
        let mut previous_right_pos = usize::MAX;
        for i in (0..right_pos).rev() {
            let (previous_len, previous_left_pos, _) = chain[i];
            if previous_left_pos < left_pos {
                let len = previous_len + 1;
                if len > longest_from_here {
                    longest_from_here = len;
                    previous_right_pos = i;
                    if len > global_longest {
                        global_longest = len;
                        global_longest_right_pos = right_pos;
                        // If this is the longest chain globally so far, we cannot find a
                        // longer one by using a previous value, so break early.
                        break;
                    }
                }
            }
        }
        chain[right_pos] = (longest_from_here, left_pos, previous_right_pos);
    }

    let mut result = vec![];
    let mut right_pos = global_longest_right_pos;
    loop {
        let (_, left_pos, previous_right_pos) = chain[right_pos];
        result.push((left_pos, right_pos));
        if previous_right_pos == usize::MAX {
            break;
        }
        right_pos = previous_right_pos;
    }
    result.reverse();

    result
}

/// Finds unchanged ranges among the ones given as arguments. The data between
/// those ranges is ignored.
pub(crate) fn unchanged_ranges(
    left: &[u8],
    right: &[u8],
    left_ranges: &[Range<usize>],
    right_ranges: &[Range<usize>],
) -> Vec<(Range<usize>, Range<usize>)> {
    if left_ranges.is_empty() || right_ranges.is_empty() {
        return vec![];
    }

    let max_occurrences = 100;
    let mut left_histogram = Histogram::calculate(left, left_ranges, max_occurrences);
    if *left_histogram.count_to_words.keys().next().unwrap() > max_occurrences {
        // If there are very many occurrences of all words, then we just give up.
        return vec![];
    }
    let mut right_histogram = Histogram::calculate(right, right_ranges, max_occurrences);
    // Look for words with few occurrences in `left` (could equally well have picked
    // `right`?). If any of them also occur in `right`, then we add the words to
    // the LCS.
    let mut uncommon_shared_words = vec![];
    while !left_histogram.count_to_words.is_empty() && uncommon_shared_words.is_empty() {
        let left_words = left_histogram
            .count_to_words
            .first_entry()
            .map(|x| x.remove())
            .unwrap();
        for left_word in left_words {
            if right_histogram.word_to_positions.contains_key(left_word) {
                uncommon_shared_words.push(left_word);
            }
        }
    }
    if uncommon_shared_words.is_empty() {
        return vec![];
    }

    // Let's say our inputs are "a b a b" and "a b c c b a b". We will have found
    // the least common words to be "a" and "b". We now assume that each
    // occurrence of each word lines up in the left and right input. We do that
    // by numbering the shared occurrences, effectively instead comparing "a1 b1
    // a2 b2" and "a1 b1 c c b2 a2 b". We then walk the common words in the
    // right input in order (["a1", "b1", "b2", "a2"]), and record the index of
    // that word in the left input ([0,1,3,2]). We then find the LCS and split
    // points based on that ([0,1,3] or [0,1,2] are both valid).

    // [(index into left_ranges, word, occurrence #)]
    let mut left_positions = vec![];
    let mut right_positions = vec![];
    for uncommon_shared_word in uncommon_shared_words {
        let left_occurrences = left_histogram
            .word_to_positions
            .get_mut(uncommon_shared_word)
            .unwrap();
        let right_occurrences = right_histogram
            .word_to_positions
            .get_mut(uncommon_shared_word)
            .unwrap();
        let shared_count = min(left_occurrences.len(), right_occurrences.len());
        for occurrence in 0..shared_count {
            left_positions.push((
                left_occurrences[occurrence],
                uncommon_shared_word,
                occurrence,
            ));
            right_positions.push((
                right_occurrences[occurrence],
                uncommon_shared_word,
                occurrence,
            ));
        }
    }
    left_positions.sort();
    right_positions.sort();
    let mut left_position_map = HashMap::new();
    for (i, (_pos, word, occurrence)) in left_positions.iter().enumerate() {
        left_position_map.insert((*word, *occurrence), i);
    }
    let mut left_index_by_right_index = vec![];
    for (_pos, word, occurrence) in &right_positions {
        left_index_by_right_index.push(*left_position_map.get(&(*word, *occurrence)).unwrap());
    }

    let lcs = find_lcs(&left_index_by_right_index);

    // Produce output ranges, recursing into the modified areas between the elements
    // in the LCS.
    let mut result = vec![];
    let mut previous_left_position = 0;
    let mut previous_right_position = 0;
    for (left_index, right_index) in lcs {
        let left_position = left_positions[left_index].0;
        let right_position = right_positions[right_index].0;
        let skipped_left_positions = previous_left_position..left_position;
        let skipped_right_positions = previous_right_position..right_position;
        if !skipped_left_positions.is_empty() || !skipped_right_positions.is_empty() {
            for unchanged_nested_range in unchanged_ranges(
                left,
                right,
                &left_ranges[skipped_left_positions.clone()],
                &right_ranges[skipped_right_positions.clone()],
            ) {
                result.push(unchanged_nested_range);
            }
        }
        result.push((
            left_ranges[left_position].clone(),
            right_ranges[right_position].clone(),
        ));
        previous_left_position = left_position + 1;
        previous_right_position = right_position + 1;
    }
    // Also recurse into range at end (after common ranges).
    let skipped_left_positions = previous_left_position..left_ranges.len();
    let skipped_right_positions = previous_right_position..right_ranges.len();
    if !skipped_left_positions.is_empty() || !skipped_right_positions.is_empty() {
        for unchanged_nested_range in unchanged_ranges(
            left,
            right,
            &left_ranges[skipped_left_positions],
            &right_ranges[skipped_right_positions],
        ) {
            result.push(unchanged_nested_range);
        }
    }

    result
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct UnchangedRange {
    base_range: Range<usize>,
    offsets: Vec<isize>,
}

impl UnchangedRange {
    fn start(&self, side: usize) -> usize {
        self.base_range
            .start
            .wrapping_add(self.offsets[side] as usize)
    }

    fn end(&self, side: usize) -> usize {
        self.base_range
            .end
            .wrapping_add(self.offsets[side] as usize)
    }
}

impl PartialOrd for UnchangedRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UnchangedRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.base_range
            .start
            .cmp(&other.base_range.start)
            .then_with(|| self.base_range.end.cmp(&other.base_range.end))
    }
}

/// Takes any number of inputs and finds regions that are them same between all
/// of them.
#[derive(Clone, Debug)]
pub struct Diff<'input> {
    base_input: &'input [u8],
    other_inputs: Vec<&'input [u8]>,
    // The key is a range in the base input. The value is the start of each non-base region
    // relative to the base region's start. By making them relative, they don't need to change
    // when the base range changes.
    unchanged_regions: Vec<UnchangedRange>,
}

/// Takes the current regions and intersects it with the new unchanged ranges
/// from a 2-way diff. The result is a map of unchanged regions with one more
/// offset in the map's values.
fn intersect_regions(
    current_ranges: Vec<UnchangedRange>,
    new_unchanged_ranges: &[(Range<usize>, Range<usize>)],
) -> Vec<UnchangedRange> {
    let mut result = vec![];
    let mut current_ranges_iter = current_ranges.into_iter().peekable();
    for (new_base_range, other_range) in new_unchanged_ranges.iter() {
        assert_eq!(new_base_range.len(), other_range.len());
        while let Some(UnchangedRange {
            base_range,
            offsets,
        }) = current_ranges_iter.peek()
        {
            // No need to look further if we're past the new range.
            if base_range.start >= new_base_range.end {
                break;
            }
            // Discard any current unchanged regions that don't match between the base and
            // the new input.
            if base_range.end <= new_base_range.start {
                current_ranges_iter.next();
                continue;
            }
            let new_start = max(base_range.start, new_base_range.start);
            let new_end = min(base_range.end, new_base_range.end);
            let mut new_offsets = offsets.clone();
            new_offsets.push(other_range.start.wrapping_sub(new_base_range.start) as isize);
            result.push(UnchangedRange {
                base_range: new_start..new_end,
                offsets: new_offsets,
            });
            if base_range.end >= new_base_range.end {
                // Break without consuming the item; there may be other new ranges that overlap
                // with it.
                break;
            }
            current_ranges_iter.next();
        }
    }
    result
}

impl<'input> Diff<'input> {
    pub fn for_tokenizer(
        inputs: &[&'input [u8]],
        tokenizer: &impl Fn(&[u8]) -> Vec<Range<usize>>,
    ) -> Self {
        assert!(!inputs.is_empty());
        let base_input = inputs[0];
        let other_inputs = inputs.iter().skip(1).copied().collect_vec();
        // First tokenize each input
        let base_token_ranges: Vec<Range<usize>> = tokenizer(base_input);
        let other_token_ranges: Vec<Vec<Range<usize>>> = other_inputs
            .iter()
            .map(|other_input| tokenizer(other_input))
            .collect_vec();

        // Look for unchanged regions. Initially consider the whole range of the base
        // input as unchanged (compared to itself). Then diff each other input
        // against the base. Intersect the previously found ranges with the
        // unchanged ranges in the diff.
        let mut unchanged_regions = vec![UnchangedRange {
            base_range: 0..base_input.len(),
            offsets: vec![],
        }];
        for (i, other_token_ranges) in other_token_ranges.iter().enumerate() {
            let unchanged_diff_ranges = unchanged_ranges(
                base_input,
                other_inputs[i],
                &base_token_ranges,
                other_token_ranges,
            );
            unchanged_regions = intersect_regions(unchanged_regions, &unchanged_diff_ranges);
        }
        // Add an empty range at the end to make life easier for hunks().
        let offsets = other_inputs
            .iter()
            .map(|input| input.len().wrapping_sub(base_input.len()) as isize)
            .collect_vec();
        unchanged_regions.push(UnchangedRange {
            base_range: base_input.len()..base_input.len(),
            offsets,
        });

        let mut diff = Self {
            base_input,
            other_inputs,
            unchanged_regions,
        };
        diff.compact_unchanged_regions();
        diff
    }

    pub fn unrefined(inputs: &[&'input [u8]]) -> Self {
        Diff::for_tokenizer(inputs, &|_| vec![])
    }

    // TODO: At least when merging, it's wasteful to refine the diff if e.g. if 2
    // out of 3 inputs match in the differing regions. Perhaps the refine()
    // method should be on the hunk instead (probably returning a new Diff)?
    // That would let each user decide which hunks to refine. However, it would
    // probably mean that many callers repeat the same code. Perhaps it
    // should be possible to refine a whole diff *or* individual hunks.
    pub fn default_refinement(inputs: &[&'input [u8]]) -> Self {
        let mut diff = Diff::for_tokenizer(inputs, &find_line_ranges);
        diff.refine_changed_regions(&find_word_ranges);
        diff.refine_changed_regions(&find_nonword_ranges);
        diff
    }

    pub fn hunks<'diff>(&'diff self) -> DiffHunkIterator<'diff, 'input> {
        let previous_offsets = vec![0; self.other_inputs.len()];
        DiffHunkIterator {
            diff: self,
            previous: UnchangedRange {
                base_range: 0..0,
                offsets: previous_offsets,
            },
            unchanged_emitted: true,
            unchanged_iter: self.unchanged_regions.iter(),
        }
    }

    /// Uses the given tokenizer to split the changed regions into smaller
    /// regions. Then tries to finds unchanged regions among them.
    pub fn refine_changed_regions(&mut self, tokenizer: &impl Fn(&[u8]) -> Vec<Range<usize>>) {
        let mut previous = UnchangedRange {
            base_range: 0..0,
            offsets: vec![0; self.other_inputs.len()],
        };
        let mut new_unchanged_ranges = vec![];
        for current in self.unchanged_regions.iter() {
            // For the changed region between the previous region and the current one,
            // create a new Diff instance. Then adjust the start positions and
            // offsets to be valid in the context of the larger Diff instance
            // (`self`).
            let mut slices =
                vec![&self.base_input[previous.base_range.end..current.base_range.start]];
            for i in 0..current.offsets.len() {
                let changed_range = previous.end(i)..current.start(i);
                slices.push(&self.other_inputs[i][changed_range]);
            }

            let refined_diff = Diff::for_tokenizer(&slices, tokenizer);

            for UnchangedRange {
                base_range,
                offsets,
            } in refined_diff.unchanged_regions
            {
                let new_base_start = base_range.start + previous.base_range.end;
                let new_base_end = base_range.end + previous.base_range.end;
                let offsets = offsets
                    .into_iter()
                    .enumerate()
                    .map(|(i, offset)| offset + previous.offsets[i])
                    .collect_vec();
                new_unchanged_ranges.push(UnchangedRange {
                    base_range: new_base_start..new_base_end,
                    offsets,
                });
            }
            previous = current.clone();
        }
        self.unchanged_regions = self
            .unchanged_regions
            .iter()
            .cloned()
            .merge(new_unchanged_ranges)
            .collect_vec();
        self.compact_unchanged_regions();
    }

    fn compact_unchanged_regions(&mut self) {
        let mut compacted = vec![];
        let mut maybe_previous: Option<UnchangedRange> = None;
        for current in self.unchanged_regions.iter() {
            if let Some(previous) = maybe_previous {
                if previous.base_range.end == current.base_range.start
                    && previous.offsets == *current.offsets
                {
                    maybe_previous = Some(UnchangedRange {
                        base_range: previous.base_range.start..current.base_range.end,
                        offsets: current.offsets.clone(),
                    });
                    continue;
                }
                compacted.push(previous);
            }
            maybe_previous = Some(current.clone());
        }
        if let Some(previous) = maybe_previous {
            compacted.push(previous);
        }
        self.unchanged_regions = compacted;
    }
}

#[derive(PartialEq, Eq, Clone)]
pub enum DiffHunk<'input> {
    Matching(&'input [u8]),
    Different(Vec<&'input [u8]>),
}

impl Debug for DiffHunk<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            DiffHunk::Matching(slice) => f
                .debug_tuple("DiffHunk::Matching")
                .field(&String::from_utf8_lossy(slice))
                .finish(),
            DiffHunk::Different(slices) => f
                .debug_tuple("DiffHunk::Different")
                .field(
                    &slices
                        .iter()
                        .map(|slice| String::from_utf8_lossy(slice))
                        .collect_vec(),
                )
                .finish(),
        }
    }
}

pub struct DiffHunkIterator<'diff, 'input> {
    diff: &'diff Diff<'input>,
    previous: UnchangedRange,
    unchanged_emitted: bool,
    unchanged_iter: slice::Iter<'diff, UnchangedRange>,
}

impl<'diff, 'input> Iterator for DiffHunkIterator<'diff, 'input> {
    type Item = DiffHunk<'input>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if !self.unchanged_emitted {
                self.unchanged_emitted = true;
                if !self.previous.base_range.is_empty() {
                    return Some(DiffHunk::Matching(
                        &self.diff.base_input[self.previous.base_range.clone()],
                    ));
                }
            }
            if let Some(current) = self.unchanged_iter.next() {
                let mut slices = vec![
                    &self.diff.base_input[self.previous.base_range.end..current.base_range.start],
                ];
                for (i, input) in self.diff.other_inputs.iter().enumerate() {
                    slices.push(&input[self.previous.end(i)..current.start(i)]);
                }
                self.previous = current.clone();
                self.unchanged_emitted = false;
                if slices.iter().any(|slice| !slice.is_empty()) {
                    return Some(DiffHunk::Different(slices));
                }
            } else {
                break;
            }
        }
        None
    }
}

/// Diffs two slices of bytes. The returned diff hunks may be any length (may
/// span many lines or may be only part of a line). This currently uses
/// Histogram diff (or maybe something similar; I'm not sure I understood the
/// algorithm correctly). It first diffs lines in the input and then refines
/// the changed ranges at the word level.
pub fn diff<'a>(left: &'a [u8], right: &'a [u8]) -> Vec<DiffHunk<'a>> {
    if left == right {
        return vec![DiffHunk::Matching(left)];
    }
    if left.is_empty() {
        return vec![DiffHunk::Different(vec![b"", right])];
    }
    if right.is_empty() {
        return vec![DiffHunk::Different(vec![left, b""])];
    }

    Diff::default_refinement(&[left, right])
        .hunks()
        .collect_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Extracted to a function because type inference is ambiguous due to
    // `impl PartialEq<aho_corasick::util::search::Span> for std::ops::Range<usize>`
    fn no_ranges() -> Vec<Range<usize>> {
        vec![]
    }

    #[test]
    fn test_find_line_ranges_empty() {
        assert_eq!(find_line_ranges(b""), no_ranges());
    }

    #[test]
    fn test_find_line_ranges_blank_line() {
        assert_eq!(find_line_ranges(b"\n"), vec![0..1]);
    }

    #[test]
    fn test_find_line_ranges_missing_newline_at_eof() {
        assert_eq!(find_line_ranges(b"foo"), vec![0..3]);
    }

    #[test]
    fn test_find_line_ranges_multiple_lines() {
        assert_eq!(find_line_ranges(b"a\nbb\nccc\n"), vec![0..2, 2..5, 5..9]);
    }

    #[test]
    fn test_find_word_ranges_empty() {
        assert_eq!(find_word_ranges(b""), no_ranges());
    }

    #[test]
    fn test_find_word_ranges_single_word() {
        assert_eq!(find_word_ranges(b"Abc"), vec![0..3]);
    }

    #[test]
    fn test_find_word_ranges_no_word() {
        assert_eq!(find_word_ranges(b"+-*/"), no_ranges());
    }

    #[test]
    fn test_find_word_ranges_word_then_non_word() {
        assert_eq!(find_word_ranges(b"Abc   "), vec![0..3]);
    }

    #[test]
    fn test_find_word_ranges_non_word_then_word() {
        assert_eq!(find_word_ranges(b"   Abc"), vec![3..6]);
    }

    #[test]
    fn test_find_word_ranges_multibyte() {
        assert_eq!(find_word_ranges("⊢".as_bytes()), vec![0..3])
    }

    #[test]
    fn test_find_lcs_empty() {
        let empty: Vec<(usize, usize)> = vec![];
        assert_eq!(find_lcs(&[]), empty);
    }

    #[test]
    fn test_find_lcs_single_element() {
        assert_eq!(find_lcs(&[0]), vec![(0, 0)]);
    }

    #[test]
    fn test_find_lcs_in_order() {
        assert_eq!(find_lcs(&[0, 1, 2]), vec![(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn test_find_lcs_reverse_order() {
        assert_eq!(find_lcs(&[2, 1, 0]), vec![(2, 0)]);
    }

    #[test]
    fn test_find_lcs_two_swapped() {
        assert_eq!(
            find_lcs(&[0, 1, 4, 3, 2, 5, 6]),
            vec![(0, 0), (1, 1), (2, 4), (5, 5), (6, 6)]
        );
    }

    #[test]
    fn test_find_lcs_element_moved_earlier() {
        assert_eq!(
            find_lcs(&[0, 1, 4, 2, 3, 5, 6]),
            vec![(0, 0), (1, 1), (2, 3), (3, 4), (5, 5), (6, 6)]
        );
    }

    #[test]
    fn test_find_lcs_element_moved_later() {
        assert_eq!(
            find_lcs(&[0, 1, 3, 4, 2, 5, 6]),
            vec![(0, 0), (1, 1), (3, 2), (4, 3), (5, 5), (6, 6)]
        );
    }

    #[test]
    fn test_find_lcs_interleaved_longest_chains() {
        assert_eq!(
            find_lcs(&[0, 4, 2, 9, 6, 5, 1, 3, 7, 8]),
            vec![(0, 0), (1, 6), (3, 7), (7, 8), (8, 9)]
        );
    }

    #[test]
    fn test_find_word_ranges_many_words() {
        assert_eq!(
            find_word_ranges(b"fn find_words(text: &[u8])"),
            vec![0..2, 3..13, 14..18, 22..24]
        );
    }

    #[test]
    fn test_unchanged_ranges_insert_in_middle() {
        assert_eq!(
            unchanged_ranges(
                b"a b b c",
                b"a b X b c",
                &[0..1, 2..3, 4..5, 6..7],
                &[0..1, 2..3, 4..5, 6..7, 8..9],
            ),
            vec![(0..1, 0..1), (2..3, 2..3), (4..5, 6..7), (6..7, 8..9)]
        );
    }

    #[test]
    fn test_unchanged_ranges_non_unique_removed() {
        assert_eq!(
            unchanged_ranges(
                b"a a a a",
                b"a b a c",
                &[0..1, 2..3, 4..5, 6..7],
                &[0..1, 2..3, 4..5, 6..7],
            ),
            vec![(0..1, 0..1), (2..3, 4..5)]
        );
    }

    #[test]
    fn test_unchanged_ranges_non_unique_added() {
        assert_eq!(
            unchanged_ranges(
                b"a b a c",
                b"a a a a",
                &[0..1, 2..3, 4..5, 6..7],
                &[0..1, 2..3, 4..5, 6..7],
            ),
            vec![(0..1, 0..1), (4..5, 2..3)]
        );
    }

    #[test]
    fn test_intersect_regions_existing_empty() {
        let actual = intersect_regions(vec![], &[(20..25, 55..60)]);
        let expected = vec![];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_intersect_regions_new_ranges_within_existing() {
        let actual = intersect_regions(
            vec![UnchangedRange {
                base_range: 20..70,
                offsets: vec![3],
            }],
            &[(25..30, 35..40), (40..50, 40..50)],
        );
        let expected = vec![
            UnchangedRange {
                base_range: 25..30,
                offsets: vec![3, 10],
            },
            UnchangedRange {
                base_range: 40..50,
                offsets: vec![3, 0],
            },
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_intersect_regions_partial_overlap() {
        let actual = intersect_regions(
            vec![UnchangedRange {
                base_range: 20..50,
                offsets: vec![-3],
            }],
            &[(15..25, 5..15), (45..60, 55..70)],
        );
        let expected = vec![
            UnchangedRange {
                base_range: 20..25,
                offsets: vec![-3, -10],
            },
            UnchangedRange {
                base_range: 45..50,
                offsets: vec![-3, 10],
            },
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_intersect_regions_new_range_overlaps_multiple_existing() {
        let actual = intersect_regions(
            vec![
                UnchangedRange {
                    base_range: 20..50,
                    offsets: vec![3, -8],
                },
                UnchangedRange {
                    base_range: 70..80,
                    offsets: vec![7, 1],
                },
            ],
            &[(10..100, 5..95)],
        );
        let expected = vec![
            UnchangedRange {
                base_range: 20..50,
                offsets: vec![3, -8, -5],
            },
            UnchangedRange {
                base_range: 70..80,
                offsets: vec![7, 1, -5],
            },
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_diff_single_input() {
        let diff = Diff::default_refinement(&[b"abc"]);
        assert_eq!(diff.hunks().collect_vec(), vec![DiffHunk::Matching(b"abc")]);
    }

    #[test]
    fn test_diff_single_empty_input() {
        let diff = Diff::default_refinement(&[b""]);
        assert_eq!(diff.hunks().collect_vec(), vec![]);
    }

    #[test]
    fn test_diff_two_inputs_one_different() {
        let diff = Diff::default_refinement(&[b"a b c", b"a X c"]);
        assert_eq!(
            diff.hunks().collect_vec(),
            vec![
                DiffHunk::Matching(b"a "),
                DiffHunk::Different(vec![b"b", b"X"]),
                DiffHunk::Matching(b" c"),
            ]
        );
    }

    #[test]
    fn test_diff_multiple_inputs_one_different() {
        let diff = Diff::default_refinement(&[b"a b c", b"a X c", b"a b c"]);
        assert_eq!(
            diff.hunks().collect_vec(),
            vec![
                DiffHunk::Matching(b"a "),
                DiffHunk::Different(vec![b"b", b"X", b"b"]),
                DiffHunk::Matching(b" c"),
            ]
        );
    }

    #[test]
    fn test_diff_multiple_inputs_all_different() {
        let diff = Diff::default_refinement(&[b"a b c", b"a X c", b"a c X"]);
        assert_eq!(
            diff.hunks().collect_vec(),
            vec![
                DiffHunk::Matching(b"a "),
                DiffHunk::Different(vec![b"b ", b"X ", b""]),
                DiffHunk::Matching(b"c"),
                DiffHunk::Different(vec![b"", b"", b" X"]),
            ]
        );
    }

    #[test]
    fn test_diff_for_tokenizer_compacted() {
        // Tests that unchanged regions are compacted when using for_tokenizer()
        let diff = Diff::for_tokenizer(
            &[b"a\nb\nc\nd\ne\nf\ng", b"a\nb\nc\nX\ne\nf\ng"],
            &find_line_ranges,
        );
        assert_eq!(
            diff.hunks().collect_vec(),
            vec![
                DiffHunk::Matching(b"a\nb\nc\n"),
                DiffHunk::Different(vec![b"d\n", b"X\n"]),
                DiffHunk::Matching(b"e\nf\ng"),
            ]
        );
    }

    #[test]
    fn test_diff_nothing_in_common() {
        assert_eq!(
            diff(b"aaa", b"bb"),
            vec![DiffHunk::Different(vec![b"aaa", b"bb"])]
        );
    }

    #[test]
    fn test_diff_insert_in_middle() {
        assert_eq!(
            diff(b"a z", b"a S z"),
            vec![
                DiffHunk::Matching(b"a "),
                DiffHunk::Different(vec![b"", b"S "]),
                DiffHunk::Matching(b"z"),
            ]
        );
    }

    #[test]
    fn test_diff_no_unique_middle_flips() {
        assert_eq!(
            diff(b"a R R S S z", b"a S S R R z"),
            vec![
                DiffHunk::Matching(b"a "),
                DiffHunk::Different(vec![b"R R ", b""]),
                DiffHunk::Matching(b"S S "),
                DiffHunk::Different(vec![b"", b"R R "]),
                DiffHunk::Matching(b"z")
            ],
        );
    }

    #[test]
    fn test_diff_recursion_needed() {
        assert_eq!(
            diff(
                b"a q x q y q z q b q y q x q c",
                b"a r r x q y z q b y q x r r c",
            ),
            vec![
                DiffHunk::Matching(b"a "),
                DiffHunk::Different(vec![b"q", b"r"]),
                DiffHunk::Matching(b" "),
                DiffHunk::Different(vec![b"", b"r "]),
                DiffHunk::Matching(b"x q y "),
                DiffHunk::Different(vec![b"q ", b""]),
                DiffHunk::Matching(b"z q b "),
                DiffHunk::Different(vec![b"q ", b""]),
                DiffHunk::Matching(b"y q x "),
                DiffHunk::Different(vec![b"q", b"r"]),
                DiffHunk::Matching(b" "),
                DiffHunk::Different(vec![b"", b"r "]),
                DiffHunk::Matching(b"c"),
            ]
        );
    }

    #[test]
    fn test_diff_real_case_write_fmt() {
        // This is from src/ui.rs in commit f44d246e3f88 in this repo. It highlights the
        // need for recursion into the range at the end: after splitting at "Arguments"
        // and "formatter", the region at the end has the unique words "write_fmt"
        // and "fmt", but we forgot to recurse into that region, so we ended up
        // saying that "write_fmt(fmt).unwrap()" was replaced by b"write_fmt(fmt)".
        assert_eq!(diff(
                b"    pub fn write_fmt(&mut self, fmt: fmt::Arguments<\'_>) {\n        self.styler().write_fmt(fmt).unwrap()\n",
                b"    pub fn write_fmt(&mut self, fmt: fmt::Arguments<\'_>) -> io::Result<()> {\n        self.styler().write_fmt(fmt)\n"
            ),
            vec![
                DiffHunk::Matching(b"    pub fn write_fmt(&mut self, fmt: fmt::Arguments<\'_>) "),
                DiffHunk::Different(vec![b"", b"-> io::Result<()> "]),
                DiffHunk::Matching(b"{\n        self.styler().write_fmt(fmt)"),
                DiffHunk::Different(vec![b".unwrap()", b""]),
                DiffHunk::Matching(b"\n")
            ]
        );
    }

    #[test]
    fn test_diff_real_case_gitgit_read_tree_c() {
        // This is the diff from commit e497ea2a9b in the git.git repo
        assert_eq!(
            diff(
                br##"/*
 * GIT - The information manager from hell
 *
 * Copyright (C) Linus Torvalds, 2005
 */
#include "#cache.h"

static int unpack(unsigned char *sha1)
{
	void *buffer;
	unsigned long size;
	char type[20];

	buffer = read_sha1_file(sha1, type, &size);
	if (!buffer)
		usage("unable to read sha1 file");
	if (strcmp(type, "tree"))
		usage("expected a 'tree' node");
	while (size) {
		int len = strlen(buffer)+1;
		unsigned char *sha1 = buffer + len;
		char *path = strchr(buffer, ' ')+1;
		unsigned int mode;
		if (size < len + 20 || sscanf(buffer, "%o", &mode) != 1)
			usage("corrupt 'tree' file");
		buffer = sha1 + 20;
		size -= len + 20;
		printf("%o %s (%s)\n", mode, path, sha1_to_hex(sha1));
	}
	return 0;
}

int main(int argc, char **argv)
{
	int fd;
	unsigned char sha1[20];

	if (argc != 2)
		usage("read-tree <key>");
	if (get_sha1_hex(argv[1], sha1) < 0)
		usage("read-tree <key>");
	sha1_file_directory = getenv(DB_ENVIRONMENT);
	if (!sha1_file_directory)
		sha1_file_directory = DEFAULT_DB_ENVIRONMENT;
	if (unpack(sha1) < 0)
		usage("unpack failed");
	return 0;
}
"##,
                br##"/*
 * GIT - The information manager from hell
 *
 * Copyright (C) Linus Torvalds, 2005
 */
#include "#cache.h"

static void create_directories(const char *path)
{
	int len = strlen(path);
	char *buf = malloc(len + 1);
	const char *slash = path;

	while ((slash = strchr(slash+1, '/')) != NULL) {
		len = slash - path;
		memcpy(buf, path, len);
		buf[len] = 0;
		mkdir(buf, 0700);
	}
}

static int create_file(const char *path)
{
	int fd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);
	if (fd < 0) {
		if (errno == ENOENT) {
			create_directories(path);
			fd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);
		}
	}
	return fd;
}

static int unpack(unsigned char *sha1)
{
	void *buffer;
	unsigned long size;
	char type[20];

	buffer = read_sha1_file(sha1, type, &size);
	if (!buffer)
		usage("unable to read sha1 file");
	if (strcmp(type, "tree"))
		usage("expected a 'tree' node");
	while (size) {
		int len = strlen(buffer)+1;
		unsigned char *sha1 = buffer + len;
		char *path = strchr(buffer, ' ')+1;
		char *data;
		unsigned long filesize;
		unsigned int mode;
		int fd;

		if (size < len + 20 || sscanf(buffer, "%o", &mode) != 1)
			usage("corrupt 'tree' file");
		buffer = sha1 + 20;
		size -= len + 20;
		data = read_sha1_file(sha1, type, &filesize);
		if (!data || strcmp(type, "blob"))
			usage("tree file refers to bad file data");
		fd = create_file(path);
		if (fd < 0)
			usage("unable to create file");
		if (write(fd, data, filesize) != filesize)
			usage("unable to write file");
		fchmod(fd, mode);
		close(fd);
		free(data);
	}
	return 0;
}

int main(int argc, char **argv)
{
	int fd;
	unsigned char sha1[20];

	if (argc != 2)
		usage("read-tree <key>");
	if (get_sha1_hex(argv[1], sha1) < 0)
		usage("read-tree <key>");
	sha1_file_directory = getenv(DB_ENVIRONMENT);
	if (!sha1_file_directory)
		sha1_file_directory = DEFAULT_DB_ENVIRONMENT;
	if (unpack(sha1) < 0)
		usage("unpack failed");
	return 0;
}
"##,
            ),
            vec![
               DiffHunk::Matching(b"/*\n * GIT - The information manager from hell\n *\n * Copyright (C) Linus Torvalds, 2005\n */\n#include \"#cache.h\"\n\n"),
               DiffHunk::Different(vec![b"", b"static void create_directories(const char *path)\n{\n\tint len = strlen(path);\n\tchar *buf = malloc(len + 1);\n\tconst char *slash = path;\n\n\twhile ((slash = strchr(slash+1, \'/\')) != NULL) {\n\t\tlen = slash - path;\n\t\tmemcpy(buf, path, len);\n\t\tbuf[len] = 0;\n\t\tmkdir(buf, 0700);\n\t}\n}\n\nstatic int create_file(const char *path)\n{\n\tint fd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);\n\tif (fd < 0) {\n\t\tif (errno == ENOENT) {\n\t\t\tcreate_directories(path);\n\t\t\tfd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);\n\t\t}\n\t}\n\treturn fd;\n}\n\n"]),
               DiffHunk::Matching(b"static int unpack(unsigned char *sha1)\n{\n\tvoid *buffer;\n\tunsigned long size;\n\tchar type[20];\n\n\tbuffer = read_sha1_file(sha1, type, &size);\n\tif (!buffer)\n\t\tusage(\"unable to read sha1 file\");\n\tif (strcmp(type, \"tree\"))\n\t\tusage(\"expected a \'tree\' node\");\n\twhile (size) {\n\t\tint len = strlen(buffer)+1;\n\t\tunsigned char *sha1 = buffer + len;\n\t\tchar *path = strchr(buffer, \' \')+1;\n"),
               DiffHunk::Different(vec![b"", b"\t\tchar *data;\n\t\tunsigned long filesize;\n"]),
               DiffHunk::Matching(b"\t\tunsigned int mode;\n"),
               DiffHunk::Different(vec![b"", b"\t\tint fd;\n\n"]),
               DiffHunk::Matching(b"\t\tif (size < len + 20 || sscanf(buffer, \"%o\", &mode) != 1)\n\t\t\tusage(\"corrupt \'tree\' file\");\n\t\tbuffer = sha1 + 20;\n\t\tsize -= len + 20;\n\t\t"),
               DiffHunk::Different(vec![b"printf", b"data = read_sha1_file"]),
               DiffHunk::Matching(b"("),
               DiffHunk::Different(vec![b"\"%o %s (%s)\\n\", mode, path, sha1_to_hex(", b""]),
               DiffHunk::Matching(b"sha1"),
               DiffHunk::Different(vec![b"", b", type, &filesize"]),
               DiffHunk::Matching(b")"),
               DiffHunk::Different(vec![b")", b""]),
               DiffHunk::Matching(b";\n"),
               DiffHunk::Different(vec![b"", b"\t\tif (!data || strcmp(type, \"blob\"))\n\t\t\tusage(\"tree file refers to bad file data\");\n\t\tfd = create_file(path);\n\t\tif (fd < 0)\n\t\t\tusage(\"unable to create file\");\n\t\tif (write(fd, data, filesize) != filesize)\n\t\t\tusage(\"unable to write file\");\n\t\tfchmod(fd, mode);\n\t\tclose(fd);\n\t\tfree(data);\n"]),
               DiffHunk::Matching(b"\t}\n\treturn 0;\n}\n\nint main(int argc, char **argv)\n{\n\tint fd;\n\tunsigned char sha1[20];\n\n\tif (argc != 2)\n\t\tusage(\"read-tree <key>\");\n\tif (get_sha1_hex(argv[1], sha1) < 0)\n\t\tusage(\"read-tree <key>\");\n\tsha1_file_directory = getenv(DB_ENVIRONMENT);\n\tif (!sha1_file_directory)\n\t\tsha1_file_directory = DEFAULT_DB_ENVIRONMENT;\n\tif (unpack(sha1) < 0)\n\t\tusage(\"unpack failed\");\n\treturn 0;\n}\n")
            ]
        );
    }
}
