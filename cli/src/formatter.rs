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

use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::io::{Error, Write};
use std::ops::Range;
use std::sync::Arc;
use std::{fmt, io, mem};

use crossterm::queue;
use crossterm::style::{Attribute, Color, SetAttribute, SetBackgroundColor, SetForegroundColor};
use itertools::Itertools;

// Lets the caller label strings and translates the labels to colors
pub trait Formatter: Write {
    /// Returns the backing `Write`. This is useful for writing data that is
    /// already formatted, such as in the graphical log.
    fn raw(&mut self) -> &mut dyn Write;

    fn push_label(&mut self, label: &str) -> io::Result<()>;

    fn pop_label(&mut self) -> io::Result<()>;
}

impl dyn Formatter + '_ {
    pub fn labeled<S: AsRef<str>>(&mut self, label: S) -> LabeledWriter<&mut Self, S> {
        LabeledWriter {
            formatter: self,
            label,
        }
    }

    pub fn with_label(
        &mut self,
        label: &str,
        write_inner: impl FnOnce(&mut dyn Formatter) -> io::Result<()>,
    ) -> io::Result<()> {
        self.push_label(label)?;
        // Call `pop_label()` whether or not `write_inner()` fails, but don't let
        // its error replace the one from `write_inner()`.
        write_inner(self).and(self.pop_label())
    }
}

/// `Formatter` wrapper to write a labeled message with `write!()` or
/// `writeln!()`.
pub struct LabeledWriter<T, S> {
    formatter: T,
    label: S,
}

impl<T, S> LabeledWriter<T, S> {
    pub fn new(formatter: T, label: S) -> Self {
        LabeledWriter { formatter, label }
    }
}

impl<'a, T, S> LabeledWriter<T, S>
where
    T: BorrowMut<dyn Formatter + 'a>,
    S: AsRef<str>,
{
    pub fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        self.with_labeled(|formatter| formatter.write_fmt(args))
    }

    fn with_labeled(
        &mut self,
        write_inner: impl FnOnce(&mut dyn Formatter) -> io::Result<()>,
    ) -> io::Result<()> {
        self.formatter
            .borrow_mut()
            .with_label(self.label.as_ref(), write_inner)
    }
}

/// Like `LabeledWriter`, but also prints the `heading` once.
///
/// The `heading` will be printed within the first `write!()` or `writeln!()`
/// invocation, which is handy because `io::Error` can be handled there.
pub struct HeadingLabeledWriter<T, S, H> {
    writer: LabeledWriter<T, S>,
    heading: Option<H>,
}

impl<T, S, H> HeadingLabeledWriter<T, S, H> {
    pub fn new(formatter: T, label: S, heading: H) -> Self {
        HeadingLabeledWriter {
            writer: LabeledWriter::new(formatter, label),
            heading: Some(heading),
        }
    }
}

impl<'a, T, S, H> HeadingLabeledWriter<T, S, H>
where
    T: BorrowMut<dyn Formatter + 'a>,
    S: AsRef<str>,
    H: fmt::Display,
{
    pub fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        self.writer.with_labeled(|formatter| {
            if let Some(heading) = self.heading.take() {
                write!(formatter.labeled("heading"), "{heading}")?;
            }
            formatter.write_fmt(args)
        })
    }
}

type Rules = Vec<(Vec<String>, Style)>;

/// Creates `Formatter` instances with preconfigured parameters.
#[derive(Clone, Debug)]
pub struct FormatterFactory {
    kind: FormatterFactoryKind,
}

#[derive(Clone, Debug)]
enum FormatterFactoryKind {
    PlainText,
    Sanitized,
    Color { rules: Arc<Rules> },
}

impl FormatterFactory {
    pub fn prepare(
        config: &config::Config,
        color: bool,
        sanitized: bool,
    ) -> Result<Self, config::ConfigError> {
        let kind = if color {
            let rules = Arc::new(rules_from_config(config)?);
            FormatterFactoryKind::Color { rules }
        } else if sanitized {
            FormatterFactoryKind::Sanitized
        } else {
            FormatterFactoryKind::PlainText
        };
        Ok(FormatterFactory { kind })
    }

