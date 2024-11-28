// Copyright 2022-2023 The Jujutsu Authors
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

use std::borrow::Cow;
use std::cmp;
use std::io;

use bstr::ByteSlice as _;
use unicode_width::UnicodeWidthChar as _;
use unicode_width::UnicodeWidthStr as _;

use crate::formatter::FormatRecorder;
use crate::formatter::Formatter;

pub fn complete_newline(s: impl Into<String>) -> String {
    let mut s = s.into();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn split_email(email: &str) -> (&str, Option<&str>) {
    if let Some((username, rest)) = email.split_once('@') {
        (username, Some(rest))
    } else {
        (email, None)
    }
}

/// Shortens `text` to `max_width` by removing leading characters. `ellipsis` is
/// added if the `text` gets truncated.
///
/// The returned string (including `ellipsis`) never exceeds the `max_width`.
pub fn elide_start<'a>(
    text: &'a str,
    ellipsis: &'a str,
    max_width: usize,
) -> (Cow<'a, str>, usize) {
    let (text_start, text_width) = truncate_start_pos(text, max_width);
    if text_start == 0 {
        return (Cow::Borrowed(text), text_width);
    }

    let (ellipsis_start, ellipsis_width) = truncate_start_pos(ellipsis, max_width);
    if ellipsis_start != 0 {
        let ellipsis = trim_start_zero_width_chars(&ellipsis[ellipsis_start..]);
        return (Cow::Borrowed(ellipsis), ellipsis_width);
    }

    let text = &text[text_start..];
    let max_text_width = max_width - ellipsis_width;
    let (skip, skipped_width) = skip_start_pos(text, text_width.saturating_sub(max_text_width));
    let text = trim_start_zero_width_chars(&text[skip..]);
    let concat_width = ellipsis_width + (text_width - skipped_width);
    assert!(concat_width <= max_width);
    (Cow::Owned([ellipsis, text].concat()), concat_width)
}

/// Shortens `text` to `max_width` by removing trailing characters. `ellipsis`
/// is added if the `text` gets truncated.
///
/// The returned string (including `ellipsis`) never exceeds the `max_width`.
pub fn elide_end<'a>(text: &'a str, ellipsis: &'a str, max_width: usize) -> (Cow<'a, str>, usize) {
    let (text_end, text_width) = truncate_end_pos(text, max_width);
    if text_end == text.len() {
        return (Cow::Borrowed(text), text_width);
    }

    let (ellipsis_end, ellipsis_width) = truncate_end_pos(ellipsis, max_width);
    if ellipsis_end != ellipsis.len() {
        let ellipsis = &ellipsis[..ellipsis_end];
        return (Cow::Borrowed(ellipsis), ellipsis_width);
    }

    let text = &text[..text_end];
    let max_text_width = max_width - ellipsis_width;
    let (skip, skipped_width) = skip_end_pos(text, text_width.saturating_sub(max_text_width));
    let text = &text[..skip];
    let concat_width = (text_width - skipped_width) + ellipsis_width;
    assert!(concat_width <= max_width);
    (Cow::Owned([text, ellipsis].concat()), concat_width)
}

/// Shortens `text` to `max_width` by removing leading characters, returning
/// `(start_index, width)`.
///
/// The truncated string may have 0-width decomposed characters at start.
fn truncate_start_pos(text: &str, max_width: usize) -> (usize, usize) {
    truncate_start_pos_with_indices(
        text.char_indices()
            .rev()
            .map(|(start, c)| (start + c.len_utf8(), c)),
        max_width,
    )
}

fn truncate_start_pos_bytes(text: &[u8], max_width: usize) -> (usize, usize) {
    truncate_start_pos_with_indices(
        text.char_indices().rev().map(|(_, end, c)| (end, c)),
        max_width,
    )
}

fn truncate_start_pos_with_indices(
    char_indices_rev: impl Iterator<Item = (usize, char)>,
    max_width: usize,
) -> (usize, usize) {
    let mut acc_width = 0;
    for (end, c) in char_indices_rev {
        let new_width = acc_width + c.width().unwrap_or(0);
        if new_width > max_width {
            return (end, acc_width);
        }
        acc_width = new_width;
    }
    (0, acc_width)
}

/// Shortens `text` to `max_width` by removing trailing characters, returning
/// `(end_index, width)`.
fn truncate_end_pos(text: &str, max_width: usize) -> (usize, usize) {
    truncate_end_pos_with_indices(text.char_indices(), text.len(), max_width)
}

fn truncate_end_pos_bytes(text: &[u8], max_width: usize) -> (usize, usize) {
    truncate_end_pos_with_indices(
        text.char_indices().map(|(start, _, c)| (start, c)),
        text.len(),
        max_width,
    )
}

fn truncate_end_pos_with_indices(
    char_indices_fwd: impl Iterator<Item = (usize, char)>,
    text_len: usize,
    max_width: usize,
) -> (usize, usize) {
    let mut acc_width = 0;
    for (start, c) in char_indices_fwd {
        let new_width = acc_width + c.width().unwrap_or(0);
        if new_width > max_width {
            return (start, acc_width);
        }
        acc_width = new_width;
    }
    (text_len, acc_width)
}

