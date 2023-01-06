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
use std::sync::Arc;
use std::{fmt, io};

use crossterm::queue;
use crossterm::style::{Attribute, Color, SetAttribute, SetForegroundColor};
use itertools::Itertools;

// Lets the caller label strings and translates the labels to colors
pub trait Formatter: Write {
    fn write_bytes(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_all(data)
    }

    fn write_str(&mut self, text: &str) -> io::Result<()> {
        self.write_all(text.as_bytes())
    }

    fn add_label(&mut self, label: &str) -> io::Result<()>;

    fn remove_label(&mut self) -> io::Result<()>;
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
        self.add_label(label)?;
        // Call `remove_label()` whether or not `write_inner()` fails, but don't let
        // its error replace the one from `write_inner()`.
        write_inner(self).and(self.remove_label())
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
        self.formatter
            .borrow_mut()
            .with_label(self.label.as_ref(), |formatter| formatter.write_fmt(args))
    }
}

/// Creates `Formatter` instances with preconfigured parameters.
#[derive(Clone, Debug)]
pub struct FormatterFactory {
    kind: FormatterFactoryKind,
}

#[derive(Clone, Debug)]
enum FormatterFactoryKind {
    PlainText,
    Color {
        rules: Arc<HashMap<Vec<String>, Style>>,
    },
}

impl FormatterFactory {
    pub fn prepare(config: &config::Config, color: bool) -> Self {
        let kind = if color {
            let rules = Arc::new(rules_from_config(config));
            FormatterFactoryKind::Color { rules }
        } else {
            FormatterFactoryKind::PlainText
        };
        FormatterFactory { kind }
    }

    pub fn new_formatter<'output, W: Write + 'output>(
        &self,
        output: W,
    ) -> Box<dyn Formatter + 'output> {
        match &self.kind {
            FormatterFactoryKind::PlainText => Box::new(PlainTextFormatter::new(output)),
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
    fn add_label(&mut self, _label: &str) -> io::Result<()> {
        Ok(())
    }

    fn remove_label(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Style {
    pub fg_color: Color,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg_color: Color::Reset,
        }
    }
}

pub struct ColorFormatter<W> {
    output: W,
    rules: Arc<HashMap<Vec<String>, Style>>,
    labels: Vec<String>,
    cached_styles: HashMap<Vec<String>, Style>,
    current_style: Style,
}

impl<W: Write> ColorFormatter<W> {
    pub fn new(output: W, rules: Arc<HashMap<Vec<String>, Style>>) -> ColorFormatter<W> {
        ColorFormatter {
            output,
            rules,
            labels: vec![],
            cached_styles: HashMap::new(),
            current_style: Style::default(),
        }
    }

    pub fn for_config(output: W, config: &config::Config) -> ColorFormatter<W> {
        let rules = rules_from_config(config);
        Self::new(output, Arc::new(rules))
    }

    fn current_style(&mut self) -> Style {
        if let Some(cached) = self.cached_styles.get(&self.labels) {
            cached.clone()
        } else {
            let mut best_match = (-1, Style::default());
            for (labels, style) in self.rules.as_ref() {
                let mut num_matching = 0;
                let mut labels_iter = self.labels.iter();
                let mut valid = true;
                for required_label in labels {
                    loop {
                        match labels_iter.next() {
                            Some(label) if label == required_label => {
                                num_matching += 1;
                            }
                            None => {
                                valid = false;
                            }
                            Some(_) => {
                                continue;
                            }
                        }
                        break;
                    }
                }
                if !valid {
                    continue;
                }
                if num_matching >= best_match.0 {
                    best_match = (num_matching, style.clone())
                }
            }

            let style = best_match.1;
            self.cached_styles
                .insert(self.labels.clone(), style.clone());
            style
        }
    }