    pub fn new_formatter<'output, W: Write + 'output>(
        &self,
        output: W,
    ) -> Box<dyn Formatter + 'output> {
        match &self.kind {
            FormatterFactoryKind::PlainText => Box::new(PlainTextFormatter::new(output)),
            FormatterFactoryKind::Sanitized => Box::new(SanitizingFormatter::new(output)),
            FormatterFactoryKind::Color { rules } => {
                Box::new(ColorFormatter::new(output, rules.clone()))
            }
        }
    }
}

pub struct PlainTextFormatter<W> {
    output: W,
}

impl<W> PlainTextFormatter<W> {
    pub fn new(output: W) -> PlainTextFormatter<W> {
        Self { output }
    }
}

impl<W: Write> Write for PlainTextFormatter<W> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        self.output.write(data)
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.output.flush()
    }
}

impl<W: Write> Formatter for PlainTextFormatter<W> {
    fn raw(&mut self) -> &mut dyn Write {
        &mut self.output
    }

    fn push_label(&mut self, _label: &str) -> io::Result<()> {
        Ok(())
    }

    fn pop_label(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct SanitizingFormatter<W> {
    output: W,
}

impl<W> SanitizingFormatter<W> {
    pub fn new(output: W) -> SanitizingFormatter<W> {
        Self { output }
    }
}

impl<W: Write> Write for SanitizingFormatter<W> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        write_sanitized(&mut self.output, data)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.output.flush()
    }
}

impl<W: Write> Formatter for SanitizingFormatter<W> {
    fn raw(&mut self) -> &mut dyn Write {
        &mut self.output
    }

    fn push_label(&mut self, _label: &str) -> io::Result<()> {
        Ok(())
    }

    fn pop_label(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg_color: Option<Color>,
    pub bg_color: Option<Color>,
    pub bold: Option<bool>,
    pub underlined: Option<bool>,
}

impl Style {
    fn merge(&mut self, other: &Style) {
        self.fg_color = other.fg_color.or(self.fg_color);
        self.bg_color = other.bg_color.or(self.bg_color);
        self.bold = other.bold.or(self.bold);
        self.underlined = other.underlined.or(self.underlined);
    }
}

#[derive(Clone, Debug)]
pub struct ColorFormatter<W: Write> {
    output: W,
    rules: Arc<Rules>,
    /// The stack of currently applied labels. These determine the desired
    /// style.
    labels: Vec<String>,
    cached_styles: HashMap<Vec<String>, Style>,
    /// The style we last wrote to the output.
    current_style: Style,
}

impl<W: Write> ColorFormatter<W> {
    pub fn new(output: W, rules: Arc<Rules>) -> ColorFormatter<W> {
        ColorFormatter {
            output,
            rules,
            labels: vec![],
            cached_styles: HashMap::new(),
            current_style: Style::default(),
        }
    }

    pub fn for_config(output: W, config: &config::Config) -> Result<Self, config::ConfigError> {
        let rules = rules_from_config(config)?;
        Ok(Self::new(output, Arc::new(rules)))
    }

    fn requested_style(&mut self) -> Style {
        if let Some(cached) = self.cached_styles.get(&self.labels) {
            cached.clone()
        } else {
            // We use the reverse list of matched indices as a measure of how well the rule
            // matches the actual labels. For example, for rule "a d" and the actual labels
            // "a b c d", we'll get [3,0]. We compare them by Rust's default Vec comparison.
            // That means "a d" will trump both rule "d" (priority [3]) and rule
            // "a b c" (priority [2,1,0]).
            let mut matched_styles = vec![];
            for (labels, style) in self.rules.as_ref() {
                let mut labels_iter = self.labels.iter().enumerate();
                // The indexes in the current label stack that match the required label.
                let mut matched_indices = vec![];
                for required_label in labels {
                    for (label_index, label) in &mut labels_iter {
                        if label == required_label {
                            matched_indices.push(label_index);
                            break;
                        }
                    }
                }
                if matched_indices.len() == labels.len() {
                    matched_indices.reverse();
                    matched_styles.push((style, matched_indices));
                }
            }
            matched_styles.sort_by_key(|(_, indices)| indices.clone());

            let mut style = Style::default();
            for (matched_style, _) in matched_styles {
                style.merge(matched_style);
            }
            self.cached_styles
                .insert(self.labels.clone(), style.clone());
            style
        }
    }

