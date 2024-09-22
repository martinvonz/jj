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

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::iter;
use std::ops::Range;
use std::slice;

use bstr::BStr;
use itertools::Itertools;

pub fn find_line_ranges(text: &[u8]) -> Vec<Range<usize>> {
    text.split_inclusive(|b| *b == b'\n')
        .scan(0, |total, line| {
            let start = *total;
            *total += line.len();
            Some(start..*total)
        })
        .collect()
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
    text.iter()
        .positions(|b| !is_word_byte(*b))
        .map(|i| i..i + 1)
        .collect()
}

/// Index in a list of word (or token) ranges.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct WordPosition(usize);

#[derive(Clone, Debug)]
struct DiffSource<'input, 'aux> {
    text: &'input BStr,
    ranges: &'aux [Range<usize>],
    /// The number of preceding word ranges excluded from the self `ranges`.
    global_offset: WordPosition,
}

impl<'input, 'aux> DiffSource<'input, 'aux> {
    fn new<T: AsRef<[u8]> + ?Sized>(text: &'input T, ranges: &'aux [Range<usize>]) -> Self {
        DiffSource {
            text: BStr::new(text),
            ranges,
            global_offset: WordPosition(0),
        }
    }

    fn narrowed(&self, positions: Range<WordPosition>) -> Self {
        DiffSource {
            text: self.text,
            ranges: &self.ranges[positions.start.0..positions.end.0],
            global_offset: self.map_to_global(positions.start),
        }
    }

    fn range_at(&self, position: WordPosition) -> Range<usize> {
        self.ranges[position.0].clone()
    }

    fn map_to_global(&self, position: WordPosition) -> WordPosition {
        WordPosition(self.global_offset.0 + position.0)
    }
}

struct Histogram<'input> {
    word_to_positions: HashMap<&'input BStr, Vec<WordPosition>>,
}

impl<'input> Histogram<'input> {
    fn calculate(source: &DiffSource<'input, '_>, max_occurrences: usize) -> Self {
        let mut word_to_positions: HashMap<&BStr, Vec<WordPosition>> = HashMap::new();
        for (i, range) in source.ranges.iter().enumerate() {
            let word = &source.text[range.clone()];
            let positions = word_to_positions.entry(word).or_default();
            // Allow one more than max_occurrences, so we can later skip those with more
            // than max_occurrences
            if positions.len() <= max_occurrences {
                positions.push(WordPosition(i));
            }
        }
        Histogram { word_to_positions }
    }

