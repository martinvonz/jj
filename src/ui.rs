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

use std::fmt;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use jujube_lib::commit::Commit;
use jujube_lib::repo::RepoRef;
use jujube_lib::settings::UserSettings;

use crate::styler::{ColorStyler, PlainTextStyler, Styler};
use crate::templater::TemplateFormatter;

pub struct Ui<'a> {
    cwd: PathBuf,
    styler: Mutex<Box<dyn Styler + 'a>>,
    settings: UserSettings,
}

impl<'a> Ui<'a> {
    pub fn new(
        cwd: PathBuf,
        stdout: Box<dyn Write + 'a>,
        is_atty: bool,
        settings: UserSettings,
    ) -> Ui<'a> {
        let styler: Box<dyn Styler + 'a> = if is_atty {
            Box::new(ColorStyler::new(stdout, &settings))
        } else {
            Box::new(PlainTextStyler::new(stdout))
        };
        let styler = Mutex::new(styler);
        Ui {
            cwd,
            styler,
            settings,
        }
    }

    pub fn for_terminal(settings: UserSettings) -> Ui<'static> {
        let cwd = std::env::current_dir().unwrap();
        let stdout: Box<dyn Write + 'static> = Box::new(io::stdout());
        Ui::new(cwd, stdout, true, settings)
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn settings(&self) -> &UserSettings {
        &self.settings
    }

    pub fn styler(&self) -> MutexGuard<Box<dyn Styler + 'a>> {
        self.styler.lock().unwrap()
    }

    pub fn write(&mut self, text: &str) {
        self.styler().write_str(text);
    }

    pub fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) {
        self.styler().write_fmt(fmt).unwrap()
    }

    pub fn write_error(&mut self, text: &str) {
        let mut styler = self.styler();
        styler.add_label(String::from("error"));
        styler.write_str(text);
        styler.remove_label();
    }

    pub fn write_commit_summary(&mut self, repo: RepoRef, commit: &Commit) {
        let template_string = self
            .settings
            .config()
            .get_str("template.commit_summary")
            .unwrap_or_else(|_| {
                String::from(
                    r#"label(if(open, "open"), commit_id.short() " " description.first_line())"#,
                )
            });
        let template = crate::template_parser::parse_commit_template(repo, &template_string);
        let mut styler = self.styler();
        let mut template_writer = TemplateFormatter::new(template, styler.as_mut());
        template_writer.format(commit);
    }
}
