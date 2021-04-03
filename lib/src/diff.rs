use std::cmp::min;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use std::ops::Range;

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

pub fn find_word_ranges(text: &[u8]) -> Vec<Range<usize>> {
    let mut word_ranges = vec![];
    let mut word_start_pos = 0;
    let mut in_word = false;
    for (i, b) in text.iter().enumerate() {
        // TODO: Make this configurable (probably higher up in the call stack)
        let is_word_byte = matches!(*b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_');
        if in_word && !is_word_byte {
            in_word = false;
            word_ranges.push(word_start_pos..i);
            word_start_pos = i;
        } else if !in_word && is_word_byte {
            in_word = true;
            word_start_pos = i;
        }
    }
    if in_word && word_start_pos < text.len() {
        word_ranges.push(word_start_pos..text.len());
    }
    word_ranges
}

pub fn find_newline_ranges(text: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = vec![];
    for (i, b) in text.iter().enumerate() {
        if *b == b'\n' {
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

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum RangeDiff {
    Unchanged(Range<usize>, Range<usize>),
    Replaced(Range<usize>, Range<usize>),
}

impl RangeDiff {
    fn is_empty(&self) -> bool {
        match self {
            RangeDiff::Unchanged(left_range, right_range) => {
                left_range.is_empty() && right_range.is_empty()
            }
            RangeDiff::Replaced(left_range, right_range) => {
                left_range.is_empty() && right_range.is_empty()
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SliceDiff<'a> {
    Unchanged(&'a [u8]),
    Replaced(&'a [u8], &'a [u8]),
}

impl Debug for SliceDiff<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            SliceDiff::Unchanged(data) => f
                .debug_tuple("Unchanged")
                .field(&String::from_utf8_lossy(data))
                .finish(),
            SliceDiff::Replaced(left, right) => f
                .debug_tuple("Replaced")
                .field(&String::from_utf8_lossy(left))
                .field(&String::from_utf8_lossy(right))
                .finish(),
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
    let mut longest = 0;
    let mut longest_right_pos = 0;
    for (right_pos, &left_pos) in input.iter().enumerate() {
        chain[right_pos] = (1, left_pos, usize::MAX);
        for i in (0..right_pos).rev() {
            let (previous_len, previous_left_pos, _) = chain[i];
            if previous_left_pos < left_pos {
                let len = previous_len + 1;
                chain[right_pos] = (len, left_pos, i);
                if len > longest {
                    longest = len;
                    longest_right_pos = right_pos;
                }
                break;
            }
        }
    }

    let mut result = vec![];
    let mut right_pos = longest_right_pos;
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

    // TODO: Don't waste time calculating entire histogram. We don't need to keep
    // data about common entries. If a word has more than N occurrences, we should
    // just ignore it (and assume that everything changes if we have no less common
    // words).
    let max_occurrences = 100;
    let mut left_histogram = Histogram::calculate(left, left_ranges, max_occurrences);
    if *left_histogram.count_to_words.first_entry().unwrap().key() > max_occurrences {
        // If there are very many occurrences of all words, then we just give up.
        return vec![];
    }
    let mut right_histogram = Histogram::calculate(right, right_ranges, max_occurrences);
    // Look for words with few occurrences in `left` (could equally well have picked
    // `right`?). If any of them also occur in `right`, then we add the words to
    // the LCS.
    let mut uncommon_shared_words = vec![];
    while !left_histogram.count_to_words.is_empty() && uncommon_shared_words.is_empty() {
        let left_words = left_histogram.count_to_words.pop_first().unwrap().1;
        for left_word in left_words {
            if right_histogram.word_to_positions.contains_key(left_word) {
                uncommon_shared_words.push(left_word);
            }
        }
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

    result
}

/// Adds ranges between around the `input` ranges so that the full ranges of
/// `left` and `right` are covered.
fn fill_in_range_gaps(
    left: &[u8],
    right: &[u8],
    input: &[(Range<usize>, Range<usize>)],
) -> Vec<RangeDiff> {
    let mut output = vec![];
    let mut previous_left_end_pos = 0;
    let mut previous_right_end_pos = 0;
    // Add an empty range at the end in order to fill in any gap just before the
    // end (without needing to duplicate code for that after the loop).
    for (left_range, right_range) in input
        .iter()
        .chain(&[(left.len()..left.len(), right.len()..right.len())])
    {
        let left_gap_range = previous_left_end_pos..left_range.start;
        let right_gap_range = previous_right_end_pos..right_range.start;
        if !left_gap_range.is_empty() || !right_gap_range.is_empty() {
            if left[left_gap_range.clone()] == right[right_gap_range.clone()] {
                output.push(RangeDiff::Unchanged(left_gap_range, right_gap_range));
            } else {
                output.push(RangeDiff::Replaced(left_gap_range, right_gap_range));
            }
        }
        previous_left_end_pos = left_range.end;
        previous_right_end_pos = right_range.end;
        if !(left_range.is_empty() && right_range.is_empty()) {
            output.push(RangeDiff::Unchanged(
                left_range.clone(),
                right_range.clone(),
            ));
        }
    }

    output
}

/// Combines adjacent ranges of the same type into larger ranges. Removes empty
/// ranges.
fn compact_ranges(input: &[RangeDiff]) -> Vec<RangeDiff> {
    if input.is_empty() {
        return vec![];
    }
    let mut output = vec![];
    let mut current_range = input[0].clone();
    for range in input.iter().skip(1) {
        match (&mut current_range, range) {
            (RangeDiff::Unchanged(left1, right1), RangeDiff::Unchanged(left2, right2)) => {
                left1.end = left2.end;
                right1.end = right2.end;
            }
            (RangeDiff::Replaced(left1, right1), RangeDiff::Replaced(left2, right2)) => {
                left1.end = left2.end;
                right1.end = right2.end;
            }
            _ => {
                // The previous range was unchanged and this one was replaced, or vice versa.
                // If the new range is empty, just ignore it, so we can possibly compact
                // with the previous one.
                if !range.is_empty() {
                    if !current_range.is_empty() {
                        output.push(current_range.clone());
                    }
                    current_range = range.clone();
                }
            }
        }
    }
    if !current_range.is_empty() {
        output.push(current_range);
    }
    output
}

fn refine_changed_ranges<'a>(
    left: &'a [u8],
    right: &'a [u8],
    input: &[RangeDiff],
    tokenizer: &impl Fn(&[u8]) -> Vec<Range<usize>>,
) -> Vec<RangeDiff> {
    let mut output = vec![];
    for range_diff in input {
        match range_diff {
            RangeDiff::Replaced(left_range, right_range) => {
                let left_slice = &left[left_range.clone()];
                let right_slice = &right[right_range.clone()];
                let refined_left_ranges: Vec<Range<usize>> = tokenizer(&left_slice);
                let refined_right_ranges: Vec<Range<usize>> = tokenizer(&right_slice);
                let unchanged_refined_ranges = unchanged_ranges(
                    &left_slice,
                    &right_slice,
                    &refined_left_ranges,
                    &refined_right_ranges,
                );
                let all_refined_ranges =
                    fill_in_range_gaps(left_slice, right_slice, &unchanged_refined_ranges);
                let compacted_refined_range_diffs = compact_ranges(&all_refined_ranges);
                for refined_range_diff in compacted_refined_range_diffs {
                    match refined_range_diff {
                        RangeDiff::Unchanged(refined_left_range, refined_right_range) => output
                            .push(RangeDiff::Unchanged(
                                left_range.start + refined_left_range.start
                                    ..left_range.start + refined_left_range.end,
                                right_range.start + refined_right_range.start
                                    ..right_range.start + refined_right_range.end,
                            )),
                        RangeDiff::Replaced(refined_left_range, refined_right_range) => output
                            .push(RangeDiff::Replaced(
                                left_range.start + refined_left_range.start
                                    ..left_range.start + refined_left_range.end,
                                right_range.start + refined_right_range.start
                                    ..right_range.start + refined_right_range.end,
                            )),
                    }
                }
            }
            range => {
                output.push(range.clone());
            }
        }
    }
    output
}

fn range_diffs_to_slice_diffs<'a>(
    left: &'a [u8],
    right: &'a [u8],
    range_diffs: &[RangeDiff],
) -> Vec<SliceDiff<'a>> {
    let mut slice_diffs = vec![];
    for range in range_diffs {
        match range {
            RangeDiff::Unchanged(left_range, _right_range) => {
                slice_diffs.push(SliceDiff::Unchanged(&left[left_range.clone()]));
            }
            RangeDiff::Replaced(left_range, right_range) => {
                slice_diffs.push(SliceDiff::Replaced(
                    &left[left_range.clone()],
                    &right[right_range.clone()],
                ));
            }
        }
    }
    slice_diffs
}

/// Diffs two slices of bytes. The returned diff hunks may be any length (may
/// span many lines or may be only part of a line). This currently uses
/// Histogram diff (or maybe something similar; I'm not sure I understood the
/// algorithm correctly). It first diffs lines in the input and then refines
/// the changed ranges at the word level.
///
/// TODO: Diff at even lower level in the non-word ranges?
pub fn diff<'a>(left: &'a [u8], right: &'a [u8]) -> Vec<SliceDiff<'a>> {
    if left == right {
        return vec![SliceDiff::Unchanged(left)];
    }
    if left.is_empty() {
        return vec![SliceDiff::Replaced(b"", right)];
    }
    if right.is_empty() {
        return vec![SliceDiff::Replaced(left, b"")];
    }

    let range_diffs = vec![RangeDiff::Replaced(0..left.len(), 0..right.len())];
    let range_diffs = refine_changed_ranges(left, right, &range_diffs, &find_line_ranges);
    let range_diffs = refine_changed_ranges(left, right, &range_diffs, &find_word_ranges);
    let range_diffs = refine_changed_ranges(left, right, &range_diffs, &find_newline_ranges);
    range_diffs_to_slice_diffs(left, right, &range_diffs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_line_ranges_empty() {
        assert_eq!(find_line_ranges(b""), vec![]);
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
        assert_eq!(find_word_ranges(b""), vec![]);
    }

    #[test]
    fn test_find_word_ranges_single_word() {
        assert_eq!(find_word_ranges(b"Abc"), vec![0..3]);
    }

    #[test]
    fn test_find_word_ranges_no_word() {
        assert_eq!(find_word_ranges(b"+-*/"), vec![]);
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
    fn test_fill_in_gaps_empty() {
        assert_eq!(
            fill_in_range_gaps(b"abc", b"abcde", &[]),
            vec![RangeDiff::Replaced(0..3, 0..5),]
        );
    }

    #[test]
    fn test_fill_in_gaps_only_middle() {
        assert_eq!(
            fill_in_range_gaps(
                b"a b c",
                b"a x b y c",
                &[(0..2, 0..2), (2..4, 4..6), (4..5, 8..9),]
            ),
            vec![
                RangeDiff::Unchanged(0..2, 0..2),
                RangeDiff::Replaced(2..2, 2..4),
                RangeDiff::Unchanged(2..4, 4..6),
                RangeDiff::Replaced(4..4, 6..8),
                RangeDiff::Unchanged(4..5, 8..9),
            ]
        );
    }

    #[test]
    fn test_fill_in_gaps_empty_gap() {
        assert_eq!(
            fill_in_range_gaps(b"a b", b"a b", &[(0..1, 0..1), (1..2, 1..2), (2..3, 2..3),]),
            vec![
                RangeDiff::Unchanged(0..1, 0..1),
                RangeDiff::Unchanged(1..2, 1..2),
                RangeDiff::Unchanged(2..3, 2..3),
            ]
        );
    }

    #[test]
    fn test_fill_in_gaps_before_and_after() {
        assert_eq!(
            fill_in_range_gaps(b" a ", b" a ", &[(1..2, 1..2),]),
            vec![
                RangeDiff::Unchanged(0..1, 0..1),
                RangeDiff::Unchanged(1..2, 1..2),
                RangeDiff::Unchanged(2..3, 2..3),
            ]
        );
    }

    #[test]
    fn test_compact_ranges_all_unchanged() {
        assert_eq!(
            compact_ranges(&[
                RangeDiff::Unchanged(0..1, 0..2),
                RangeDiff::Unchanged(1..2, 2..4),
                RangeDiff::Unchanged(2..3, 4..6),
            ]),
            vec![RangeDiff::Unchanged(0..3, 0..6),]
        );
    }

    #[test]
    fn test_compact_ranges_all_replaced() {
        assert_eq!(
            compact_ranges(&[
                RangeDiff::Replaced(0..1, 0..2),
                RangeDiff::Replaced(1..2, 2..4),
                RangeDiff::Replaced(2..3, 4..6),
            ]),
            vec![RangeDiff::Replaced(0..3, 0..6),]
        );
    }

    #[test]
    fn test_compact_ranges_mixed() {
        assert_eq!(
            compact_ranges(&[
                RangeDiff::Replaced(0..1, 0..2),
                RangeDiff::Replaced(1..2, 2..4),
                RangeDiff::Unchanged(2..3, 4..6),
                RangeDiff::Unchanged(3..4, 6..8),
                RangeDiff::Replaced(4..5, 8..10),
                RangeDiff::Replaced(5..6, 10..12),
            ]),
            vec![
                RangeDiff::Replaced(0..2, 0..4),
                RangeDiff::Unchanged(2..4, 4..8),
                RangeDiff::Replaced(4..6, 8..12),
            ]
        );
    }

    #[test]
    fn test_compact_ranges_mixed_empty_range() {
        assert_eq!(
            compact_ranges(&[
                RangeDiff::Replaced(0..1, 0..2),
                RangeDiff::Replaced(1..2, 2..4),
                RangeDiff::Unchanged(2..2, 4..4),
                RangeDiff::Replaced(3..4, 6..8),
                RangeDiff::Replaced(4..5, 8..10),
            ]),
            vec![RangeDiff::Replaced(0..5, 0..10)]
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
    fn test_diff_nothing_in_common() {
        assert_eq!(
            diff(b"aaa", b"bb"),
            vec![SliceDiff::Replaced(b"aaa", b"bb")]
        );
    }

    #[test]
    fn test_diff_insert_in_middle() {
        assert_eq!(
            diff(b"a z", b"a S z"),
            vec![
                SliceDiff::Unchanged(b"a"),
                SliceDiff::Replaced(b" ", b" S "),
                SliceDiff::Unchanged(b"z"),
            ]
        );
    }

    #[test]
    fn test_diff_no_unique_middle_flips() {
        assert_eq!(
            diff(b"a R R S S z", b"a S S R R z"),
            vec![
                SliceDiff::Unchanged(b"a"),
                SliceDiff::Replaced(b" R R ", b" "),
                SliceDiff::Unchanged(b"S S"),
                SliceDiff::Replaced(b" ", b" R R "),
                SliceDiff::Unchanged(b"z")
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
                SliceDiff::Unchanged(b"a"),
                SliceDiff::Replaced(b" q ", b" r r "),
                SliceDiff::Unchanged(b"x q y"),
                SliceDiff::Replaced(b" q ", b" "),
                SliceDiff::Unchanged(b"z q b"),
                SliceDiff::Replaced(b" q ", b" "),
                SliceDiff::Unchanged(b"y q x"),
                SliceDiff::Replaced(b" q ", b" r r "),
                SliceDiff::Unchanged(b"c"),
            ]
        );
    }

    #[test]
    fn test_diff_gitgit_read_tree_c() {
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
            // TODO: Move matching whitespace at ends of replaced section out into unchanged section
            vec![
               SliceDiff::Unchanged(b"/*\n * GIT - The information manager from hell\n *\n * Copyright (C) Linus Torvalds, 2005\n */\n#include \"#cache.h\"\n\n"),
               SliceDiff::Replaced(b"", b"static void create_directories(const char *path)\n{\n\tint len = strlen(path);\n\tchar *buf = malloc(len + 1);\n\tconst char *slash = path;\n\n\twhile ((slash = strchr(slash+1, \'/\')) != NULL) {\n\t\tlen = slash - path;\n\t\tmemcpy(buf, path, len);\n\t\tbuf[len] = 0;\n\t\tmkdir(buf, 0700);\n\t}\n}\n\nstatic int create_file(const char *path)\n{\n\tint fd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);\n\tif (fd < 0) {\n\t\tif (errno == ENOENT) {\n\t\t\tcreate_directories(path);\n\t\t\tfd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0600);\n\t\t}\n\t}\n\treturn fd;\n}\n\n"),
               SliceDiff::Unchanged(b"static int unpack(unsigned char *sha1)\n{\n\tvoid *buffer;\n\tunsigned long size;\n\tchar type[20];\n\n\tbuffer = read_sha1_file(sha1, type, &size);\n\tif (!buffer)\n\t\tusage(\"unable to read sha1 file\");\n\tif (strcmp(type, \"tree\"))\n\t\tusage(\"expected a \'tree\' node\");\n\twhile (size) {\n\t\tint len = strlen(buffer)+1;\n\t\tunsigned char *sha1 = buffer + len;\n\t\tchar *path = strchr(buffer, \' \')+1;\n"),
               SliceDiff::Replaced(b"", b"\t\tchar *data;\n\t\tunsigned long filesize;\n"),
               SliceDiff::Unchanged(b"\t\tunsigned int mode;\n"),
               SliceDiff::Replaced(b"", b"\t\tint fd;\n\n"),
               SliceDiff::Unchanged(b"\t\tif (size < len + 20 || sscanf(buffer, \"%o\", &mode) != 1)\n\t\t\tusage(\"corrupt \'tree\' file\");\n\t\tbuffer = sha1 + 20;\n\t\tsize -= len + 20;\n"),
               SliceDiff::Replaced(b"\t\tprintf(\"%o %s (%s)\\n\", mode, path, sha1_to_hex(", b"\t\tdata = read_sha1_file("),
               SliceDiff::Unchanged(b"sha1"),
               SliceDiff::Replaced(b"));", b", type, &filesize);"),
               SliceDiff::Unchanged(b"\n"),
               SliceDiff::Replaced(b"", b"\t\tif (!data || strcmp(type, \"blob\"))\n\t\t\tusage(\"tree file refers to bad file data\");\n\t\tfd = create_file(path);\n\t\tif (fd < 0)\n\t\t\tusage(\"unable to create file\");\n\t\tif (write(fd, data, filesize) != filesize)\n\t\t\tusage(\"unable to write file\");\n\t\tfchmod(fd, mode);\n\t\tclose(fd);\n\t\tfree(data);\n"),
               SliceDiff::Unchanged(b"\t}\n\treturn 0;\n}\n\nint main(int argc, char **argv)\n{\n\tint fd;\n\tunsigned char sha1[20];\n\n\tif (argc != 2)\n\t\tusage(\"read-tree <key>\");\n\tif (get_sha1_hex(argv[1], sha1) < 0)\n\t\tusage(\"read-tree <key>\");\n\tsha1_file_directory = getenv(DB_ENVIRONMENT);\n\tif (!sha1_file_directory)\n\t\tsha1_file_directory = DEFAULT_DB_ENVIRONMENT;\n\tif (unpack(sha1) < 0)\n\t\tusage(\"unpack failed\");\n\treturn 0;\n}\n")
            ]
        );
    }
}