    fn build_count_to_entries(&self) -> BTreeMap<usize, Vec<(&'input BStr, &Vec<WordPosition>)>> {
        let mut count_to_entries: BTreeMap<usize, Vec<_>> = BTreeMap::new();
        for (word, positions) in &self.word_to_positions {
            let entries = count_to_entries.entry(positions.len()).or_default();
            entries.push((*word, positions));
        }
        count_to_entries
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

/// Finds unchanged word (or token) positions among the ones given as
/// arguments. The data between those words is ignored.
fn collect_unchanged_words(
    found_positions: &mut Vec<(WordPosition, WordPosition)>,
    left: &DiffSource,
    right: &DiffSource,
) {
    if left.ranges.is_empty() || right.ranges.is_empty() {
        return;
    }

    // Prioritize LCS-based algorithm than leading/trailing matches
    let old_len = found_positions.len();
    collect_unchanged_words_lcs(found_positions, left, right);
    if found_positions.len() != old_len {
        return;
    }

    // Trim leading common ranges (i.e. grow previous unchanged region)
    let common_leading_len = iter::zip(left.ranges, right.ranges)
        .take_while(|&(l, r)| left.text[l.clone()] == right.text[r.clone()])
        .count();
    let left_ranges = &left.ranges[common_leading_len..];
    let right_ranges = &right.ranges[common_leading_len..];

    // Trim trailing common ranges (i.e. grow next unchanged region)
    let common_trailing_len = iter::zip(left_ranges.iter().rev(), right_ranges.iter().rev())
        .take_while(|&(l, r)| left.text[l.clone()] == right.text[r.clone()])
        .count();

    found_positions.extend(itertools::chain(
        (0..common_leading_len).map(|i| {
            (
                left.map_to_global(WordPosition(i)),
                right.map_to_global(WordPosition(i)),
            )
        }),
        (1..=common_trailing_len).rev().map(|i| {
            (
                left.map_to_global(WordPosition(left.ranges.len() - i)),
                right.map_to_global(WordPosition(right.ranges.len() - i)),
            )
        }),
    ));
}

fn collect_unchanged_words_lcs(
    found_positions: &mut Vec<(WordPosition, WordPosition)>,
    left: &DiffSource,
    right: &DiffSource,
) {
    let max_occurrences = 100;
    let left_histogram = Histogram::calculate(left, max_occurrences);
    let left_count_to_entries = left_histogram.build_count_to_entries();
    if *left_count_to_entries.keys().next().unwrap() > max_occurrences {
        // If there are very many occurrences of all words, then we just give up.
        return;
    }
    let right_histogram = Histogram::calculate(right, max_occurrences);
    // Look for words with few occurrences in `left` (could equally well have picked
    // `right`?). If any of them also occur in `right`, then we add the words to
    // the LCS.
    let Some(uncommon_shared_word_positions) =
        left_count_to_entries.values().find_map(|left_entries| {
            let mut both_positions = left_entries
                .iter()
                .filter_map(|&(word, left_positions)| {
                    let right_positions = right_histogram.word_to_positions.get(word)?;
                    (left_positions.len() == right_positions.len())
                        .then_some((left_positions, right_positions))
                })
                .peekable();
            both_positions.peek().is_some().then_some(both_positions)
        })
    else {
        return;
    };

    // [(index into ranges, serial to identify {word, occurrence #})]
    let (mut left_positions, mut right_positions): (Vec<_>, Vec<_>) =
        uncommon_shared_word_positions
            .flat_map(|(lefts, rights)| iter::zip(lefts, rights))
            .enumerate()
            .map(|(serial, (&left_pos, &right_pos))| ((left_pos, serial), (right_pos, serial)))
            .unzip();
    left_positions.sort_unstable_by_key(|&(pos, _serial)| pos);
    right_positions.sort_unstable_by_key(|&(pos, _serial)| pos);
    let left_index_by_right_index: Vec<usize> = {
        let mut left_index_map = vec![0; left_positions.len()];
        for (i, &(_pos, serial)) in left_positions.iter().enumerate() {
            left_index_map[serial] = i;
        }
        right_positions
            .iter()
            .map(|&(_pos, serial)| left_index_map[serial])
            .collect()
    };

    let lcs = find_lcs(&left_index_by_right_index);

    // Produce output word positions, recursing into the modified areas between
    // the elements in the LCS.
    let mut previous_left_position = WordPosition(0);
    let mut previous_right_position = WordPosition(0);
    for (left_index, right_index) in lcs {
        let (left_position, _) = left_positions[left_index];
        let (right_position, _) = right_positions[right_index];
        collect_unchanged_words(
            found_positions,
            &left.narrowed(previous_left_position..left_position),
            &right.narrowed(previous_right_position..right_position),
        );
        found_positions.push((
            left.map_to_global(left_position),
            right.map_to_global(right_position),
        ));
        previous_left_position = WordPosition(left_position.0 + 1);
        previous_right_position = WordPosition(right_position.0 + 1);
    }
    // Also recurse into range at end (after common ranges).
    collect_unchanged_words(
        found_positions,
        &left.narrowed(previous_left_position..WordPosition(left.ranges.len())),
        &right.narrowed(previous_right_position..WordPosition(right.ranges.len())),
    );
}

/// Intersects two sorted sequences of `(base, other)` word positions by
/// `base`. `base` positions should refer to the same source text.
fn intersect_unchanged_words(
    current_positions: Vec<(WordPosition, Vec<WordPosition>)>,
    new_positions: &[(WordPosition, WordPosition)],
) -> Vec<(WordPosition, Vec<WordPosition>)> {
    itertools::merge_join_by(
        current_positions,
        new_positions,
        |(cur_base_pos, _), (new_base_pos, _)| cur_base_pos.cmp(new_base_pos),
    )
    .filter_map(|entry| entry.both())
    .map(|((base_pos, mut other_positions), &(_, new_other_pos))| {
        other_positions.push(new_other_pos);
        (base_pos, other_positions)
    })
    .collect()
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct UnchangedRange {
    base: Range<usize>,
    others: Vec<Range<usize>>,
}

impl UnchangedRange {
    /// Translates word positions to byte ranges in the source texts.
    fn from_word_positions(
        base_source: &DiffSource,
        other_sources: &[DiffSource],
        base_position: WordPosition,
        other_positions: &[WordPosition],
    ) -> Self {
        assert_eq!(other_sources.len(), other_positions.len());
        let base = base_source.range_at(base_position);
        let others = iter::zip(other_sources, other_positions)
            .map(|(source, pos)| source.range_at(*pos))
            .collect();
        UnchangedRange { base, others }
    }
}

impl PartialOrd for UnchangedRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UnchangedRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.base
            .start
            .cmp(&other.base.start)
            .then_with(|| self.base.end.cmp(&other.base.end))
    }
}

/// Takes any number of inputs and finds regions that are them same between all
/// of them.
#[derive(Clone, Debug)]
pub struct Diff<'input> {
    base_input: &'input BStr,
    other_inputs: Vec<&'input BStr>,
    unchanged_regions: Vec<UnchangedRange>,
}

