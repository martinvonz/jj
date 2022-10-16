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
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
use std::{fmt, io};

use atty::Stream;
use jujutsu_lib::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use jujutsu_lib::settings::UserSettings;

use crate::formatter::{Formatter, FormatterFactory};

pub struct Ui<'a> {
    cwd: PathBuf,
    formatter_factory: FormatterFactory,
    output_pair: UiOutputPair<'a>,
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

fn use_color(choice: ColorChoice, maybe_tty: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => maybe_tty && atty::is(Stream::Stdout),
    }
}

impl<'stdout> Ui<'stdout> {
    pub fn new(
        cwd: PathBuf,
        stdout: Box<dyn Write + 'stdout>,
        stderr: Box<dyn Write + 'stdout>,
        color: bool,
        settings: UserSettings,
    ) -> Ui<'stdout> {
        let formatter_factory = FormatterFactory::prepare(&settings, color);
        Ui {
            cwd,
            formatter_factory,
            output_pair: UiOutputPair::Dyn {
                stdout: Mutex::new(stdout),
                stderr: Mutex::new(stderr),
            },
            settings,
        }
    }

    pub fn for_terminal(settings: UserSettings) -> Ui<'static> {
        let cwd = std::env::current_dir().unwrap();
        let color = use_color(color_setting(&settings), true);
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
        let maybe_tty = matches!(&self.output_pair, UiOutputPair::Terminal { .. });
        let color = use_color(choice, maybe_tty);
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
            UiOutputPair::Dyn { stdout, .. } => {
                let output = DynWriteLock(stdout.lock().unwrap());
                self.new_formatter(output)
            }
            UiOutputPair::Terminal { stdout, .. } => self.new_formatter(stdout.lock()),
        }
    }

    /// Creates a formatter for the locked stderr stream.
    pub fn stderr_formatter<'a>(&'a self) -> Box<dyn Formatter + 'a> {
        match &self.output_pair {
            UiOutputPair::Dyn { stderr, .. } => {
                let output = DynWriteLock(stderr.lock().unwrap());
                self.new_formatter(output)
            }
            UiOutputPair::Terminal { stderr, .. } => self.new_formatter(stderr.lock()),
        }
    }

    pub fn write(&mut self, text: &str) -> io::Result<()> {
        let data = text.as_bytes();
        match &mut self.output_pair {
            UiOutputPair::Dyn { stdout, .. } => stdout.get_mut().unwrap().write_all(data),
            UiOutputPair::Terminal { stdout, .. } => stdout.write_all(data),
        }
    }

    pub fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        match &mut self.output_pair {
            UiOutputPair::Dyn { stdout, .. } => stdout.get_mut().unwrap().write_fmt(fmt),
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

    /// Parses a path relative to cwd into a RepoPath relative to wc_path
    pub fn parse_file_path(
        &self,
        wc_path: &Path,
        input: &str,
    ) -> Result<RepoPath, FilePathParseError> {
        let repo_relative_path = relative_path(wc_path, &self.cwd.join(input));
        let mut repo_path = RepoPath::root();
        for component in repo_relative_path.components() {
            match component {
                Component::Normal(a) => {
                    repo_path = repo_path.join(&RepoPathComponent::from(a.to_str().unwrap()));
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if let Some(parent) = repo_path.parent() {
                        repo_path = parent;
                    } else {
                        return Err(FilePathParseError::InputNotInRepo(input.to_string()));
                    }
                }
                _ => {
                    return Err(FilePathParseError::InputNotInRepo(input.to_string()));
                }
            }
        }
        Ok(repo_path)
    }
}

enum UiOutputPair<'output> {
    Dyn {
        stdout: Mutex<Box<dyn Write + 'output>>,
        stderr: Mutex<Box<dyn Write + 'output>>,
    },
    Terminal {
        stdout: Stdout,
        stderr: Stderr,
    },
}

/// Wrapper to implement `Write` for locked `Box<dyn Write>`.
struct DynWriteLock<'a, 'output>(MutexGuard<'a, Box<dyn Write + 'output>>);

impl Write for DynWriteLock<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum FilePathParseError {
    InputNotInRepo(String),
}

