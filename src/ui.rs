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

use std::io::{Stderr, Stdout, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fmt, io};

use atty::Stream;
use jujutsu_lib::settings::UserSettings;

use crate::formatter::{Formatter, FormatterFactory};

pub struct Ui {
    cwd: PathBuf,
    formatter_factory: FormatterFactory,
    output_pair: UiOutputPair,
    settings: UserSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorChoice {
    Always,
    Never,
    Auto,
}

impl Default for ColorChoice {
    fn default() -> Self {
        ColorChoice::Auto
    }
}

impl FromStr for ColorChoice {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "always" => Ok(ColorChoice::Always),
            "never" => Ok(ColorChoice::Never),
            "auto" => Ok(ColorChoice::Auto),
            _ => Err("must be one of always, never, or auto"),
        }
    }
}

fn color_setting(settings: &UserSettings) -> ColorChoice {
    settings
        .config()
        .get_string("ui.color")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

fn use_color(choice: ColorChoice) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => atty::is(Stream::Stdout),
    }
}

impl Ui {
    pub fn for_terminal(settings: UserSettings) -> Ui {
        let cwd = std::env::current_dir().unwrap();
        let color = use_color(color_setting(&settings));
        let formatter_factory = FormatterFactory::prepare(&settings, color);
        Ui {
            cwd,
            formatter_factory,
            output_pair: UiOutputPair::Terminal {
                stdout: io::stdout(),
                stderr: io::stderr(),
            },
            settings,
        }
    }

    /// Reconfigures the underlying outputs with the new color choice.
    pub fn reset_color(&mut self, choice: ColorChoice) {
        let color = use_color(choice);
        if self.formatter_factory.is_color() != color {
            self.formatter_factory = FormatterFactory::prepare(&self.settings, color);
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn settings(&self) -> &UserSettings {
        &self.settings
    }

    pub fn new_formatter<'output, W: Write + 'output>(
        &self,
        output: W,
    ) -> Box<dyn Formatter + 'output> {
        self.formatter_factory.new_formatter(output)
    }

    /// Creates a formatter for the locked stdout stream.
    ///
    /// Labels added to the returned formatter should be removed by caller.
    /// Otherwise the last color would persist.
    pub fn stdout_formatter<'a>(&'a self) -> Box<dyn Formatter + 'a> {
        match &self.output_pair {
            UiOutputPair::Terminal { stdout, .. } => self.new_formatter(stdout.lock()),
        }
    }

    /// Creates a formatter for the locked stderr stream.
    pub fn stderr_formatter<'a>(&'a self) -> Box<dyn Formatter + 'a> {
        match &self.output_pair {
            UiOutputPair::Terminal { stderr, .. } => self.new_formatter(stderr.lock()),
        }
    }

    /// Whether continuous feedback should be displayed for long-running
    /// operations
    pub fn use_progress_indicator(&self) -> bool {
        self.settings().use_progress_indicator() && atty::is(Stream::Stdout)
    }

    pub fn write(&mut self, text: &str) -> io::Result<()> {
        let data = text.as_bytes();
        match &mut self.output_pair {
            UiOutputPair::Terminal { stdout, .. } => stdout.write_all(data),
        }
    }

    pub fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        match &mut self.output_pair {
            UiOutputPair::Terminal { stdout, .. } => stdout.write_fmt(fmt),
        }
    }

    pub fn write_hint(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        let mut formatter = self.stderr_formatter();
        formatter.add_label("hint")?;
        formatter.write_str(text.as_ref())?;
        formatter.remove_label()?;
        Ok(())
    }

    pub fn write_warn(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        let mut formatter = self.stderr_formatter();
        formatter.add_label("warning")?;
        formatter.write_str(text.as_ref())?;
        formatter.remove_label()?;
        Ok(())
    }

    pub fn write_error(&mut self, text: &str) -> io::Result<()> {
        let mut formatter = self.stderr_formatter();
        formatter.add_label("error")?;
        formatter.write_str(text)?;
        formatter.remove_label()?;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        match &mut self.output_pair {
            UiOutputPair::Terminal { stdout, .. } => stdout.flush(),
        }
    }

    pub fn size(&self) -> Option<(u16, u16)> {
        crossterm::terminal::size().ok()
    }
}

enum UiOutputPair {
    Terminal { stdout: Stdout, stderr: Stderr },
}
