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

use std::fmt::{Debug, Error, Formatter};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use jujutsu_lib::testutils::{new_user_home, user_settings};

use crate::commands;
use crate::ui::Ui;

pub struct CommandRunner {
    pub cwd: PathBuf,
    pub stdout_buf: Vec<u8>,
}

impl CommandRunner {
    pub fn new(cwd: &Path) -> CommandRunner {
        CommandRunner {
            cwd: cwd.to_owned(),
            stdout_buf: vec![],
        }
    }

    pub fn run(self, mut args: Vec<&str>) -> CommandOutput {
        let _home_dir = new_user_home();
        let mut stdout_buf = self.stdout_buf;
        let stdout = Box::new(Cursor::new(&mut stdout_buf));
        let ui = Ui::new(self.cwd, stdout, false, user_settings());
        args.insert(0, "jj");
        let status = commands::dispatch(ui, args);
        CommandOutput { status, stdout_buf }
    }
}

#[derive(PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout_buf: Vec<u8>,
}

impl CommandOutput {
    pub fn stdout_string(&self) -> String {
        String::from_utf8(self.stdout_buf.clone()).unwrap()
    }
}

impl Debug for CommandOutput {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("CommandOutput")
            .field("status", &self.status)
            .field("stdout_buf", &String::from_utf8_lossy(&self.stdout_buf))
            .finish()
    }
}
