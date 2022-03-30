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

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub struct TestEnvironment {
    _temp_dir: TempDir,
    env_root: PathBuf,
    home_dir: PathBuf,
    config_path: PathBuf,
    command_number: RefCell<i64>,
}

impl Default for TestEnvironment {
    fn default() -> Self {
        let tmp_dir = TempDir::new().unwrap();
        let env_root = tmp_dir.path().canonicalize().unwrap();
        let home_dir = env_root.join("home");
        std::fs::create_dir(&home_dir).unwrap();
        let config_path = env_root.join("config.toml");
        std::fs::write(&config_path, b"").unwrap();
        Self {
            _temp_dir: tmp_dir,
            env_root,
            home_dir,
            config_path,
            command_number: RefCell::new(0),
        }
    }
}

impl TestEnvironment {
    pub fn jj_cmd(&self, current_dir: &Path, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("jj").unwrap();
        cmd.current_dir(current_dir);
        cmd.args(args);
        cmd.env_clear();
        cmd.env("RUST_BACKTRACE", "1");
        cmd.env("HOME", self.home_dir.to_str().unwrap());
        let timestamp = chrono::DateTime::parse_from_rfc3339("2001-02-03T04:05:06+07:00").unwrap();
        let mut command_number = self.command_number.borrow_mut();
        *command_number += 1;
        cmd.env("JJ_CONFIG", self.config_path.to_str().unwrap());
        let timestamp = timestamp + chrono::Duration::seconds(*command_number);
        cmd.env("JJ_TIMESTAMP", timestamp.to_rfc3339());
        cmd.env("JJ_USER", "Test User");
        cmd.env("JJ_EMAIL", "test.user@example.com");
        cmd
    }

    /// Run a `jj` command, check that it was successful, and return its stdout
    pub fn jj_cmd_success(&self, current_dir: &Path, args: &[&str]) -> String {
        let assert = self.jj_cmd(current_dir, args).assert().success().stderr("");
        get_stdout_string(&assert)
    }

    pub fn env_root(&self) -> &Path {
        &self.env_root
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
}

pub fn get_stdout_string(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}