pub fn relative_path(mut from: &Path, to: &Path) -> PathBuf {
    let mut result = PathBuf::from("");
    loop {
        if let Ok(suffix) = to.strip_prefix(from) {
            result = result.join(suffix);
            break;
        }
        if let Some(parent) = from.parent() {
            result = result.join("..");
            from = parent;
        } else {
            result = to.to_path_buf();
            break;
        }
    }
    if result.as_os_str().is_empty() {
        result = PathBuf::from(".");
    }
    result
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use jujutsu_lib::testutils;

    use super::*;

    #[test]
    fn parse_file_path_wc_in_cwd() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("repo");
        let wc_path = cwd_path.clone();
        let mut unused_stdout_buf = vec![];
        let mut unused_stderr_buf = vec![];
        let unused_stdout = Box::new(Cursor::new(&mut unused_stdout_buf));
        let unused_stderr = Box::new(Cursor::new(&mut unused_stderr_buf));
        let ui = Ui::new(
            cwd_path,
            unused_stdout,
            unused_stderr,
            false,
            UserSettings::default(),
        );

        assert_eq!(ui.parse_file_path(&wc_path, ""), Ok(RepoPath::root()));
        assert_eq!(ui.parse_file_path(&wc_path, "."), Ok(RepoPath::root()));
        assert_eq!(
            ui.parse_file_path(&wc_path, "file"),
            Ok(RepoPath::from_internal_string("file"))
        );
        // Both slash and the platform's separator are allowed
        assert_eq!(
            ui.parse_file_path(&wc_path, &format!("dir{}file", std::path::MAIN_SEPARATOR)),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "dir/file"),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, ".."),
            Err(FilePathParseError::InputNotInRepo("..".to_string()))
        );
        // TODO: handle these cases:
        // assert_eq!(ui.parse_file_path(&cwd_path, "../repo"),
        // Ok(RepoPath::root())); assert_eq!(ui.parse_file_path(&cwd_path,
        // "../repo/file"), Ok(RepoPath::from_internal_string("file")));
    }

    #[test]
    fn parse_file_path_wc_in_cwd_parent() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("dir");
        let wc_path = cwd_path.parent().unwrap().to_path_buf();
        let mut unused_stdout_buf = vec![];
        let mut unused_stderr_buf = vec![];
        let unused_stdout = Box::new(Cursor::new(&mut unused_stdout_buf));
        let unused_stderr = Box::new(Cursor::new(&mut unused_stderr_buf));
        let ui = Ui::new(
            cwd_path,
            unused_stdout,
            unused_stderr,
            false,
            UserSettings::default(),
        );

        assert_eq!(
            ui.parse_file_path(&wc_path, ""),
            Ok(RepoPath::from_internal_string("dir"))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "."),
            Ok(RepoPath::from_internal_string("dir"))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "file"),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "subdir/file"),
            Ok(RepoPath::from_internal_string("dir/subdir/file"))
        );
        assert_eq!(ui.parse_file_path(&wc_path, ".."), Ok(RepoPath::root()));
        assert_eq!(
            ui.parse_file_path(&wc_path, "../.."),
            Err(FilePathParseError::InputNotInRepo("../..".to_string()))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "../other-dir/file"),
            Ok(RepoPath::from_internal_string("other-dir/file"))
        );
    }

    #[test]
    fn parse_file_path_wc_in_cwd_child() {
        let temp_dir = testutils::new_temp_dir();
        let cwd_path = temp_dir.path().join("cwd");
        let wc_path = cwd_path.join("repo");
        let mut unused_stdout_buf = vec![];
        let mut unused_stderr_buf = vec![];
        let unused_stdout = Box::new(Cursor::new(&mut unused_stdout_buf));
        let unused_stderr = Box::new(Cursor::new(&mut unused_stderr_buf));
        let ui = Ui::new(
            cwd_path,
            unused_stdout,
            unused_stderr,
            false,
            UserSettings::default(),
        );

        assert_eq!(
            ui.parse_file_path(&wc_path, ""),
            Err(FilePathParseError::InputNotInRepo("".to_string()))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "not-repo"),
            Err(FilePathParseError::InputNotInRepo("not-repo".to_string()))
        );
        assert_eq!(ui.parse_file_path(&wc_path, "repo"), Ok(RepoPath::root()));
        assert_eq!(
            ui.parse_file_path(&wc_path, "repo/file"),
            Ok(RepoPath::from_internal_string("file"))
        );
        assert_eq!(
            ui.parse_file_path(&wc_path, "repo/dir/file"),
            Ok(RepoPath::from_internal_string("dir/file"))
        );
    }
}