/// Skips `width` leading characters, returning `(start_index, skipped_width)`.
///
/// The `skipped_width` may exceed the given `width` if `width` is not at
/// character boundary.
///
/// The truncated string may have 0-width decomposed characters at start.
fn skip_start_pos(text: &str, width: usize) -> (usize, usize) {
    skip_start_pos_with_indices(text.char_indices(), text.len(), width)
}

fn skip_start_pos_with_indices(
    char_indices_fwd: impl Iterator<Item = (usize, char)>,
    text_len: usize,
    width: usize,
) -> (usize, usize) {
    let mut acc_width = 0;
    for (start, c) in char_indices_fwd {
        if acc_width >= width {
            return (start, acc_width);
        }
        acc_width += c.width().unwrap_or(0);
    }
    (text_len, acc_width)
}

/// Skips `width` trailing characters, returning `(end_index, skipped_width)`.
///
/// The `skipped_width` may exceed the given `width` if `width` is not at
/// character boundary.
fn skip_end_pos(text: &str, width: usize) -> (usize, usize) {
    skip_end_pos_with_indices(
        text.char_indices()
            .rev()
            .map(|(start, c)| (start + c.len_utf8(), c)),
        width,
    )
}

fn skip_end_pos_with_indices(
    char_indices_rev: impl Iterator<Item = (usize, char)>,
    width: usize,
) -> (usize, usize) {
    let mut acc_width = 0;
    for (end, c) in char_indices_rev {
        if acc_width >= width {
            return (end, acc_width);
        }
        acc_width += c.width().unwrap_or(0);
    }
    (0, acc_width)
}

/// Removes leading 0-width characters.
fn trim_start_zero_width_chars(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.width().unwrap_or(0) == 0)
}

/// Returns bytes length of leading 0-width characters.
fn count_start_zero_width_chars_bytes(text: &[u8]) -> usize {
    text.char_indices()
        .find(|(_, _, c)| c.width().unwrap_or(0) != 0)
        .map(|(start, _, _)| start)
        .unwrap_or(text.len())
}

/// Writes text truncated to `max_width` by removing leading characters. Returns
/// width of the truncated text, which may be shorter than `max_width`.
///
/// The input `recorded_content` should be a single-line text.
pub fn write_truncated_start(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    max_width: usize,
) -> io::Result<usize> {
    let data = recorded_content.data();
    let (start, truncated_width) = truncate_start_pos_bytes(data, max_width);
    let truncated_start = start + count_start_zero_width_chars_bytes(&data[start..]);
    recorded_content.replay_with(formatter, |formatter, range| {
        let start = cmp::max(range.start, truncated_start);
        if start < range.end {
            formatter.write_all(&data[start..range.end])?;
        }
        Ok(())
    })?;
    Ok(truncated_width)
}

/// Writes text truncated to `max_width` by removing trailing characters.
/// Returns width of the truncated text, which may be shorter than `max_width`.
///
/// The input `recorded_content` should be a single-line text.
pub fn write_truncated_end(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    max_width: usize,
) -> io::Result<usize> {
    let data = recorded_content.data();
    let (truncated_end, truncated_width) = truncate_end_pos_bytes(data, max_width);
    recorded_content.replay_with(formatter, |formatter, range| {
        let end = cmp::min(range.end, truncated_end);
        if range.start < end {
            formatter.write_all(&data[range.start..end])?;
        }
        Ok(())
    })?;
    Ok(truncated_width)
}

/// Writes text padded to `min_width` by adding leading fill characters.
///
/// The input `recorded_content` should be a single-line text. The
/// `recorded_fill_char` should be bytes of 1-width character.
pub fn write_padded_start(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    recorded_fill_char: &FormatRecorder,
    min_width: usize,
) -> io::Result<()> {
    // We don't care about the width of non-UTF-8 bytes, but should not panic.
    let width = String::from_utf8_lossy(recorded_content.data()).width();
    let fill_width = min_width.saturating_sub(width);
    write_padding(formatter, recorded_fill_char, fill_width)?;
    recorded_content.replay(formatter)?;
    Ok(())
}

/// Writes text padded to `min_width` by adding leading fill characters.
///
/// The input `recorded_content` should be a single-line text. The
/// `recorded_fill_char` should be bytes of 1-width character.
pub fn write_padded_end(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    recorded_fill_char: &FormatRecorder,
    min_width: usize,
) -> io::Result<()> {
    // We don't care about the width of non-UTF-8 bytes, but should not panic.
    let width = String::from_utf8_lossy(recorded_content.data()).width();
    let fill_width = min_width.saturating_sub(width);
    recorded_content.replay(formatter)?;
    write_padding(formatter, recorded_fill_char, fill_width)?;
    Ok(())
}

fn write_padding(
    formatter: &mut dyn Formatter,
    recorded_fill_char: &FormatRecorder,
    fill_width: usize,
) -> io::Result<()> {
    if fill_width == 0 {
        return Ok(());
    }
    let data = recorded_fill_char.data();
    recorded_fill_char.replay_with(formatter, |formatter, range| {
        // Don't emit labels repeatedly, just repeat content. Suppose fill char
        // is a single character, the byte sequence shouldn't be broken up to
        // multiple labeled regions.
        for _ in 0..fill_width {
            formatter.write_all(&data[range.clone()])?;
        }
        Ok(())
    })
}