    fn write_new_style(&mut self) -> io::Result<()> {
        let new_style = self.requested_style();
        if new_style != self.current_style {
            if new_style.bold != self.current_style.bold {
                if new_style.bold.unwrap_or_default() {
                    queue!(self.output, SetAttribute(Attribute::Bold))?;
                } else {
                    // NoBold results in double underlining on some terminals, so we use reset
                    // instead. However, that resets other attributes as well, so we reset
                    // our record of the current style so we re-apply the other attributes
                    // below.
                    queue!(self.output, SetAttribute(Attribute::Reset))?;
                    self.current_style = Style::default();
                }
            }
            if new_style.underlined != self.current_style.underlined {
                if new_style.underlined.unwrap_or_default() {
                    queue!(self.output, SetAttribute(Attribute::Underlined))?;
                } else {
                    queue!(self.output, SetAttribute(Attribute::NoUnderline))?;
                }
            }
            if new_style.fg_color != self.current_style.fg_color {
                queue!(
                    self.output,
                    SetForegroundColor(new_style.fg_color.unwrap_or(Color::Reset))
                )?;
            }
            if new_style.bg_color != self.current_style.bg_color {
                queue!(
                    self.output,
                    SetBackgroundColor(new_style.bg_color.unwrap_or(Color::Reset))
                )?;
            }
            self.current_style = new_style;
        }
        Ok(())
    }
}

fn rules_from_config(config: &config::Config) -> Result<Rules, config::ConfigError> {
    let mut result = vec![];
    let table = config.get_table("colors")?;
    for (key, value) in table {
        let labels = key
            .split_whitespace()
            .map(ToString::to_string)
            .collect_vec();
        match value.kind {
            config::ValueKind::String(color_name) => {
                let style = Style {
                    fg_color: Some(color_for_name_or_hex(&color_name)?),
                    bg_color: None,
                    bold: None,
                    underlined: None,
                };
                result.push((labels, style));
            }
            config::ValueKind::Table(style_table) => {
                let mut style = Style::default();
                if let Some(value) = style_table.get("fg") {
                    if let config::ValueKind::String(color_name) = &value.kind {
                        style.fg_color = Some(color_for_name_or_hex(color_name)?);
                    }
                }
                if let Some(value) = style_table.get("bg") {
                    if let config::ValueKind::String(color_name) = &value.kind {
                        style.bg_color = Some(color_for_name_or_hex(color_name)?);
                    }
                }
                if let Some(value) = style_table.get("bold") {
                    if let config::ValueKind::Boolean(value) = &value.kind {
                        style.bold = Some(*value);
                    }
                }
                if let Some(value) = style_table.get("underline") {
                    if let config::ValueKind::Boolean(value) = &value.kind {
                        style.underlined = Some(*value);
                    }
                }
                result.push((labels, style));
            }
            _ => {}
        }
    }
    Ok(result)
}

fn color_for_name_or_hex(name_or_hex: &str) -> Result<Color, config::ConfigError> {
    match name_or_hex {
        "default" => Ok(Color::Reset),
        "black" => Ok(Color::Black),
        "red" => Ok(Color::DarkRed),
        "green" => Ok(Color::DarkGreen),
        "yellow" => Ok(Color::DarkYellow),
        "blue" => Ok(Color::DarkBlue),
        "magenta" => Ok(Color::DarkMagenta),
        "cyan" => Ok(Color::DarkCyan),
        "white" => Ok(Color::Grey),
        "bright black" => Ok(Color::DarkGrey),
        "bright red" => Ok(Color::Red),
        "bright green" => Ok(Color::Green),
        "bright yellow" => Ok(Color::Yellow),
        "bright blue" => Ok(Color::Blue),
        "bright magenta" => Ok(Color::Magenta),
        "bright cyan" => Ok(Color::Cyan),
        "bright white" => Ok(Color::White),
        _ => color_for_hex(name_or_hex)
            .ok_or_else(|| config::ConfigError::Message(format!("invalid color: {}", name_or_hex))),
    }
}

fn color_for_hex(color: &str) -> Option<Color> {
    if color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        let r = u8::from_str_radix(&color[1..3], 16);
        let g = u8::from_str_radix(&color[3..5], 16);
        let b = u8::from_str_radix(&color[5..7], 16);
        match (r, g, b) {
            (Ok(r), Ok(g), Ok(b)) => Some(Color::Rgb { r, g, b }),
            _ => None,
        }
    } else {
        None
    }
}

