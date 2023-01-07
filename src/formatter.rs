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
        colors: Arc<HashMap<String, String>>,
    },
}

impl FormatterFactory {
    pub fn prepare(config: &config::Config, color: bool) -> Self {
        let kind = if color {
            let colors = Arc::new(config_colors(config));
            FormatterFactoryKind::Color { colors }
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
            FormatterFactoryKind::Color { colors } => {
                Box::new(ColorFormatter::new(output, colors.clone()))
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

pub struct ColorFormatter<W> {
    output: W,
    colors: Arc<HashMap<String, String>>,
    labels: Vec<String>,
    cached_colors: HashMap<Vec<String>, Vec<u8>>,
    current_color: Vec<u8>,
}

fn config_colors(config: &config::Config) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if let Ok(table) = config.get_table("colors") {
        for (key, value) in table {
            result.insert(key, value.to_string());
        }
    }
    result
}

impl<W: Write> ColorFormatter<W> {
    pub fn new(output: W, colors: Arc<HashMap<String, String>>) -> ColorFormatter<W> {
        ColorFormatter {
            output,
            colors,
            labels: vec![],
            cached_colors: HashMap::new(),
            current_color: b"\x1b[0m".to_vec(),
        }
    }

    fn current_color(&mut self) -> Vec<u8> {
        if let Some(cached) = self.cached_colors.get(&self.labels) {
            cached.clone()
        } else {
            let mut best_match = (-1, "");
            for (key, value) in self.colors.as_ref() {
                let mut num_matching = 0;
                let mut labels_iter = self.labels.iter();
                let mut valid = true;
                for required_label in key.split_whitespace() {
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
                    best_match = (num_matching, value)
                }
            }

            let color = color_for_name(best_match.1);
            self.cached_colors
                .insert(self.labels.clone(), color.clone());
            color
        }
    }

    fn write_new_color(&mut self) -> io::Result<()> {
        let new_color = self.current_color();
        if new_color != self.current_color {
            self.output.write_all(&new_color)?;
            self.current_color = new_color;
        }
        Ok(())
    }
}

fn color_for_name(color_name: &str) -> Vec<u8> {
    match color_name {
        "black" => b"\x1b[30m".to_vec(),
        "red" => b"\x1b[31m".to_vec(),
        "green" => b"\x1b[32m".to_vec(),
        "yellow" => b"\x1b[33m".to_vec(),
        "blue" => b"\x1b[34m".to_vec(),
        "magenta" => b"\x1b[35m".to_vec(),
        "cyan" => b"\x1b[36m".to_vec(),
        "white" => b"\x1b[37m".to_vec(),
        "bright black" => b"\x1b[1;30m".to_vec(),
        "bright red" => b"\x1b[1;31m".to_vec(),
        "bright green" => b"\x1b[1;32m".to_vec(),
        "bright yellow" => b"\x1b[1;33m".to_vec(),
        "bright blue" => b"\x1b[1;34m".to_vec(),
        "bright magenta" => b"\x1b[1;35m".to_vec(),
        "bright cyan" => b"\x1b[1;36m".to_vec(),
        "bright white" => b"\x1b[1;37m".to_vec(),
        _ => b"\x1b[0m".to_vec(),
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
        self.write_new_color()
    }

    fn remove_label(&mut self) -> io::Result<()> {
        self.labels.pop();
        self.write_new_color()
    }
}

#[cfg(test)]
mod tests {
    use maplit::hashmap;

    use super::*;

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
        let mut color_config = HashMap::new();
        for color in &colors {
            // Use the color name as the label.
            color_config.insert(color.replace(' ', "-").to_string(), color.to_string());
        }
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(&mut output, Arc::new(color_config));
        for color in colors {
            formatter.add_label(&color.replace(' ', "-")).unwrap();
            formatter.write_str(&format!(" {color} ")).unwrap();
            formatter.remove_label().unwrap();
            formatter.write_str("\n").unwrap();
        }
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @r###"
        [30m black [0m
        [31m red [0m
        [32m green [0m
        [33m yellow [0m
        [34m blue [0m
        [35m magenta [0m
        [36m cyan [0m
        [37m white [0m
        [1;30m bright black [0m
        [1;31m bright red [0m
        [1;32m bright green [0m
        [1;33m bright yellow [0m
        [1;34m bright blue [0m
        [1;35m bright magenta [0m
        [1;36m bright cyan [0m
        [1;37m bright white [0m
        "###);
    }

    #[test]
    fn test_color_formatter_single_label() {
        // Test that a single label can be colored and that the color is reset
        // afterwards.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "inside".to_string() => "green".to_string(),
            }),
        );
        formatter.write_str(" before ").unwrap();
        formatter.add_label("inside").unwrap();
        formatter.write_str(" inside ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" after ").unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @" before [32m inside [0m after ");
    }

    #[test]
    fn test_color_formatter_no_space() {
        // Test that two different colors can touch.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "red".to_string() => "red".to_string(),
                "green".to_string() => "green".to_string(),
            }),
        );
        formatter.write_str("before").unwrap();
        formatter.add_label("red").unwrap();
        formatter.write_str("first").unwrap();
        formatter.remove_label().unwrap();
        formatter.add_label("green").unwrap();
        formatter.write_str("second").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str("after").unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"before[31mfirst[0m[32msecond[0mafter");
    }

    #[test]
    fn test_color_formatter_ansi_codes_in_text() {
        // Test that ANSI codes in the input text are escaped.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "red".to_string() => "red".to_string(),
            }),
        );
        formatter.add_label("red").unwrap();
        formatter
            .write_str("\x1b[1mnot actually bold\x1b[0m")
            .unwrap();
        formatter.remove_label().unwrap();
        // TODO: Replace the ANSI escape (\x1b) by something else (🌈?)
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), @"[31m[1mnot actually bold[0m[0m");
    }

    #[test]
    fn test_color_formatter_nested() {
        // A color can be associated with a combination of labels. A more specific match
        // overrides a less specific match. After the inner label is removed, the outer
        // color is used again (we don't reset).
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "outer".to_string() => "blue".to_string(),
                "inner".to_string() => "red".to_string(),
                "outer inner".to_string() => "green".to_string(),
            }),
        );
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
        @" before outer [34m before inner [32m inside inner [34m after inner [0m after outer ");
    }

    #[test]
    fn test_color_formatter_partial_match() {
        // A partial match doesn't count
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "outer inner".to_string() => "green".to_string(),
            }),
        );
        formatter.add_label("outer").unwrap();
        formatter.write_str(" not colored ").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" colored ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" not colored ").unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @" not colored [32m colored [0m not colored ");
    }

    #[test]
    fn test_color_formatter_unrecognized_color() {
        // An unrecognized color is ignored; it doesn't reset the color.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "outer".to_string() => "red".to_string(),
                "outer inner".to_string() => "bloo".to_string(),
            }),
        );
        formatter.add_label("outer").unwrap();
        formatter.write_str(" red before ").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" still red inside ").unwrap();
        formatter.remove_label().unwrap();
        formatter.write_str(" also red afterwards ").unwrap();
        formatter.remove_label().unwrap();
        // TODO: Make this not reset the color inside
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @"[31m red before [0m still red inside [31m also red afterwards [0m");
    }

    #[test]
    fn test_color_formatter_sibling() {
        // A partial match on one rule does not eliminate other rules.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "outer1 inner1".to_string() => "red".to_string(),
                "inner2".to_string() => "green".to_string(),
            }),
        );
        formatter.add_label("outer1").unwrap();
        formatter.add_label("inner2").unwrap();
        formatter.write_str(" hello ").unwrap();
        formatter.remove_label().unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @"[32m hello [0m");
    }

    #[test]
    fn test_color_formatter_reverse_order() {
        // Rules don't match labels out of order
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "inner outer".to_string() => "green".to_string(),
            }),
        );
        formatter.add_label("outer").unwrap();
        formatter.add_label("inner").unwrap();
        formatter.write_str(" hello ").unwrap();
        formatter.remove_label().unwrap();
        formatter.remove_label().unwrap();
        insta::assert_snapshot!(String::from_utf8(output).unwrap(), 
        @" hello ");
    }

    #[test]
    fn test_color_formatter_number_of_matches_matters() {
        // Rules that match more labels take precedence.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "a b".to_string() => "red".to_string(),
                "c".to_string() => "green".to_string(),
                "b c d".to_string() => "blue".to_string(),
            }),
        );
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
        @" a1 [31m b1  c1 [34m d [31m c2  b2 [0m a2 ");
    }

    #[test]
    fn test_color_formatter_innermost_wins() {
        // When two labels match, the innermost one wins.
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::new(
            &mut output,
            Arc::new(hashmap! {
                "a".to_string() => "red".to_string(),
                "b".to_string() => "green".to_string(),
                "a c".to_string() => "blue".to_string(),
                "b c".to_string() => "yellow".to_string(),
            }),
        );
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
        // @"[31m a1 [32m b1 [33m c [32m b2 [31m a2 [0m");
    }
}