/// Indents each line by the given prefix preserving labels.
pub fn write_indented(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    mut write_prefix: impl FnMut(&mut dyn Formatter) -> io::Result<()>,
) -> io::Result<()> {
    let data = recorded_content.data();
    let mut new_line = true;
    recorded_content.replay_with(formatter, |formatter, range| {
        for line in data[range].split_inclusive(|&c| c == b'\n') {
            if new_line && line != b"\n" {
                // Prefix inherits the current labels. This is implementation detail
                // and may be fixed later.
                write_prefix(formatter)?;
            }
            formatter.write_all(line)?;
            new_line = line.ends_with(b"\n");
        }
        Ok(())
    })
}

/// Word with trailing whitespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteFragment<'a> {
    word: &'a [u8],
    whitespace_len: usize,
    word_width: usize,
}

impl<'a> ByteFragment<'a> {
    fn new(word: &'a [u8], whitespace_len: usize) -> Self {
        // We don't care about the width of non-UTF-8 bytes, but should not panic.
        let word_width = textwrap::core::display_width(&String::from_utf8_lossy(word));
        ByteFragment {
            word,
            whitespace_len,
            word_width,
        }
    }

    fn offset_in(&self, text: &[u8]) -> usize {
        byte_offset_from(text, self.word)
    }
}

impl textwrap::core::Fragment for ByteFragment<'_> {
    fn width(&self) -> f64 {
        self.word_width as f64
    }

    fn whitespace_width(&self) -> f64 {
        self.whitespace_len as f64
    }

    fn penalty_width(&self) -> f64 {
        0.0
    }
}

fn byte_offset_from(outer: &[u8], inner: &[u8]) -> usize {
    let outer_start = outer.as_ptr() as usize;
    let inner_start = inner.as_ptr() as usize;
    assert!(outer_start <= inner_start);
    assert!(inner_start + inner.len() <= outer_start + outer.len());
    inner_start - outer_start
}

fn split_byte_line_to_words(line: &[u8]) -> Vec<ByteFragment<'_>> {
    let mut words = Vec::new();
    let mut tail = line;
    while let Some(word_end) = tail.iter().position(|&c| c == b' ') {
        let word = &tail[..word_end];
        let ws_end = tail[word_end + 1..]
            .iter()
            .position(|&c| c != b' ')
            .map(|p| p + word_end + 1)
            .unwrap_or(tail.len());
        words.push(ByteFragment::new(word, ws_end - word_end));
        tail = &tail[ws_end..];
    }
    if !tail.is_empty() {
        words.push(ByteFragment::new(tail, 0));
    }
    words
}

/// Wraps lines at the given width, returns a vector of lines (excluding "\n".)
///
/// Existing newline characters will never be removed. For `str` content, you
/// can use `textwrap::refill()` to refill a pre-formatted text.
///
/// Each line is a sub-slice of the given text, even if the line is empty.
///
/// The wrapping logic is more restricted than the default of the `textwrap`.
/// Notably, this doesn't support hyphenation nor unicode line break. The
/// display width is calculated based on unicode property in the same manner
/// as `textwrap::wrap()`.
pub fn wrap_bytes(text: &[u8], width: usize) -> Vec<&[u8]> {
    let mut split_lines = Vec::new();
    for line in text.split(|&c| c == b'\n') {
        let words = split_byte_line_to_words(line);
        let split = textwrap::wrap_algorithms::wrap_first_fit(&words, &[width as f64]);
        split_lines.extend(split.iter().map(|words| match words {
            [] => &line[..0], // Empty line
            [a] => a.word,
            [a, .., b] => {
                let start = a.offset_in(line);
                let end = b.offset_in(line) + b.word.len();
                &line[start..end]
            }
        }));
    }
    split_lines
}

/// Wraps lines at the given width preserving labels.
///
/// `textwrap::wrap()` can also process text containing ANSI escape sequences.
/// The main difference is that this function will reset the style for each line
/// and recreate it on the following line if the output `formatter` is
/// a `ColorFormatter`.
pub fn write_wrapped(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    width: usize,
) -> io::Result<()> {
    let data = recorded_content.data();
    let mut line_ranges = wrap_bytes(data, width)
        .into_iter()
        .map(|line| {
            let start = byte_offset_from(data, line);
            start..start + line.len()
        })
        .peekable();
    // The recorded data ranges are contiguous, and the line ranges are increasing
    // sequence (with some holes.) Both ranges should start from data[0].
    recorded_content.replay_with(formatter, |formatter, data_range| {
        while let Some(line_range) = line_ranges.peek() {
            let start = cmp::max(data_range.start, line_range.start);
            let end = cmp::min(data_range.end, line_range.end);
            if start < end {
                formatter.write_all(&data[start..end])?;
            }
            if data_range.end <= line_range.end {
                break; // No more lines in this data range
            }
            line_ranges.next().unwrap();
            if line_ranges.peek().is_some() {
                writeln!(formatter)?; // Not the last line
            }
        }
        Ok(())
    })
}

