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

use std::{cmp, io};

use crate::formatter::{FormatRecorder, Formatter};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatter::{ColorFormatter, PlainTextFormatter};

    fn format_colored(write: impl FnOnce(&mut dyn Formatter) -> io::Result<()>) -> String {
        let config = config::Config::builder()
            .set_override("colors.cyan", "cyan")
            .unwrap()
            .set_override("colors.red", "red")
            .unwrap()
            .build()
            .unwrap();
        let mut output = Vec::new();
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        write(&mut formatter).unwrap();
        String::from_utf8(output).unwrap()
    }

    fn format_plain_text(write: impl FnOnce(&mut dyn Formatter) -> io::Result<()>) -> String {
        let mut output = Vec::new();
        let mut formatter = PlainTextFormatter::new(&mut output);
        write(&mut formatter).unwrap();
        String::from_utf8(output).unwrap()
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
        recorder.write_str("foo bar baz\nqux quux\n").unwrap();
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
            recorder.write_str(word).unwrap();
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
            recorder.write_str(word).unwrap();
            recorder.write_str("\n").unwrap();
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
        recorder.write_str("foo bar").unwrap();
        recorder.pop_label().unwrap();
        recorder.write_str(" ").unwrap();
        recorder.push_label("cyan").unwrap();
        recorder.write_str("baz\n").unwrap();
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
        recorder.write_str("foo bar ba").unwrap();
        recorder.pop_label().unwrap();
        recorder.push_label("cyan").unwrap();
        recorder.write_str("z\n").unwrap();
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
        recorder.write_str(" ").unwrap();
        recorder.pop_label().unwrap();
        recorder.write_str("foo").unwrap();
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
        recorder.write_str("foo").unwrap();
        recorder.push_label("red").unwrap();
        recorder.write_str(" ").unwrap();
        recorder.pop_label().unwrap();
        assert_eq!(
            format_plain_text(|formatter| write_wrapped(formatter, &recorder, 10)),
            "foo",
        );

        // data: "foo" "\n"
        // line:  ---     -
        let mut recorder = FormatRecorder::new();
        recorder.write_str("foo").unwrap();
        recorder.push_label("red").unwrap();
        recorder.write_str("\n").unwrap();
        recorder.pop_label().unwrap();
        assert_eq!(
            format_plain_text(|formatter| write_wrapped(formatter, &recorder, 10)),
            "foo\n",
        );

        // data: "foo\n" " "
        // line:  ---    -
        let mut recorder = FormatRecorder::new();
        recorder.write_str("foo\n").unwrap();
        recorder.push_label("red").unwrap();
        recorder.write_str(" ").unwrap();
        recorder.pop_label().unwrap();
        assert_eq!(
            format_plain_text(|formatter| write_wrapped(formatter, &recorder, 10)),
            "foo\n",
        );
    }
}
