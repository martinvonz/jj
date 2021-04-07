// Copyright 2020 Google LLC
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

use std::collections::HashMap;
use std::io;
use std::io::{Error, Read, Write};

use jujube_lib::settings::UserSettings;

// Lets the caller label strings and translates the labels to colors
pub trait Styler: Write {
    fn write_bytes(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_all(data)
    }

    fn write_str(&mut self, text: &str) -> io::Result<()> {
        self.write_all(text.as_bytes())
    }

    fn write_from_reader(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        let mut buffer = vec![];
        reader.read_to_end(&mut buffer).unwrap();
        self.write_all(buffer.as_slice())
    }

    fn add_label(&mut self, label: String) -> io::Result<()>;

    fn remove_label(&mut self) -> io::Result<()>;
}

pub struct PlainTextStyler<'a> {
    output: Box<dyn Write + 'a>,
}

impl<'a> PlainTextStyler<'a> {
    pub fn new(output: Box<dyn Write + 'a>) -> PlainTextStyler<'a> {
        Self { output }
    }
}

impl Write for PlainTextStyler<'_> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        self.output.write(data)
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.output.flush()
    }
}

impl Styler for PlainTextStyler<'_> {
    fn add_label(&mut self, _label: String) -> io::Result<()> {
        Ok(())
    }

    fn remove_label(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ColorStyler<'a> {
    output: Box<dyn Write + 'a>,
    colors: HashMap<String, String>,
    labels: Vec<String>,
    cached_colors: HashMap<Vec<String>, Vec<u8>>,
    current_color: Vec<u8>,
}

fn config_colors(user_settings: &UserSettings) -> HashMap<String, String> {
    let mut result = HashMap::new();
    result.insert(String::from("error"), String::from("red"));

    result.insert(String::from("commit_id"), String::from("blue"));
    result.insert(String::from("commit_id open"), String::from("green"));
    result.insert(String::from("change_id"), String::from("magenta"));
    result.insert(String::from("author"), String::from("yellow"));
    result.insert(String::from("committer"), String::from("yellow"));
    result.insert(String::from("git_refs"), String::from("magenta"));
    result.insert(String::from("pruned"), String::from("red"));
    result.insert(String::from("obsolete"), String::from("red"));
    result.insert(String::from("orphan"), String::from("red"));
    result.insert(String::from("divergent"), String::from("red"));
    result.insert(String::from("conflict"), String::from("red"));

    result.insert(String::from("diff header"), String::from("yellow"));
    result.insert(String::from("diff left"), String::from("red"));
    result.insert(String::from("diff right"), String::from("green"));

    result.insert(String::from("op-log id"), String::from("blue"));
    result.insert(String::from("op-log user"), String::from("yellow"));
    result.insert(String::from("op-log time"), String::from("magenta"));

    if let Ok(table) = user_settings.config().get_table("colors") {
        for (key, value) in table {
            result.insert(key, value.to_string());
        }
    }
    result
}

impl<'a> ColorStyler<'a> {
    pub fn new(output: Box<dyn Write + 'a>, user_settings: &UserSettings) -> ColorStyler<'a> {
        ColorStyler {
            output,
            colors: config_colors(user_settings),
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
            for (key, value) in &self.colors {
                let mut num_matching = 0;
                let mut valid = true;
                for label in key.split_whitespace() {
                    if !self.labels.contains(&label.to_string()) {
                        valid = false;
                        break;
                    }
                    num_matching += 1;
                }
                if !valid {
                    continue;
                }
                if num_matching >= best_match.0 {
                    best_match = (num_matching, value)
                }
            }

            let color = self.color_for_name(&best_match.1);
            self.cached_colors
                .insert(self.labels.clone(), color.clone());
            color
        }
    }

    fn color_for_name(&self, color_name: &str) -> Vec<u8> {
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
}

impl Write for ColorStyler<'_> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        self.output.write(data)
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.output.flush()
    }
}

impl Styler for ColorStyler<'_> {
    fn add_label(&mut self, label: String) -> io::Result<()> {
        self.labels.push(label);
        let new_color = self.current_color();
        if new_color != self.current_color {
            self.output.write_all(&new_color)?;
        }
        self.current_color = new_color;
        Ok(())
    }

    fn remove_label(&mut self) -> io::Result<()> {
        self.labels.pop();
        let new_color = self.current_color();
        if new_color != self.current_color {
            self.output.write_all(&new_color)?;
        }
        self.current_color = new_color;
        Ok(())
    }
}