pub fn parse_author(author: &str) -> Result<(String, String), &'static str> {
    let re = regex::Regex::new(r"(?<name>.*?)\s*<(?<email>.+)>$").unwrap();
    let captures = re.captures(author).ok_or("Invalid author string")?;
    Ok((captures["name"].to_string(), captures["email"].to_string()))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use indoc::indoc;
    use jj_lib::config::ConfigLayer;
    use jj_lib::config::ConfigSource;
    use jj_lib::config::StackedConfig;

    use super::*;
    use crate::formatter::ColorFormatter;
    use crate::formatter::PlainTextFormatter;

    fn format_colored(write: impl FnOnce(&mut dyn Formatter) -> io::Result<()>) -> String {
        let mut config = StackedConfig::empty();
        config.add_layer(
            ConfigLayer::parse(
                ConfigSource::Default,
                indoc! {"
                    colors.cyan = 'cyan'
                    colors.red = 'red'
                "},
            )
            .unwrap(),
        );
        let mut output = Vec::new();
        let mut formatter = ColorFormatter::for_config(&mut output, &config, false).unwrap();
        write(&mut formatter).unwrap();
        drop(formatter);
        String::from_utf8(output).unwrap()
    }

    fn format_plain_text(write: impl FnOnce(&mut dyn Formatter) -> io::Result<()>) -> String {
        let mut output = Vec::new();
        let mut formatter = PlainTextFormatter::new(&mut output);
        write(&mut formatter).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_elide_start() {
        // Empty string
        assert_eq!(elide_start("", "", 1), ("".into(), 0));

        // Basic truncation
        assert_eq!(elide_start("abcdef", "", 6), ("abcdef".into(), 6));
        assert_eq!(elide_start("abcdef", "", 5), ("bcdef".into(), 5));
        assert_eq!(elide_start("abcdef", "", 1), ("f".into(), 1));
        assert_eq!(elide_start("abcdef", "", 0), ("".into(), 0));
        assert_eq!(elide_start("abcdef", "-=~", 6), ("abcdef".into(), 6));
        assert_eq!(elide_start("abcdef", "-=~", 5), ("-=~ef".into(), 5));
        assert_eq!(elide_start("abcdef", "-=~", 4), ("-=~f".into(), 4));
        assert_eq!(elide_start("abcdef", "-=~", 3), ("-=~".into(), 3));
        assert_eq!(elide_start("abcdef", "-=~", 2), ("=~".into(), 2));
        assert_eq!(elide_start("abcdef", "-=~", 1), ("~".into(), 1));
        assert_eq!(elide_start("abcdef", "-=~", 0), ("".into(), 0));

        // East Asian characters (char.width() == 2)
        assert_eq!(elide_start("一二三", "", 6), ("一二三".into(), 6));
        assert_eq!(elide_start("一二三", "", 5), ("二三".into(), 4));
        assert_eq!(elide_start("一二三", "", 4), ("二三".into(), 4));
        assert_eq!(elide_start("一二三", "", 1), ("".into(), 0));
        assert_eq!(elide_start("一二三", "-=~", 6), ("一二三".into(), 6));
        assert_eq!(elide_start("一二三", "-=~", 5), ("-=~三".into(), 5));
        assert_eq!(elide_start("一二三", "-=~", 4), ("-=~".into(), 3));
        assert_eq!(elide_start("一二三", "略", 6), ("一二三".into(), 6));
        assert_eq!(elide_start("一二三", "略", 5), ("略三".into(), 4));
        assert_eq!(elide_start("一二三", "略", 4), ("略三".into(), 4));
        assert_eq!(elide_start("一二三", "略", 2), ("略".into(), 2));
        assert_eq!(elide_start("一二三", "略", 1), ("".into(), 0));
        assert_eq!(elide_start("一二三", ".", 5), (".二三".into(), 5));
        assert_eq!(elide_start("一二三", ".", 4), (".三".into(), 3));
        assert_eq!(elide_start("一二三", "略.", 5), ("略.三".into(), 5));
        assert_eq!(elide_start("一二三", "略.", 4), ("略.".into(), 3));

        // Multi-byte character at boundary
        assert_eq!(elide_start("àbcdè", "", 5), ("àbcdè".into(), 5));
        assert_eq!(elide_start("àbcdè", "", 4), ("bcdè".into(), 4));
        assert_eq!(elide_start("àbcdè", "", 1), ("è".into(), 1));
        assert_eq!(elide_start("àbcdè", "", 0), ("".into(), 0));
        assert_eq!(elide_start("àbcdè", "ÀÇÈ", 4), ("ÀÇÈè".into(), 4));
        assert_eq!(elide_start("àbcdè", "ÀÇÈ", 3), ("ÀÇÈ".into(), 3));
        assert_eq!(elide_start("àbcdè", "ÀÇÈ", 2), ("ÇÈ".into(), 2));

        // Decomposed character at boundary
        assert_eq!(
            elide_start("a\u{300}bcde\u{300}", "", 5),
            ("a\u{300}bcde\u{300}".into(), 5)
        );
        assert_eq!(
            elide_start("a\u{300}bcde\u{300}", "", 4),
            ("bcde\u{300}".into(), 4)
        );
        assert_eq!(
            elide_start("a\u{300}bcde\u{300}", "", 1),
            ("e\u{300}".into(), 1)
        );
        assert_eq!(elide_start("a\u{300}bcde\u{300}", "", 0), ("".into(), 0));
        assert_eq!(
            elide_start("a\u{300}bcde\u{300}", "A\u{300}CE\u{300}", 4),
            ("A\u{300}CE\u{300}e\u{300}".into(), 4)
        );
        assert_eq!(
            elide_start("a\u{300}bcde\u{300}", "A\u{300}CE\u{300}", 3),
            ("A\u{300}CE\u{300}".into(), 3)
        );
        assert_eq!(
            elide_start("a\u{300}bcde\u{300}", "A\u{300}CE\u{300}", 2),
            ("CE\u{300}".into(), 2)
        );
    }

    #[test]
    fn test_elide_end() {
        // Empty string
        assert_eq!(elide_end("", "", 1), ("".into(), 0));

        // Basic truncation
        assert_eq!(elide_end("abcdef", "", 6), ("abcdef".into(), 6));
        assert_eq!(elide_end("abcdef", "", 5), ("abcde".into(), 5));
        assert_eq!(elide_end("abcdef", "", 1), ("a".into(), 1));
        assert_eq!(elide_end("abcdef", "", 0), ("".into(), 0));
        assert_eq!(elide_end("abcdef", "-=~", 6), ("abcdef".into(), 6));
        assert_eq!(elide_end("abcdef", "-=~", 5), ("ab-=~".into(), 5));
        assert_eq!(elide_end("abcdef", "-=~", 4), ("a-=~".into(), 4));
        assert_eq!(elide_end("abcdef", "-=~", 3), ("-=~".into(), 3));
        assert_eq!(elide_end("abcdef", "-=~", 2), ("-=".into(), 2));
        assert_eq!(elide_end("abcdef", "-=~", 1), ("-".into(), 1));
        assert_eq!(elide_end("abcdef", "-=~", 0), ("".into(), 0));

        // East Asian characters (char.width() == 2)
        assert_eq!(elide_end("一二三", "", 6), ("一二三".into(), 6));
        assert_eq!(elide_end("一二三", "", 5), ("一二".into(), 4));
        assert_eq!(elide_end("一二三", "", 4), ("一二".into(), 4));
        assert_eq!(elide_end("一二三", "", 1), ("".into(), 0));
        assert_eq!(elide_end("一二三", "-=~", 6), ("一二三".into(), 6));
        assert_eq!(elide_end("一二三", "-=~", 5), ("一-=~".into(), 5));
        assert_eq!(elide_end("一二三", "-=~", 4), ("-=~".into(), 3));
        assert_eq!(elide_end("一二三", "略", 6), ("一二三".into(), 6));
        assert_eq!(elide_end("一二三", "略", 5), ("一略".into(), 4));
        assert_eq!(elide_end("一二三", "略", 4), ("一略".into(), 4));
        assert_eq!(elide_end("一二三", "略", 2), ("略".into(), 2));
        assert_eq!(elide_end("一二三", "略", 1), ("".into(), 0));
        assert_eq!(elide_end("一二三", ".", 5), ("一二.".into(), 5));
        assert_eq!(elide_end("一二三", ".", 4), ("一.".into(), 3));
        assert_eq!(elide_end("一二三", "略.", 5), ("一略.".into(), 5));
        assert_eq!(elide_end("一二三", "略.", 4), ("略.".into(), 3));

        // Multi-byte character at boundary
        assert_eq!(elide_end("àbcdè", "", 5), ("àbcdè".into(), 5));
        assert_eq!(elide_end("àbcdè", "", 4), ("àbcd".into(), 4));
        assert_eq!(elide_end("àbcdè", "", 1), ("à".into(), 1));
        assert_eq!(elide_end("àbcdè", "", 0), ("".into(), 0));
        assert_eq!(elide_end("àbcdè", "ÀÇÈ", 4), ("àÀÇÈ".into(), 4));
        assert_eq!(elide_end("àbcdè", "ÀÇÈ", 3), ("ÀÇÈ".into(), 3));
        assert_eq!(elide_end("àbcdè", "ÀÇÈ", 2), ("ÀÇ".into(), 2));

        // Decomposed character at boundary
        assert_eq!(
            elide_end("a\u{300}bcde\u{300}", "", 5),
            ("a\u{300}bcde\u{300}".into(), 5)
        );
        assert_eq!(
            elide_end("a\u{300}bcde\u{300}", "", 4),
            ("a\u{300}bcd".into(), 4)
        );
        assert_eq!(
            elide_end("a\u{300}bcde\u{300}", "", 1),
            ("a\u{300}".into(), 1)
        );
        assert_eq!(elide_end("a\u{300}bcde\u{300}", "", 0), ("".into(), 0));
        assert_eq!(
            elide_end("a\u{300}bcde\u{300}", "A\u{300}CE\u{300}", 4),
            ("a\u{300}A\u{300}CE\u{300}".into(), 4)
        );
        assert_eq!(
            elide_end("a\u{300}bcde\u{300}", "A\u{300}CE\u{300}", 3),
            ("A\u{300}CE\u{300}".into(), 3)
        );
        assert_eq!(
            elide_end("a\u{300}bcde\u{300}", "A\u{300}CE\u{300}", 2),
            ("A\u{300}C".into(), 2)
        );
    }

    #[test]
    fn test_write_truncated_labeled() {
        let mut recorder = FormatRecorder::new();
        for (label, word) in [("red", "foo"), ("cyan", "bar")] {
            recorder.push_label(label).unwrap();
            write!(recorder, "{word}").unwrap();
            recorder.pop_label().unwrap();
        }

        // Truncate start
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 6).map(|_| ())),
            @"[38;5;1mfoo[39m[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 5).map(|_| ())),
            @"[38;5;1moo[39m[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 3).map(|_| ())),
            @"[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 2).map(|_| ())),
            @"[38;5;6mar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 0).map(|_| ())),
            @""
        );

        // Truncate end
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 6).map(|_| ())),
            @"[38;5;1mfoo[39m[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 5).map(|_| ())),
            @"[38;5;1mfoo[39m[38;5;6mba[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 3).map(|_| ())),
            @"[38;5;1mfoo[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 2).map(|_| ())),
            @"[38;5;1mfo[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 0).map(|_| ())),
            @""
        );
    }

    #[test]
    fn test_write_truncated_non_ascii_chars() {
        let mut recorder = FormatRecorder::new();
        write!(recorder, "a\u{300}bc\u{300}一二三").unwrap();

        // Truncate start
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 1).map(|_| ())),
            @""
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 2).map(|_| ())),
            @"三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 3).map(|_| ())),
            @"三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 6).map(|_| ())),
            @"一二三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 7).map(|_| ())),
            @"c̀一二三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 9).map(|_| ())),
            @"àbc̀一二三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 10).map(|_| ())),
            @"àbc̀一二三"
        );

        // Truncate end
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 1).map(|_| ())),
            @"à"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 4).map(|_| ())),
            @"àbc̀"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 5).map(|_| ())),
            @"àbc̀一"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 9).map(|_| ())),
            @"àbc̀一二三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 10).map(|_| ())),
            @"àbc̀一二三"
        );
    }

    #[test]
    fn test_write_truncated_empty_content() {
        let recorder = FormatRecorder::new();

        // Truncate start
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 0).map(|_| ())),
            @""
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_start(formatter, &recorder, 1).map(|_| ())),
            @""
        );

        // Truncate end
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 0).map(|_| ())),
            @""
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_truncated_end(formatter, &recorder, 1).map(|_| ())),
            @""
        );
    }

    #[test]
    fn test_write_padded_labeled_content() {
        let mut recorder = FormatRecorder::new();
        for (label, word) in [("red", "foo"), ("cyan", "bar")] {
            recorder.push_label(label).unwrap();
            write!(recorder, "{word}").unwrap();
            recorder.pop_label().unwrap();
        }
        let fill = FormatRecorder::with_data("=");

        // Pad start
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 6)),
            @"[38;5;1mfoo[39m[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 7)),
            @"=[38;5;1mfoo[39m[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 8)),
            @"==[38;5;1mfoo[39m[38;5;6mbar[39m"
        );

        // Pad end
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 6)),
            @"[38;5;1mfoo[39m[38;5;6mbar[39m"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 7)),
            @"[38;5;1mfoo[39m[38;5;6mbar[39m="
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 8)),
            @"[38;5;1mfoo[39m[38;5;6mbar[39m=="
        );
    }

    #[test]
    fn test_write_padded_labeled_fill_char() {
        let recorder = FormatRecorder::with_data("foo");
        let mut fill = FormatRecorder::new();
        fill.push_label("red").unwrap();
        write!(fill, "=").unwrap();
        fill.pop_label().unwrap();

        // Pad start
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 5)),
            @"[38;5;1m==[39mfoo"
        );

        // Pad end
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 6)),
            @"foo[38;5;1m===[39m"
        );
    }

    #[test]
    fn test_write_padded_non_ascii_chars() {
        let recorder = FormatRecorder::with_data("a\u{300}bc\u{300}一二三");
        let fill = FormatRecorder::with_data("=");

        // Pad start
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 9)),
            @"àbc̀一二三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 10)),
            @"=àbc̀一二三"
        );

        // Pad end
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 9)),
            @"àbc̀一二三"
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 10)),
            @"àbc̀一二三="
        );
    }

    #[test]
    fn test_write_padded_empty_content() {
        let recorder = FormatRecorder::new();
        let fill = FormatRecorder::with_data("=");

        // Pad start
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 0)),
            @""
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_start(formatter, &recorder, &fill, 1)),
            @"="
        );

        // Pad end
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 0)),
            @""
        );
        insta::assert_snapshot!(
            format_colored(|formatter| write_padded_end(formatter, &recorder, &fill, 1)),
            @"="
        );
    }

    #[test]
    fn test_split_byte_line_to_words() {
        assert_eq!(split_byte_line_to_words(b""), vec![]);
        assert_eq!(
            split_byte_line_to_words(b"foo"),
            vec![ByteFragment {
                word: b"foo",
                whitespace_len: 0,
                word_width: 3
            }],
        );
        assert_eq!(
            split_byte_line_to_words(b"  foo"),
            vec![
                ByteFragment {
                    word: b"",
                    whitespace_len: 2,
                    word_width: 0
                },
                ByteFragment {
                    word: b"foo",
                    whitespace_len: 0,
                    word_width: 3
                },
            ],
        );
        assert_eq!(
            split_byte_line_to_words(b"foo  "),
            vec![ByteFragment {
                word: b"foo",
                whitespace_len: 2,
                word_width: 3
            }],
        );
        assert_eq!(
            split_byte_line_to_words(b"a b  foo bar "),
            vec![
                ByteFragment {
                    word: b"a",
                    whitespace_len: 1,
                    word_width: 1
                },
                ByteFragment {
                    word: b"b",
                    whitespace_len: 2,
                    word_width: 1
                },
                ByteFragment {
                    word: b"foo",
                    whitespace_len: 1,
                    word_width: 3,
                },
                ByteFragment {
                    word: b"bar",
                    whitespace_len: 1,
                    word_width: 3,
                },
            ],
        );
    }

    #[test]
    fn test_wrap_bytes() {
        assert_eq!(wrap_bytes(b"foo", 10), [b"foo".as_ref()]);
        assert_eq!(wrap_bytes(b"foo bar", 10), [b"foo bar".as_ref()]);
        assert_eq!(
            wrap_bytes(b"foo bar baz", 10),
            [b"foo bar".as_ref(), b"baz".as_ref()],
        );

        // Empty text is represented as [""]
        assert_eq!(wrap_bytes(b"", 10), [b"".as_ref()]);
        assert_eq!(wrap_bytes(b" ", 10), [b"".as_ref()]);

        // Whitespace in the middle should be preserved
        assert_eq!(
            wrap_bytes(b"foo  bar   baz", 8),
            [b"foo  bar".as_ref(), b"baz".as_ref()],
        );
        assert_eq!(
            wrap_bytes(b"foo  bar   x", 7),
            [b"foo".as_ref(), b"bar   x".as_ref()],
        );
        assert_eq!(
            wrap_bytes(b"foo bar \nx", 7),
            [b"foo bar".as_ref(), b"x".as_ref()],
        );
        assert_eq!(
            wrap_bytes(b"foo bar\n x", 7),
            [b"foo bar".as_ref(), b" x".as_ref()],
        );
        assert_eq!(
            wrap_bytes(b"foo bar x", 4),
            [b"foo".as_ref(), b"bar".as_ref(), b"x".as_ref()],
        );

        // Ends with "\n"
        assert_eq!(wrap_bytes(b"foo\n", 10), [b"foo".as_ref(), b"".as_ref()]);
        assert_eq!(wrap_bytes(b"foo\n", 3), [b"foo".as_ref(), b"".as_ref()]);
        assert_eq!(wrap_bytes(b"\n", 10), [b"".as_ref(), b"".as_ref()]);

        // Overflow
        assert_eq!(wrap_bytes(b"foo x", 2), [b"foo".as_ref(), b"x".as_ref()]);
        assert_eq!(wrap_bytes(b"x y", 0), [b"x".as_ref(), b"y".as_ref()]);

        // Invalid UTF-8 bytes should not cause panic
        assert_eq!(wrap_bytes(b"foo\x80", 10), [b"foo\x80".as_ref()]);
    }

    #[test]
    fn test_wrap_bytes_slice_ptr() {
        let text = b"\nfoo\n\nbar baz\n";
        let lines = wrap_bytes(text, 10);
        assert_eq!(
            lines,
            [
                b"".as_ref(),
                b"foo".as_ref(),
                b"".as_ref(),
                b"bar baz".as_ref(),
                b"".as_ref()
            ],
        );
        // Each line should be a sub-slice of the source text
        assert_eq!(lines[0].as_ptr(), text[0..].as_ptr());
        assert_eq!(lines[1].as_ptr(), text[1..].as_ptr());
        assert_eq!(lines[2].as_ptr(), text[5..].as_ptr());
        assert_eq!(lines[3].as_ptr(), text[6..].as_ptr());
        assert_eq!(lines[4].as_ptr(), text[14..].as_ptr());
    }

    #[test]
    fn test_write_wrapped() {
        // Split single label chunk
        let mut recorder = FormatRecorder::new();
        recorder.push_label("red").unwrap();
        write!(recorder, "foo bar baz\nqux quux\n").unwrap();
        recorder.pop_label().unwrap();
        insta::assert_snapshot!(
            format_colored(|formatter| write_wrapped(formatter, &recorder, 7)),
            @r###"
        [38;5;1mfoo bar[39m
        [38;5;1mbaz[39m
        [38;5;1mqux[39m
        [38;5;1mquux[39m
        "###
        );

        // Multiple label chunks in a line
        let mut recorder = FormatRecorder::new();
        for (i, word) in ["foo ", "bar ", "baz\n", "qux ", "quux"].iter().enumerate() {
            recorder.push_label(["red", "cyan"][i & 1]).unwrap();
            write!(recorder, "{word}").unwrap();
            recorder.pop_label().unwrap();
        }
        insta::assert_snapshot!(
            format_colored(|formatter| write_wrapped(formatter, &recorder, 7)),
            @r###"
        [38;5;1mfoo [39m[38;5;6mbar[39m
        [38;5;1mbaz[39m
        [38;5;6mqux[39m
        [38;5;1mquux[39m
        "###
        );

        // Empty lines should not cause panic
        let mut recorder = FormatRecorder::new();
        for (i, word) in ["", "foo", "", "bar baz", ""].iter().enumerate() {
            recorder.push_label(["red", "cyan"][i & 1]).unwrap();
            writeln!(recorder, "{word}").unwrap();
            recorder.pop_label().unwrap();
        }
        insta::assert_snapshot!(
            format_colored(|formatter| write_wrapped(formatter, &recorder, 10)),
            @r###"
        [38;5;1m[39m
        [38;5;6mfoo[39m
        [38;5;1m[39m
        [38;5;6mbar baz[39m
        [38;5;1m[39m
        "###
        );

        // Split at label boundary
        let mut recorder = FormatRecorder::new();
        recorder.push_label("red").unwrap();
        write!(recorder, "foo bar").unwrap();
        recorder.pop_label().unwrap();
        write!(recorder, " ").unwrap();
        recorder.push_label("cyan").unwrap();
        writeln!(recorder, "baz").unwrap();
        recorder.pop_label().unwrap();
        insta::assert_snapshot!(
            format_colored(|formatter| write_wrapped(formatter, &recorder, 10)),
            @r###"
        [38;5;1mfoo bar[39m
        [38;5;6mbaz[39m
        "###
        );

        // Do not split at label boundary "ba|z" (since it's a single word)
        let mut recorder = FormatRecorder::new();
        recorder.push_label("red").unwrap();
        write!(recorder, "foo bar ba").unwrap();
        recorder.pop_label().unwrap();
        recorder.push_label("cyan").unwrap();
        writeln!(recorder, "z").unwrap();
        recorder.pop_label().unwrap();
        insta::assert_snapshot!(
            format_colored(|formatter| write_wrapped(formatter, &recorder, 10)),
            @r###"
        [38;5;1mfoo bar[39m
        [38;5;1mba[39m[38;5;6mz[39m
        "###
        );
    }

    #[test]
    fn test_write_wrapped_leading_labeled_whitespace() {
        let mut recorder = FormatRecorder::new();
        recorder.push_label("red").unwrap();
        write!(recorder, " ").unwrap();
        recorder.pop_label().unwrap();
        write!(recorder, "foo").unwrap();
        insta::assert_snapshot!(
            format_colored(|formatter| write_wrapped(formatter, &recorder, 10)),
            @"[38;5;1m [39mfoo"
        );
    }

    #[test]
    fn test_write_wrapped_trailing_labeled_whitespace() {
        // data: "foo" " "
        // line:  ---
        let mut recorder = FormatRecorder::new();
        write!(recorder, "foo").unwrap();
        recorder.push_label("red").unwrap();
        write!(recorder, " ").unwrap();
        recorder.pop_label().unwrap();
        assert_eq!(
            format_plain_text(|formatter| write_wrapped(formatter, &recorder, 10)),
            "foo",
        );

        // data: "foo" "\n"
        // line:  ---     -
        let mut recorder = FormatRecorder::new();
        write!(recorder, "foo").unwrap();
        recorder.push_label("red").unwrap();
        writeln!(recorder).unwrap();
        recorder.pop_label().unwrap();
        assert_eq!(
            format_plain_text(|formatter| write_wrapped(formatter, &recorder, 10)),
            "foo\n",
        );

        // data: "foo\n" " "
        // line:  ---    -
        let mut recorder = FormatRecorder::new();
        writeln!(recorder, "foo").unwrap();
        recorder.push_label("red").unwrap();
        write!(recorder, " ").unwrap();
        recorder.pop_label().unwrap();
        assert_eq!(
            format_plain_text(|formatter| write_wrapped(formatter, &recorder, 10)),
            "foo\n",
        );
    }

    #[test]
    fn test_parse_author() {
        let expected_name = "Example";
        let expected_email = "example@example.com";
        let parsed = parse_author(&format!("{expected_name} <{expected_email}>")).unwrap();
        assert_eq!(
            (expected_name.to_string(), expected_email.to_string()),
            parsed
        );
    }

    #[test]
    fn test_parse_author_with_utf8() {
        let expected_name = "Ąćęłńóśżź";
        let expected_email = "example@example.com";
        let parsed = parse_author(&format!("{expected_name} <{expected_email}>")).unwrap();
        assert_eq!(
            (expected_name.to_string(), expected_email.to_string()),
            parsed
        );
    }

    #[test]
    fn test_parse_author_without_name() {
        let expected_email = "example@example.com";
        let parsed = parse_author(&format!("<{expected_email}>")).unwrap();
        assert_eq!(("".to_string(), expected_email.to_string()), parsed);
    }
}