    fn write_new_style(&mut self) -> io::Result<()> {
        let new_style = self.current_style();
        if new_style != self.current_style {
            // For now, make bright colors imply bold font. That better matches our
            // behavior from when we used ANSI codes 30-37 plus an optional 1 for
            // bold/bright (we now use code 38 for setting foreground color).
            // TODO: Make boldness configurable separately from color
            if !is_bright(&self.current_style.fg_color) && is_bright(&new_style.fg_color) {
                queue!(self.output, SetAttribute(Attribute::Bold))?;
            } else if !is_bright(&new_style.fg_color) && is_bright(&self.current_style.fg_color) {
                queue!(self.output, SetAttribute(Attribute::Reset))?;
            }
            queue!(self.output, SetForegroundColor(new_style.fg_color))?;
            self.current_style = new_style;
        }
        Ok(())
    }
}

fn is_bright(color: &Color) -> bool {
    matches!(
        color,
        Color::DarkGrey
            | Color::Red
            | Color::Green
            | Color::Yellow
            | Color::Blue
            | Color::Magenta
            | Color::Cyan
            | Color::White
    )
}

fn rules_from_config(config: &config::Config) -> HashMap<Vec<String>, Style> {
    let mut result = HashMap::new();
    if let Ok(table) = config.get_table("colors") {
        for (key, value) in table {
            let labels = key
                .split_whitespace()
                .map(ToString::to_string)
                .collect_vec();
            match value.kind {
                config::ValueKind::String(color_name) => {
                    let style = Style {
                        fg_color: color_for_name(&color_name),
                    };
                    result.insert(labels, style);
                }
                config::ValueKind::Table(style_table) => {
                    let mut style = Style::default();
                    if let Some(value) = style_table.get("fg") {
                        if let config::ValueKind::String(color_name) = &value.kind {
                            style.fg_color = color_for_name(color_name);
                        }
                    }
                    result.insert(labels, style);
                }
                _ => {}
            }
        }
    }
    result
}

fn color_for_name(color_name: &str) -> Color {
    match color_name {
        "black" => Color::Black,
        "red" => Color::DarkRed,
        "green" => Color::DarkGreen,
        "yellow" => Color::DarkYellow,
        "blue" => Color::DarkBlue,
        "magenta" => Color::DarkMagenta,
        "cyan" => Color::DarkCyan,
        "white" => Color::Grey,
        "bright black" => Color::DarkGrey,
        "bright red" => Color::Red,
        "bright green" => Color::Green,
        "bright yellow" => Color::Yellow,
        "bright blue" => Color::Blue,
        "bright magenta" => Color::Magenta,
        "bright cyan" => Color::Cyan,
        "bright white" => Color::White,
        _ => Color::Reset,
    }
}

impl<W: Write> Write for ColorFormatter<W> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        self.output.write(data)
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.output.flush()
    }
}

impl<W: Write> Formatter for ColorFormatter<W> {
    fn add_label(&mut self, label: &str) -> io::Result<()> {
        self.labels.push(label.to_owned());
        self.write_new_style()
    }

    fn remove_label(&mut self) -> io::Result<()> {
        self.labels.pop();
        self.write_new_style()
    }
}

