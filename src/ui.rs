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

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::{fmt, io};

use atty::Stream;
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use jujutsu_lib::settings::UserSettings;

use crate::formatter::{ColorFormatter, Formatter, PlainTextFormatter};
use crate::templater::TemplateFormatter;

pub struct Ui<'a> {
    cwd: PathBuf,
    color: bool,
    formatter: Mutex<Box<dyn Formatter + 'a>>,
    settings: UserSettings,
}

fn new_formatter<'output>(
    settings: &UserSettings,
    color: bool,
    output: Box<dyn Write + 'output>,
) -> Box<dyn Formatter + 'output> {
    if color {
        Box::new(ColorFormatter::new(output, settings))
    } else {
        Box::new(PlainTextFormatter::new(output))
    }
}

fn use_color(settings: &UserSettings) -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    let color_setting = settings
        .config()
        .get_string("ui.color")
        .unwrap_or_else(|_| "auto".to_string());
    match color_setting.as_str() {
        "always" => true,
        "never" => false,
        _ => atty::is(Stream::Stdout),
    }
}

impl<'stdout> Ui<'stdout> {
    pub fn new(
        cwd: PathBuf,
        stdout: Box<dyn Write + 'stdout>,
        color: bool,
        settings: UserSettings,
    ) -> Ui<'stdout> {
        let formatter = Mutex::new(new_formatter(&settings, color, stdout));
        Ui {
            cwd,
            color,
            formatter,
            settings,
        }
    }

    pub fn for_terminal(settings: UserSettings) -> Ui<'static> {
        let cwd = std::env::current_dir().unwrap();
        let stdout: Box<dyn Write + 'static> = Box::new(io::stdout());
        let color = use_color(&settings);
        Ui::new(cwd, stdout, color, settings)
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn settings(&self) -> &UserSettings {
        &self.settings
    }

    pub fn new_formatter<'output>(
        &self,
        output: Box<dyn Write + 'output>,
    ) -> Box<dyn Formatter + 'output> {
        new_formatter(&self.settings, self.color, output)
    }

    pub fn stdout_formatter(&self) -> MutexGuard<Box<dyn Formatter + 'stdout>> {
        self.formatter.lock().unwrap()
    }

    pub fn write(&mut self, text: &str) -> io::Result<()> {
        self.stdout_formatter().write_str(text)
    }

    pub fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        self.stdout_formatter().write_fmt(fmt)
    }

    pub fn write_error(&mut self, text: &str) -> io::Result<()> {
        // TODO: We should print the error to stderr
        let mut formatter = self.stdout_formatter();
        formatter.add_label(String::from("error"))?;
        formatter.write_str(text)?;
        formatter.remove_label()?;
        Ok(())
    }

    pub fn write_commit_summary(
        &mut self,
        repo: RepoRef,
        workspace_id: &WorkspaceId,
        commit: &Commit,
    ) -> io::Result<()> {
        let template_string = self
            .settings
            .config()
            .get_string("template.commit_summary")
            .unwrap_or_else(|_| {
                String::from(
                    r#"label(if(open, "open"), commit_id.short() " " description.first_line())"#,
                )
            });
        let template =
            crate::template_parser::parse_commit_template(repo, workspace_id, &template_string);
        let mut formatter = self.stdout_formatter();
        let mut template_writer = TemplateFormatter::new(template, formatter.as_mut());
        template_writer.format(commit)?;
        Ok(())
    }

    pub fn format_file_path(&self, wc_path: &Path, file: &RepoPath) -> String {
        relative_path(&self.cwd, &file.to_fs_path(wc_path))
            .to_str()
            .unwrap()
            .to_owned()
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

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum FilePathParseError {
    InputNotInRepo(String),
}

pub fn relative_path(mut from: &Path, to: &Path) -> PathBuf {
    let mut result = PathBuf::from("");
    loop {
        if let Ok(suffix) = to.strip_prefix(from) {
            return result.join(suffix);
        }
        if let Some(parent) = from.parent() {
            result = result.join("..");
            from = parent;
        } else {
            return to.to_owned();
        }
    }
}