impl<'input> Diff<'input> {
    pub fn for_tokenizer<T: AsRef<[u8]> + ?Sized + 'input>(
        inputs: impl IntoIterator<Item = &'input T>,
        tokenizer: impl Fn(&[u8]) -> Vec<Range<usize>>,
    ) -> Self {
        let mut inputs = inputs.into_iter().map(BStr::new);
        let base_input = inputs.next().expect("inputs must not be empty");
        let other_inputs = inputs.collect_vec();
        // First tokenize each input
        let base_token_ranges: Vec<Range<usize>>;
        let other_token_ranges: Vec<Vec<Range<usize>>>;
        // No need to tokenize if one of the inputs is empty. Non-empty inputs
        // are all different.
        if base_input.is_empty() || other_inputs.iter().any(|input| input.is_empty()) {
            base_token_ranges = vec![];
            other_token_ranges = iter::repeat(vec![]).take(other_inputs.len()).collect();
        } else {
            base_token_ranges = tokenizer(base_input);
            other_token_ranges = other_inputs
                .iter()
                .map(|other_input| tokenizer(other_input))
                .collect();
        }
        Self::with_inputs_and_token_ranges(
            base_input,
            other_inputs,
            &base_token_ranges,
            &other_token_ranges,
        )
    }

    fn with_inputs_and_token_ranges(
        base_input: &'input BStr,
        other_inputs: Vec<&'input BStr>,
        base_token_ranges: &[Range<usize>],
        other_token_ranges: &[Vec<Range<usize>>],
    ) -> Self {
        assert_eq!(other_inputs.len(), other_token_ranges.len());
        let base_source = DiffSource::new(base_input, base_token_ranges);
        let other_sources = iter::zip(&other_inputs, other_token_ranges)
            .map(|(input, token_ranges)| DiffSource::new(input, token_ranges))
            .collect_vec();
        let mut unchanged_regions = match &*other_sources {
            // Consider the whole range of the base input as unchanged compared
            // to itself.
            [] => {
                let whole_range = UnchangedRange {
                    base: 0..base_source.text.len(),
                    others: vec![],
                };
                vec![whole_range]
            }
            // Diff each other input against the base. Intersect the previously
            // found ranges with the ranges in the diff.
            [first_other_source, tail_other_sources @ ..] => {
                let mut first_positions = Vec::new();
                collect_unchanged_words(&mut first_positions, &base_source, first_other_source);
                if tail_other_sources.is_empty() {
                    first_positions
                        .iter()
                        .map(|&(base_pos, other_pos)| {
                            UnchangedRange::from_word_positions(
                                &base_source,
                                &other_sources,
                                base_pos,
                                &[other_pos],
                            )
                        })
                        .collect()
                } else {
                    let first_positions = first_positions
                        .iter()
                        .map(|&(base_pos, other_pos)| (base_pos, vec![other_pos]))
                        .collect();
                    let intersected_positions = tail_other_sources.iter().fold(
                        first_positions,
                        |current_positions, other_source| {
                            let mut new_positions = Vec::new();
                            collect_unchanged_words(&mut new_positions, &base_source, other_source);
                            intersect_unchanged_words(current_positions, &new_positions)
                        },
                    );
                    intersected_positions
                        .iter()
                        .map(|(base_pos, other_positions)| {
                            UnchangedRange::from_word_positions(
                                &base_source,
                                &other_sources,
                                *base_pos,
                                other_positions,
                            )
                        })
                        .collect()
                }
            }
        };
        // Add an empty range at the end to make life easier for hunks().
        unchanged_regions.push(UnchangedRange {
            base: base_input.len()..base_input.len(),
            others: other_inputs
                .iter()
                .map(|input| input.len()..input.len())
                .collect(),
        });

        let mut diff = Self {
            base_input,
            other_inputs,
            unchanged_regions,
        };
        diff.compact_unchanged_regions();
        diff
    }

    pub fn unrefined<T: AsRef<[u8]> + ?Sized + 'input>(
        inputs: impl IntoIterator<Item = &'input T>,
    ) -> Self {
        Diff::for_tokenizer(inputs, |_| vec![])
    }

    /// Compares `inputs` line by line.
    pub fn by_line<T: AsRef<[u8]> + ?Sized + 'input>(
        inputs: impl IntoIterator<Item = &'input T>,
    ) -> Self {
        Diff::for_tokenizer(inputs, find_line_ranges)
    }

    /// Compares `inputs` word by word.
    ///
    /// The `inputs` is usually a changed hunk (e.g. a `DiffHunk::Different`)
    /// that was the output from a line-by-line diff.
    pub fn by_word<T: AsRef<[u8]> + ?Sized + 'input>(
        inputs: impl IntoIterator<Item = &'input T>,
    ) -> Self {
        let mut diff = Diff::for_tokenizer(inputs, find_word_ranges);
        diff.refine_changed_regions(find_nonword_ranges);
        diff
    }

    pub fn hunks<'diff>(&'diff self) -> DiffHunkIterator<'diff, 'input> {
        DiffHunkIterator {
            diff: self,
            previous: UnchangedRange {
                base: 0..0,
                others: vec![0..0; self.other_inputs.len()],
            },
            unchanged_emitted: true,
            unchanged_iter: self.unchanged_regions.iter(),
        }
    }

    /// Returns contents between the `previous` ends and the `current` starts.
    fn hunk_between<'a>(
        &'a self,
        previous: &'a UnchangedRange,
        current: &'a UnchangedRange,
    ) -> impl Iterator<Item = &'input BStr> + 'a {
        itertools::chain(
            iter::once(&self.base_input[previous.base.end..current.base.start]),
            itertools::izip!(&self.other_inputs, &previous.others, &current.others)
                .map(|(input, prev, cur)| &input[prev.end..cur.start]),
        )
    }

    /// Uses the given tokenizer to split the changed regions into smaller
    /// regions. Then tries to finds unchanged regions among them.
    pub fn refine_changed_regions(&mut self, tokenizer: impl Fn(&[u8]) -> Vec<Range<usize>>) {
        let mut previous = UnchangedRange {
            base: 0..0,
            others: vec![0..0; self.other_inputs.len()],
        };
        let mut new_unchanged_ranges = vec![];
        for current in self.unchanged_regions.iter() {
            // For the changed region between the previous region and the current one,
            // create a new Diff instance. Then adjust the start positions and
            // offsets to be valid in the context of the larger Diff instance
            // (`self`).
            let refined_diff =
                Diff::for_tokenizer(self.hunk_between(&previous, current), &tokenizer);
            for refined in &refined_diff.unchanged_regions {
                let new_base_start = refined.base.start + previous.base.end;
                let new_base_end = refined.base.end + previous.base.end;
                let new_others = iter::zip(&refined.others, &previous.others)
                    .map(|(refi, prev)| (refi.start + prev.end)..(refi.end + prev.end))
                    .collect();
                new_unchanged_ranges.push(UnchangedRange {
                    base: new_base_start..new_base_end,
                    others: new_others,
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
                if previous.base.end == current.base.start
                    && iter::zip(&previous.others, &current.others)
                        .all(|(prev, cur)| prev.end == cur.start)
                {
                    maybe_previous = Some(UnchangedRange {
                        base: previous.base.start..current.base.end,
                        others: iter::zip(&previous.others, &current.others)
                            .map(|(prev, cur)| prev.start..cur.end)
                            .collect(),
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

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum DiffHunk<'input> {
    Matching(&'input BStr),
    Different(Vec<&'input BStr>),
}

impl<'input> DiffHunk<'input> {
    pub fn matching<T: AsRef<[u8]> + ?Sized>(content: &'input T) -> Self {
        DiffHunk::Matching(BStr::new(content))
    }

    pub fn different<T: AsRef<[u8]> + ?Sized + 'input>(
        contents: impl IntoIterator<Item = &'input T>,
    ) -> Self {
        DiffHunk::Different(contents.into_iter().map(BStr::new).collect())
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
                if !self.previous.base.is_empty() {
                    return Some(DiffHunk::Matching(
                        &self.diff.base_input[self.previous.base.clone()],
                    ));
                }
            }
            if let Some(current) = self.unchanged_iter.next() {
                let slices = self
                    .diff
                    .hunk_between(&self.previous, current)
                    .collect_vec();
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

/// Diffs slices of bytes. The returned diff hunks may be any length (may
/// span many lines or may be only part of a line). This currently uses
/// Histogram diff (or maybe something similar; I'm not sure I understood the
/// algorithm correctly). It first diffs lines in the input and then refines
/// the changed ranges at the word level.
pub fn diff<'a, T: AsRef<[u8]> + ?Sized + 'a>(
    inputs: impl IntoIterator<Item = &'a T>,
) -> Vec<DiffHunk<'a>> {
    let mut diff = Diff::for_tokenizer(inputs, find_line_ranges);
    diff.refine_changed_regions(find_word_ranges);
    diff.refine_changed_regions(find_nonword_ranges);
    diff.hunks().collect()
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

    fn unchanged_ranges(
        left: &DiffSource,
        right: &DiffSource,
    ) -> Vec<(Range<usize>, Range<usize>)> {
        let mut positions = Vec::new();
        collect_unchanged_words(&mut positions, left, right);
        positions
            .into_iter()
            .map(|(left_pos, right_pos)| (left.range_at(left_pos), right.range_at(right_pos)))
            .collect()
    }

    #[test]
    fn test_unchanged_ranges_insert_in_middle() {
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a b b c", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a b X b c", &[0..1, 2..3, 4..5, 6..7, 8..9]),
            ),
            vec![(0..1, 0..1), (2..3, 2..3), (4..5, 6..7), (6..7, 8..9)]
        );
    }

    #[test]
    fn test_unchanged_ranges_non_unique_removed() {
        // We used to consider the first two "a" in the first input to match the two
        // "a"s in the second input. We no longer do.
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a b a c", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 0..1)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"b a c a", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(6..7, 6..7)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"b a a c", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a b c a", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 0..1), (6..7, 6..7)]
        );
    }

    #[test]
    fn test_unchanged_ranges_non_unique_added() {
        // We used to consider the first two "a" in the first input to match the two
        // "a"s in the second input. We no longer do.
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a b a c", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 0..1)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"b a c a", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(6..7, 6..7)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"b a a c", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a b c a", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a a a a", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 0..1), (6..7, 6..7)]
        );
    }

    #[test]
    fn test_unchanged_ranges_recursion_needed() {
        // "|" matches first, then "b" matches within the left/right range.
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a b | b", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"b c d |", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(2..3, 0..1), (4..5, 6..7)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"| b c d", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"b | a b", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 2..3), (2..3, 6..7)]
        );
        // "|" matches first, then the middle range is trimmed.
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"| b c |", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"| b b |", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 0..1), (2..3, 2..3), (6..7, 6..7)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"| c c |", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"| b c |", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 0..1), (4..5, 4..5), (6..7, 6..7)]
        );
        // "|" matches first, then "a", then "b".
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"a b c | a", &[0..1, 2..3, 4..5, 6..7, 8..9]),
                &DiffSource::new(b"b a b |", &[0..1, 2..3, 4..5, 6..7]),
            ),
            vec![(0..1, 2..3), (2..3, 4..5), (6..7, 6..7)]
        );
        assert_eq!(
            unchanged_ranges(
                &DiffSource::new(b"| b a b", &[0..1, 2..3, 4..5, 6..7]),
                &DiffSource::new(b"a | a b c", &[0..1, 2..3, 4..5, 6..7, 8..9]),
            ),
            vec![(0..1, 2..3), (4..5, 4..5), (6..7, 6..7)]
        );
    }

    #[test]
    fn test_diff_single_input() {
        assert_eq!(diff(["abc"]), vec![DiffHunk::matching("abc")]);
    }

    #[test]
    fn test_diff_some_empty_inputs() {
        // All empty
        assert_eq!(diff([""]), vec![]);
        assert_eq!(diff(["", ""]), vec![]);
        assert_eq!(diff(["", "", ""]), vec![]);

        // One empty
        assert_eq!(diff(["a b", ""]), vec![DiffHunk::different(["a b", ""])]);
        assert_eq!(diff(["", "a b"]), vec![DiffHunk::different(["", "a b"])]);

        // One empty, two match
        assert_eq!(
            diff(["a b", "", "a b"]),
            vec![DiffHunk::different(["a b", "", "a b"])]
        );
        assert_eq!(
            diff(["", "a b", "a b"]),
            vec![DiffHunk::different(["", "a b", "a b"])]
        );

        // Two empty, one differs
        assert_eq!(
            diff(["a b", "", ""]),
            vec![DiffHunk::different(["a b", "", ""])]
        );
        assert_eq!(
            diff(["", "a b", ""]),
            vec![DiffHunk::different(["", "a b", ""])]
        );
    }

    #[test]
    fn test_diff_two_inputs_one_different() {
        assert_eq!(
            diff(["a b c", "a X c"]),
            vec![
                DiffHunk::matching("a "),
                DiffHunk::different(["b", "X"]),
                DiffHunk::matching(" c"),
            ]
        );
    }

    #[test]
    fn test_diff_multiple_inputs_one_different() {
        assert_eq!(
            diff(["a b c", "a X c", "a b c"]),
            vec![
                DiffHunk::matching("a "),
                DiffHunk::different(["b", "X", "b"]),
                DiffHunk::matching(" c"),
            ]
        );
    }

    #[test]
    fn test_diff_multiple_inputs_all_different() {
        assert_eq!(
            diff(["a b c", "a X c", "a c X"]),
            vec![
                DiffHunk::matching("a "),
                DiffHunk::different(["b ", "X ", ""]),
                DiffHunk::matching("c"),
                DiffHunk::different(["", "", " X"]),
            ]
        );
    }

    #[test]
    fn test_diff_for_tokenizer_compacted() {
        // Tests that unchanged regions are compacted when using for_tokenizer()
        let diff = Diff::for_tokenizer(
            ["a\nb\nc\nd\ne\nf\ng", "a\nb\nc\nX\ne\nf\ng"],
            find_line_ranges,
        );
        assert_eq!(
            diff.hunks().collect_vec(),
            vec![
                DiffHunk::matching("a\nb\nc\n"),
                DiffHunk::different(["d\n", "X\n"]),
                DiffHunk::matching("e\nf\ng"),
            ]
        );
    }

    #[test]
    fn test_diff_nothing_in_common() {
        assert_eq!(
            diff(["aaa", "bb"]),
            vec![DiffHunk::different(["aaa", "bb"])]
        );
    }

    #[test]
    fn test_diff_insert_in_middle() {
        assert_eq!(
            diff(["a z", "a S z"]),
            vec![
                DiffHunk::matching("a "),
                DiffHunk::different(["", "S "]),
                DiffHunk::matching("z"),
            ]
        );
    }

    #[test]
    fn test_diff_no_unique_middle_flips() {
        assert_eq!(
            diff(["a R R S S z", "a S S R R z"]),
            vec![
                DiffHunk::matching("a "),
                DiffHunk::different(["R R ", ""]),
                DiffHunk::matching("S S "),
                DiffHunk::different(["", "R R "]),
                DiffHunk::matching("z")
            ],
        );
    }

    #[test]
    fn test_diff_recursion_needed() {
        assert_eq!(
            diff([
                "a q x q y q z q b q y q x q c",
                "a r r x q y z q b y q x r r c",
            ]),
            vec![
                DiffHunk::matching("a "),
                DiffHunk::different(["q", "r"]),
                DiffHunk::matching(" "),
                DiffHunk::different(["", "r "]),
                DiffHunk::matching("x q y "),
                DiffHunk::different(["q ", ""]),
                DiffHunk::matching("z q b "),
                DiffHunk::different(["q ", ""]),
                DiffHunk::matching("y q x "),
                DiffHunk::different(["q", "r"]),
                DiffHunk::matching(" "),
                DiffHunk::different(["", "r "]),
                DiffHunk::matching("c"),
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
        #[rustfmt::skip]
        assert_eq!(
            diff([
                "    pub fn write_fmt(&mut self, fmt: fmt::Arguments<\'_>) {\n        self.styler().write_fmt(fmt).unwrap()\n",
                "    pub fn write_fmt(&mut self, fmt: fmt::Arguments<\'_>) -> io::Result<()> {\n        self.styler().write_fmt(fmt)\n"
            ]),
            vec![
                DiffHunk::matching("    pub fn write_fmt(&mut self, fmt: fmt::Arguments<\'_>) "),
                DiffHunk::different(["", "-> io::Result<()> "]),
                DiffHunk::matching("{\n        self.styler().write_fmt(fmt)"),
                DiffHunk::different([".unwrap()", ""]),
                DiffHunk::matching("\n")
            ]
        );
    }

    #[test]
    fn test_diff_real_case_gitgit_read_tree_c() {
        // This is the diff from commit e497ea2a9b in the git.git repo
        #[rustfmt::skip]
        assert_eq!(
            diff([
                r##"/*
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
                r##"/*
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
            ]),
            vec![
               DiffHunk::matching("/*\n * GIT - The information manager from hell\n *\n * Copyright (C) Linus Torvalds, 2005\n */\n#include \"#cache.h\"\n\n"),
               DiffHunk::different(["", "static void create_directories(const char *path)\n{\n\tint len = strlen(path);\n\tchar *buf = malloc(len + 1);\n\tconst char *slash = path;\n\n\twhile ((slash = strchr(slash+1, \'/\')) != NULL) {\n\t\tlen = slash - path;\n\t\tmemcpy(buf, path, len);\n\t\tbuf[len] = 0;\n\t\tmkdir(buf, 0700);\n\t}\n}\n\nstatic int create_file(const char *path)\n{\n\tint fd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);\n\tif (fd < 0) {\n\t\tif (errno == ENOENT) {\n\t\t\tcreate_directories(path);\n\t\t\tfd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);\n\t\t}\n\t}\n\treturn fd;\n}\n\n"]),
               DiffHunk::matching("static int unpack(unsigned char *sha1)\n{\n\tvoid *buffer;\n\tunsigned long size;\n\tchar type[20];\n\n\tbuffer = read_sha1_file(sha1, type, &size);\n\tif (!buffer)\n\t\tusage(\"unable to read sha1 file\");\n\tif (strcmp(type, \"tree\"))\n\t\tusage(\"expected a \'tree\' node\");\n\twhile (size) {\n\t\tint len = strlen(buffer)+1;\n\t\tunsigned char *sha1 = buffer + len;\n\t\tchar *path = strchr(buffer, \' \')+1;\n"),
               DiffHunk::different(["", "\t\tchar *data;\n\t\tunsigned long filesize;\n"]),
               DiffHunk::matching("\t\tunsigned int mode;\n"),
               DiffHunk::different(["", "\t\tint fd;\n\n"]),
               DiffHunk::matching("\t\tif (size < len + 20 || sscanf(buffer, \"%o\", &mode) != 1)\n\t\t\tusage(\"corrupt \'tree\' file\");\n\t\tbuffer = sha1 + 20;\n\t\tsize -= len + 20;\n\t\t"),
               DiffHunk::different(["printf(\"%o %s (%s)\\n\", mode, path,", "data ="]),
               DiffHunk::matching(" "),
               DiffHunk::different(["sha1_to_hex", "read_sha1_file"]),
               DiffHunk::matching("(sha1"),
               DiffHunk::different([")", ", type, &filesize);\n\t\tif (!data || strcmp(type, \"blob\"))\n\t\t\tusage(\"tree file refers to bad file data\");\n\t\tfd = create_file(path);\n\t\tif (fd < 0)\n\t\t\tusage(\"unable to create file\");\n\t\tif (write(fd, data, filesize) != filesize)\n\t\t\tusage(\"unable to write file\");\n\t\tfchmod(fd, mode);\n\t\tclose(fd);\n\t\tfree(data"]),
               DiffHunk::matching(");\n\t}\n\treturn 0;\n}\n\nint main(int argc, char **argv)\n{\n\tint fd;\n\tunsigned char sha1[20];\n\n\tif (argc != 2)\n\t\tusage(\"read-tree <key>\");\n\tif (get_sha1_hex(argv[1], sha1) < 0)\n\t\tusage(\"read-tree <key>\");\n\tsha1_file_directory = getenv(DB_ENVIRONMENT);\n\tif (!sha1_file_directory)\n\t\tsha1_file_directory = DEFAULT_DB_ENVIRONMENT;\n\tif (unpack(sha1) < 0)\n\t\tusage(\"unpack failed\");\n\treturn 0;\n}\n"),
            ]
        );
    }
}