#[cfg(test)]
mod tests {
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
        formatter.add_label("warning").unwrap();
        formatter.write_str("hello").unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"hello");
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
            ColorFormatter::for_config(&mut output, &config_builder.build().unwrap());
        for color in colors {
            formatter.add_label(&color.replace(' ', "-")).unwrap();
            formatter.write_str(&format!(" {color} ")).unwrap();
            formatter.remove_label().unwrap();
            formatter.write_str("\n").unwrap();
        }
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @r###"
        [38;5;0m black [39m
        [38;5;1m red [39m
        [38;5;2m green [39m
        [38;5;3m yellow [39m
        [38;5;4m blue [39m
        [38;5;5m magenta [39m
        [38;5;6m cyan [39m
        [38;5;7m white [39m
        [1m[38;5;8m bright black [0m[39m
        [1m[38;5;9m bright red [0m[39m
        [1m[38;5;10m bright green [0m[39m
        [1m[38;5;11m bright yellow [0m[39m
        [1m[38;5;12m bright blue [0m[39m
        [1m[38;5;13m bright magenta [0m[39m
        [1m[38;5;14m bright cyan [0m[39m
        [1m[38;5;15m bright white [0m[39m
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.write_str(" before ").unwrap();
        formatter.add_label("inside").unwrap();
        formatter.write_str(" inside ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" after ").unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @" before [38;5;2m inside [39m after ");
    }

    #[test]
    fn test_color_formatter_attributes() {
        // Test that each attribute of the style can be set.
        let config = config_from_string(
            r#"
        colors.red_fg = { fg = "red" }
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("red_fg").unwrap();
        formatter.write_str(" fg only ").unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[38;5;1m fg only [39m");
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.write_str("before").unwrap();
        formatter.add_label("red").unwrap();
        formatter.write_str("first").unwrap();
        formatter.remove_label().unwrap();
        formatter.add_label("green").unwrap();
        formatter.write_str("second").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str("after").unwrap();
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("red").unwrap();
        formatter
            .write_str("\x1b[1mnot actually bold\x1b[0m")
            .unwrap();
        formatter.remove_label().unwrap();
        // TODO: Replace the ANSI escape (\x1b) by something else (🌈?)
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[38;5;1m[1mnot actually bold[0m[39m");
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.write_str(" before outer ").unwrap();
        formatter.add_label("outer").unwrap();
        formatter.write_str(" before inner ").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" inside inner ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" after inner ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" after outer ").unwrap();
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("outer").unwrap();
        formatter.write_str(" not colored ").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" colored ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" not colored ").unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @" not colored [38;5;2m colored [39m not colored ");
    }

    #[test]
    fn test_color_formatter_unrecognized_color() {
        // An unrecognized color is ignored; it doesn't reset the color.
        let config = config_from_string(
            r#"
        colors."outer" = "red"
        colors."outer inner" = "bloo"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("outer").unwrap();
        formatter.write_str(" red before ").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" still red inside ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" also red afterwards ").unwrap();
        formatter.remove_label().unwrap();
        // TODO: Make this not reset the color inside
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @"[38;5;1m red before [39m still red inside [38;5;1m also red afterwards [39m");
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("outer1").unwrap();
        formatter.add_label("inner2").unwrap();
        formatter.write_str(" hello ").unwrap();
        formatter.remove_label().unwrap();
        formatter.remove_label().unwrap();
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("outer").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" hello ").unwrap();
        formatter.remove_label().unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @" hello ");
    }

    #[test]
    fn test_color_formatter_number_of_matches_matters() {
        // Rules that match more labels take precedence.
        let config = config_from_string(
            r#"
        colors."a b" = "red"
        colors."c" = "green"
        colors."b c d" = "blue"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("a").unwrap();
        formatter.write_str(" a1 ").unwrap();
        formatter.add_label("b").unwrap();
        formatter.write_str(" b1 ").unwrap();
        formatter.add_label("c").unwrap();
        formatter.write_str(" c1 ").unwrap();
        formatter.add_label("d").unwrap();
        formatter.write_str(" d ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" c2 ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" b2 ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" a2 ").unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @" a1 [38;5;1m b1  c1 [38;5;4m d [38;5;1m c2  b2 [39m a2 ");
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
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        formatter.add_label("a").unwrap();
        formatter.write_str(" a1 ").unwrap();
        formatter.add_label("b").unwrap();
        formatter.write_str(" b1 ").unwrap();
        formatter.add_label("c").unwrap();
        formatter.write_str(" c ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" b2 ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" a2 ").unwrap();
        formatter.remove_label().unwrap();
        // TODO: This is currently not deterministic.
        // insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        // @"[38;5;1m a1 [38;5;2m b1 [38;5;3m c [38;5;2m b2 [38;5;1m a2 [39m");
    }
}
