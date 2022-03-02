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

use std::path::{Path, PathBuf};

use itertools::Itertools;
use regex::Regex;
use tempfile::TempDir;

pub struct TestEnvironment {
    _temp_dir: TempDir,
    env_root: PathBuf,
    home_dir: PathBuf,
}

impl Default for TestEnvironment {
    fn default() -> Self {
        let tmp_dir = TempDir::new().unwrap();
        let env_root = tmp_dir.path().canonicalize().unwrap();
        let home_dir = env_root.join("home");
        Self {
            _temp_dir: tmp_dir,
            env_root,
            home_dir,
        }
    }
}

impl TestEnvironment {
    pub fn jj_cmd(&self, current_dir: &Path, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("jj").unwrap();
        cmd.current_dir(current_dir);
        cmd.args(args);
        cmd.env("HOME", self.home_dir.to_str().unwrap());
        cmd
    }

    pub fn env_root(&self) -> &Path {
        &self.env_root
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }
}

pub fn get_stdout_string(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

pub fn capture_matches(
    assert: assert_cmd::assert::Assert,
    pattern: &str,
) -> (assert_cmd::assert::Assert, Vec<String>) {
    let stdout_string = get_stdout_string(&assert);
    let assert = assert.stdout(predicates::str::is_match(pattern).unwrap());
    let matches = Regex::new(pattern)
        .unwrap()
        .captures(&stdout_string)
        .unwrap()
        .iter()
        .map(|m| m.unwrap().as_str().to_owned())
        .collect_vec();
    (assert, matches)
}