impl<W: Write> Write for ColorFormatter<W> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        /*
        We clear the current style at the end of each line, and then we re-apply the style
        after the newline. There are several reasons for this:

         * We can more easily skip styling a trailing blank line, which other
           internal code then can correctly detect as having a trailing
           newline.

         * Some tools (like `less -R`) add an extra newline if the final
           character is not a newline (e.g. if there's a color reset after
           it), which led to an annoying blank line after the diff summary in
           e.g. `jj status`.

         * Since each line is styled independently, you get all the necessary
           escapes even when grepping through the output.

         * Some terminals extend background color to the end of the terminal
           (i.e. past the newline character), which is probably not what the
           user wanted.

         * Some tools (like `less -R`) get confused and lose coloring of lines
           after a newline.
         */
        for line in data.split_inclusive(|b| *b == b'\n') {
            if line.ends_with(b"\n") {
                self.write_new_style()?;
                write_sanitized(&mut self.output, &line[..line.len() - 1])?;
                let labels = mem::take(&mut self.labels);
                self.write_new_style()?;
                self.output.write_all(b"\n")?;
                self.labels = labels;
            } else {
                self.write_new_style()?;
                write_sanitized(&mut self.output, line)?;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.output.flush()
    }
}

impl<W: Write> Formatter for ColorFormatter<W> {
    fn raw(&mut self) -> &mut dyn Write {
        &mut self.output
    }

    fn push_label(&mut self, label: &str) -> io::Result<()> {
        self.labels.push(label.to_owned());
        Ok(())
    }

    fn pop_label(&mut self) -> io::Result<()> {
        self.labels.pop();
        if self.labels.is_empty() {
            self.write_new_style()?
        }
        Ok(())
    }
}

impl<W: Write> Drop for ColorFormatter<W> {
    fn drop(&mut self) {
        // If a `ColorFormatter` was dropped without popping all labels first (perhaps
        // because of an error), let's still try to reset any currently active style.
        self.labels.clear();
        self.write_new_style().ok();
    }
}

/// Like buffered formatter, but records `push`/`pop_label()` calls.
///
/// This allows you to manipulate the recorded data without losing labels.
/// The recorded data and labels can be written to another formatter. If
/// the destination formatter has already been labeled, the recorded labels
/// will be stacked on top of the existing labels, and the subsequent data
/// may be colorized differently.
#[derive(Clone, Debug, Default)]
pub struct FormatRecorder {
    data: Vec<u8>,
    label_ops: Vec<(usize, LabelOp)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LabelOp {
    PushLabel(String),
    PopLabel,
}

impl FormatRecorder {
    pub fn new() -> Self {
        FormatRecorder::default()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    fn push_label_op(&mut self, op: LabelOp) {
        self.label_ops.push((self.data.len(), op));
    }

    pub fn replay(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.replay_with(formatter, |formatter, range| {
            formatter.write_all(&self.data[range])
        })
    }

    pub fn replay_with(
        &self,
        formatter: &mut dyn Formatter,
        mut write_data: impl FnMut(&mut dyn Formatter, Range<usize>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut last_pos = 0;
        let mut flush_data = |formatter: &mut dyn Formatter, pos| -> io::Result<()> {
            if last_pos != pos {
                write_data(formatter, last_pos..pos)?;
                last_pos = pos;
            }
            Ok(())
        };
        for (pos, op) in &self.label_ops {
            flush_data(formatter, *pos)?;
            match op {
                LabelOp::PushLabel(label) => formatter.push_label(label)?,
                LabelOp::PopLabel => formatter.pop_label()?,
            }
        }
        flush_data(formatter, self.data.len())
    }
}

impl Write for FormatRecorder {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.data.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Formatter for FormatRecorder {
    fn raw(&mut self) -> &mut dyn Write {
        panic!("raw output isn't supported by FormatRecorder")
    }

    fn push_label(&mut self, label: &str) -> io::Result<()> {
        self.push_label_op(LabelOp::PushLabel(label.to_owned()));
        Ok(())
    }

    fn pop_label(&mut self) -> io::Result<()> {
        self.push_label_op(LabelOp::PopLabel);
        Ok(())
    }
}

fn write_sanitized(output: &mut impl Write, buf: &[u8]) -> Result<(), Error> {
    if buf.contains(&b'\x1b') {
        let mut sanitized = Vec::with_capacity(buf.len());
        for b in buf {
            if *b == b'\x1b' {
                sanitized.extend_from_slice("␛".as_bytes());
            } else {
                sanitized.push(*b);
            }
        }
        output.write_all(&sanitized)
    } else {
        output.write_all(buf)
    }
}

#[cfg(test)]
mod tests {
    use std::str;

    use super::*;

    fn config_from_string(text: &str) -> config::Config {
        config::Config::builder()
            .add_source(config::File::from_str(text, config::FileFormat::Toml))
            .build()
            .unwrap()
    }

    #[test]
    fn test_plaintext_formatter() {
        // Test that PlainTextFormatter ignores labels.
        let mut output: Vec<u8> = vec![];
        let mut formatter = PlainTextFormatter::new(&mut output);
        formatter.push_label("warning").unwrap();
        write!(formatter, "hello").unwrap();
        formatter.pop_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"hello");
    }

    #[test]
    fn test_plaintext_formatter_ansi_codes_in_text() {
        // Test that ANSI codes in the input text are NOT escaped.
        let mut output: Vec<u8> = vec![];
        let mut formatter = PlainTextFormatter::new(&mut output);
        write!(formatter, "\x1b[1mactually bold\x1b[0m").unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[1mactually bold[0m");
    }

    #[test]
    fn test_sanitizing_formatter_ansi_codes_in_text() {
        // Test that ANSI codes in the input text are escaped.
        let mut output: Vec<u8> = vec![];
        let mut formatter = SanitizingFormatter::new(&mut output);
        write!(formatter, "\x1b[1mnot actually bold\x1b[0m").unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"␛[1mnot actually bold␛[0m");
    }

    #[test]
    fn test_color_formatter_color_codes() {
        // Test the color code for each color.
        let colors = [
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "magenta",
            "cyan",
            "white",
            "bright black",
            "bright red",
            "bright green",
            "bright yellow",
            "bright blue",
            "bright magenta",
            "bright cyan",
            "bright white",
        ];
        let mut config_builder = config::Config::builder();
        for color in colors {
            // Use the color name as the label.
            config_builder = config_builder
                .set_override(format!("colors.{}", color.replace(' ', "-")), color)
                .unwrap();
        }
        let mut output: Vec<u8> = vec![];
        let mut formatter =
            ColorFormatter::for_config(&mut output, &config_builder.build().unwrap()).unwrap();
        for color in colors {
            formatter.push_label(&color.replace(' ', "-")).unwrap();
            write!(formatter, " {color} ").unwrap();
            formatter.pop_label().unwrap();
            writeln!(formatter).unwrap();
        }
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @r###"
        [38;5;0m black [39m
        [38;5;1m red [39m
        [38;5;2m green [39m
        [38;5;3m yellow [39m
        [38;5;4m blue [39m
        [38;5;5m magenta [39m
        [38;5;6m cyan [39m
        [38;5;7m white [39m
        [38;5;8m bright black [39m
        [38;5;9m bright red [39m
        [38;5;10m bright green [39m
        [38;5;11m bright yellow [39m
        [38;5;12m bright blue [39m
        [38;5;13m bright magenta [39m
        [38;5;14m bright cyan [39m
        [38;5;15m bright white [39m
        "###);
    }

    #[test]
    fn test_color_formatter_hex_colors() {
        // Test the color code for each color.
        let labels_and_colors = [
            ["black", "#000000"],
            ["white", "#ffffff"],
            ["pastel-blue", "#AFE0D9"],
        ];
        let mut config_builder = config::Config::builder();
        for [label, color] in labels_and_colors {
            // Use the color name as the label.
            config_builder = config_builder
                .set_override(format!("colors.{}", label), color)
                .unwrap();
        }
        let mut output: Vec<u8> = vec![];
        let mut formatter =
            ColorFormatter::for_config(&mut output, &config_builder.build().unwrap()).unwrap();
        for [label, _] in labels_and_colors {
            formatter.push_label(&label.replace(' ', "-")).unwrap();
            write!(formatter, " {label} ").unwrap();
            formatter.pop_label().unwrap();
            writeln!(formatter).unwrap();
        }
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @r###"
        [38;2;0;0;0m black [39m
        [38;2;255;255;255m white [39m
        [38;2;175;224;217m pastel-blue [39m
        "###);
    }

    #[test]
    fn test_color_formatter_single_label() {
        // Test that a single label can be colored and that the color is reset
        // afterwards.
        let config = config_from_string(
            r#"
        colors.inside = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        write!(formatter, " before ").unwrap();
        formatter.push_label("inside").unwrap();
        write!(formatter, " inside ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " after ").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @" before [38;5;2m inside [39m after ");
    }

    #[test]
    fn test_color_formatter_attributes() {
        // Test that each attribute of the style can be set and that they can be
        // combined in a single rule or by using multiple rules.
        let config = config_from_string(
            r#"
        colors.red_fg = { fg = "red" }
        colors.blue_bg = { bg = "blue" }
        colors.bold_font = { bold = true }
        colors.underlined_text = { underline = true }
        colors.multiple = { fg = "green", bg = "yellow", bold = true, underline = true }
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("red_fg").unwrap();
        write!(formatter, " fg only ").unwrap();
        formatter.pop_label().unwrap();
        writeln!(formatter).unwrap();
        formatter.push_label("blue_bg").unwrap();
        write!(formatter, " bg only ").unwrap();
        formatter.pop_label().unwrap();
        writeln!(formatter).unwrap();
        formatter.push_label("bold_font").unwrap();
        write!(formatter, " bold only ").unwrap();
        formatter.pop_label().unwrap();
        writeln!(formatter).unwrap();
        formatter.push_label("underlined_text").unwrap();
        write!(formatter, " underlined only ").unwrap();
        formatter.pop_label().unwrap();
        writeln!(formatter).unwrap();
        formatter.push_label("multiple").unwrap();
        write!(formatter, " single rule ").unwrap();
        formatter.pop_label().unwrap();
        writeln!(formatter).unwrap();
        formatter.push_label("red_fg").unwrap();
        formatter.push_label("blue_bg").unwrap();
        write!(formatter, " two rules ").unwrap();
        formatter.pop_label().unwrap();
        formatter.pop_label().unwrap();
        writeln!(formatter).unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @r###"
        [38;5;1m fg only [39m
        [48;5;4m bg only [49m
        [1m bold only [0m
        [4m underlined only [24m
        [1m[4m[38;5;2m[48;5;3m single rule [0m
        [38;5;1m[48;5;4m two rules [39m[49m
        "###);
    }

    #[test]
    fn test_color_formatter_bold_reset() {
        // Test that we don't lose other attributes when we reset the bold attribute.
        let config = config_from_string(
            r#"
        colors.not_bold = { fg = "red", bg = "blue", underline = true }
        colors.bold_font = { bold = true }
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("not_bold").unwrap();
        write!(formatter, " not bold ").unwrap();
        formatter.push_label("bold_font").unwrap();
        write!(formatter, " bold ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " not bold again ").unwrap();
        formatter.pop_label().unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[4m[38;5;1m[48;5;4m not bold [1m bold [0m[4m[38;5;1m[48;5;4m not bold again [24m[39m[49m");
    }

    #[test]
    fn test_color_formatter_no_space() {
        // Test that two different colors can touch.
        let config = config_from_string(
            r#"
        colors.red = "red"
        colors.green = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        write!(formatter, "before").unwrap();
        formatter.push_label("red").unwrap();
        write!(formatter, "first").unwrap();
        formatter.pop_label().unwrap();
        formatter.push_label("green").unwrap();
        write!(formatter, "second").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, "after").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"before[38;5;1mfirst[39m[38;5;2msecond[39mafter");
    }

    #[test]
    fn test_color_formatter_ansi_codes_in_text() {
        // Test that ANSI codes in the input text are escaped.
        let config = config_from_string(
            r#"
        colors.red = "red"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("red").unwrap();
        write!(formatter, "\x1b[1mnot actually bold\x1b[0m").unwrap();
        formatter.pop_label().unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[38;5;1m␛[1mnot actually bold␛[0m[39m");
    }

    #[test]
    fn test_color_formatter_nested() {
        // A color can be associated with a combination of labels. A more specific match
        // overrides a less specific match. After the inner label is removed, the outer
        // color is used again (we don't reset).
        let config = config_from_string(
            r#"
        colors.outer = "blue"
        colors.inner = "red"
        colors."outer inner" = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        write!(formatter, " before outer ").unwrap();
        formatter.push_label("outer").unwrap();
        write!(formatter, " before inner ").unwrap();
        formatter.push_label("inner").unwrap();
        write!(formatter, " inside inner ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " after inner ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " after outer ").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @" before outer [38;5;4m before inner [38;5;2m inside inner [38;5;4m after inner [39m after outer ");
    }

    #[test]
    fn test_color_formatter_partial_match() {
        // A partial match doesn't count
        let config = config_from_string(
            r#"
        colors."outer inner" = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("outer").unwrap();
        write!(formatter, " not colored ").unwrap();
        formatter.push_label("inner").unwrap();
        write!(formatter, " colored ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " not colored ").unwrap();
        formatter.pop_label().unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @" not colored [38;5;2m colored [39m not colored ");
    }

    #[test]
    fn test_color_formatter_unrecognized_color() {
        // An unrecognized color causes an error.
        let config = config_from_string(
            r#"
        colors."outer" = "red"
        colors."outer inner" = "bloo"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let err = ColorFormatter::for_config(&mut output, &config)
            .unwrap_err()
            .to_string();
        insta::assert_snapshot!(err,
        @"invalid color: bloo");
    }

    #[test]
    fn test_color_formatter_unrecognized_hex_color() {
        // An unrecognized hex color causes an error.
        let config = config_from_string(
            r##"
            colors."outer" = "red"
            colors."outer inner" = "#ffgggg"
            "##,
        );
        let mut output: Vec<u8> = vec![];
        let err = ColorFormatter::for_config(&mut output, &config)
            .unwrap_err()
            .to_string();
        insta::assert_snapshot!(err,
            @"invalid color: #ffgggg");
    }

    #[test]
    fn test_color_formatter_normal_color() {
        // The "default" color resets the color. It is possible to reset only the
        // background or only the foreground.
        let config = config_from_string(
            r#"
        colors."outer" = {bg="yellow", fg="blue"}
        colors."outer default_fg" = "default"
        colors."outer default_bg" = {bg = "default"}
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("outer").unwrap();
        write!(formatter, "Blue on yellow, ").unwrap();
        formatter.push_label("default_fg").unwrap();
        write!(formatter, " default fg, ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " and back.\nBlue on yellow, ").unwrap();
        formatter.push_label("default_bg").unwrap();
        write!(formatter, " default bg, ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " and back.").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @r###"
        [38;5;4m[48;5;3mBlue on yellow, [39m default fg, [38;5;4m and back.[39m[49m
        [38;5;4m[48;5;3mBlue on yellow, [49m default bg, [48;5;3m and back.[39m[49m
        "###);
    }

    #[test]
    fn test_color_formatter_sibling() {
        // A partial match on one rule does not eliminate other rules.
        let config = config_from_string(
            r#"
        colors."outer1 inner1" = "red"
        colors.inner2 = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("outer1").unwrap();
        formatter.push_label("inner2").unwrap();
        write!(formatter, " hello ").unwrap();
        formatter.pop_label().unwrap();
        formatter.pop_label().unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @"[38;5;2m hello [39m");
    }

    #[test]
    fn test_color_formatter_reverse_order() {
        // Rules don't match labels out of order
        let config = config_from_string(
            r#"
        colors."inner outer" = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("outer").unwrap();
        formatter.push_label("inner").unwrap();
        write!(formatter, " hello ").unwrap();
        formatter.pop_label().unwrap();
        formatter.pop_label().unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @" hello ");
    }

    #[test]
    fn test_color_formatter_innermost_wins() {
        // When two labels match, the innermost one wins.
        let config = config_from_string(
            r#"
        colors."a" = "red"
        colors."b" = "green"
        colors."a c" = "blue"
        colors."b c" = "yellow"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("a").unwrap();
        write!(formatter, " a1 ").unwrap();
        formatter.push_label("b").unwrap();
        write!(formatter, " b1 ").unwrap();
        formatter.push_label("c").unwrap();
        write!(formatter, " c ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " b2 ").unwrap();
        formatter.pop_label().unwrap();
        write!(formatter, " a2 ").unwrap();
        formatter.pop_label().unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @"[38;5;1m a1 [38;5;2m b1 [38;5;3m c [38;5;2m b2 [38;5;1m a2 [39m");
    }

    #[test]
    fn test_color_formatter_dropped() {
        // Test that the style gets reset if the formatter is dropped without popping
        // all labels.
        let config = config_from_string(
            r#"
        colors.outer = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        formatter.push_label("outer").unwrap();
        formatter.push_label("inner").unwrap();
        write!(formatter, " inside ").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[38;5;2m inside [39m");
    }

    #[test]
    fn test_heading_labeled_writer() {
        let config = config_from_string(
            r#"
        colors.inner = "green"
        colors."inner heading" = "red"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter: Box<dyn Formatter> =
            Box::new(ColorFormatter::for_config(&mut output, &config).unwrap());
        HeadingLabeledWriter::new(formatter.as_mut(), "inner", "Should be noop: ");
        let mut writer = HeadingLabeledWriter::new(formatter.as_mut(), "inner", "Heading: ");
        write!(writer, "Message").unwrap();
        writeln!(writer, " continues").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @r###"
        [38;5;1mHeading: [38;5;2mMessage[39m[38;5;2m continues[39m
        "###);
    }

    #[test]
    fn test_heading_labeled_writer_empty_string() {
        let mut output: Vec<u8> = vec![];
        let mut formatter: Box<dyn Formatter> = Box::new(PlainTextFormatter::new(&mut output));
        let mut writer = HeadingLabeledWriter::new(formatter.as_mut(), "inner", "Heading: ");
        // write_fmt() is called even if the format string is empty. I don't
        // know if that's guaranteed, but let's record the current behavior.
        write!(writer, "").unwrap();
        write!(writer, "").unwrap();
        drop(formatter);
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"Heading: ");
    }

    #[test]
    fn test_format_recorder() {
        let mut recorder = FormatRecorder::new();
        write!(recorder, " outer1 ").unwrap();
        recorder.push_label("inner").unwrap();
        write!(recorder, " inner1 ").unwrap();
        write!(recorder, " inner2 ").unwrap();
        recorder.pop_label().unwrap();
        write!(recorder, " outer2 ").unwrap();

        insta::assert_snapshot!(
            str::from_utf8(recorder.data()).unwrap(),
            @" outer1  inner1  inner2  outer2 ");

        // Replayed output should be labeled.
        let config = config_from_string(r#" colors.inner = "red" "#);
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        recorder.replay(&mut formatter).unwrap();
        drop(formatter);
        insta::assert_snapshot!(
            String::from_utf8(output).unwrap(),
            @" outer1 [38;5;1m inner1  inner2 [39m outer2 ");

        // Replayed output should be split at push/pop_label() call.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config).unwrap();
        recorder
            .replay_with(&mut formatter, |formatter, range| {
                let data = &recorder.data()[range];
                write!(formatter, "<<{}>>", str::from_utf8(data).unwrap())
            })
            .unwrap();
        drop(formatter);
        insta::assert_snapshot!(
            String::from_utf8(output).unwrap(),
            @"<< outer1 >>[38;5;1m<< inner1  inner2 >>[39m<< outer2 >>");
    }
}
