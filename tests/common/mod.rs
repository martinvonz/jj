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

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::{Captures, Regex};
use tempfile::TempDir;
use testutils;

pub struct TestEnvironment {
    _temp_dir: TempDir,
    env_root: PathBuf,
    home_dir: PathBuf,
    config_path: PathBuf,
    env_vars: HashMap<String, String>,
    config_file_number: RefCell<i64>,
    command_number: RefCell<i64>,
}

impl Default for TestEnvironment {
    fn default() -> Self {
        testutils::hermetic_libgit2();

        let tmp_dir = testutils::new_temp_dir();
        let env_root = tmp_dir.path().canonicalize().unwrap();
        let home_dir = env_root.join("home");
        std::fs::create_dir(&home_dir).unwrap();
        let config_dir = env_root.join("config");
        std::fs::create_dir(&config_dir).unwrap();
        let env_vars = HashMap::new();
        let env = Self {
            _temp_dir: tmp_dir,
            env_root,
            home_dir,
            config_path: config_dir,
            env_vars,
            config_file_number: RefCell::new(0),
            command_number: RefCell::new(0),
        };
        // Use absolute timestamps in the operation log to make tests independent of the
        // current time.
        env.add_config(
            r#"
[template-aliases]
'format_time_range(time_range)' = 'time_range.start() ++ " - " ++ time_range.end()'
        "#,
        );
        env
    }
}

impl TestEnvironment {
    pub fn jj_cmd(&self, current_dir: &Path, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("jj").unwrap();
        cmd.current_dir(current_dir);
        cmd.args(args);
        cmd.env_clear();
        for (key, value) in &self.env_vars {
            cmd.env(key, value);
        }
        cmd.env("RUST_BACKTRACE", "1");
        cmd.env("HOME", self.home_dir.to_str().unwrap());
        cmd.env("JJ_CONFIG", self.config_path.to_str().unwrap());
        cmd.env("JJ_USER", "Test User");
        cmd.env("JJ_EMAIL", "test.user@example.com");
        cmd.env("JJ_OP_HOSTNAME", "host.example.com");
        cmd.env("JJ_OP_USERNAME", "test-username");

        let mut command_number = self.command_number.borrow_mut();
        *command_number += 1;
        cmd.env("JJ_RANDOMNESS_SEED", command_number.to_string());
        let timestamp = chrono::DateTime::parse_from_rfc3339("2001-02-03T04:05:06+07:00").unwrap();
        let timestamp = timestamp + chrono::Duration::seconds(*command_number);
        cmd.env("JJ_TIMESTAMP", timestamp.to_rfc3339());
        cmd.env("JJ_OP_TIMESTAMP", timestamp.to_rfc3339());

        cmd
    }

    /// Run a `jj` command, check that it was successful, and return its stdout
    pub fn jj_cmd_success(&self, current_dir: &Path, args: &[&str]) -> String {
        let assert = self.jj_cmd(current_dir, args).assert().success().stderr("");
        self.normalize_output(&get_stdout_string(&assert))
    }

    /// Run a `jj` command, check that it failed with code 1, and return its
    /// stderr
    #[must_use]
    pub fn jj_cmd_failure(&self, current_dir: &Path, args: &[&str]) -> String {
        let assert = self.jj_cmd(current_dir, args).assert().code(1).stdout("");
        self.normalize_output(&get_stderr_string(&assert))
    }

    /// Run a `jj` command and check that it failed with code 2 (for invalid
    /// usage)
    #[must_use]
    pub fn jj_cmd_cli_error(&self, current_dir: &Path, args: &[&str]) -> String {
        let assert = self.jj_cmd(current_dir, args).assert().code(2).stdout("");
        self.normalize_output(&get_stderr_string(&assert))
    }

    /// Run a `jj` command, check that it failed with code 255, and return its
    /// stderr
    #[must_use]
    pub fn jj_cmd_internal_error(&self, current_dir: &Path, args: &[&str]) -> String {
        let assert = self.jj_cmd(current_dir, args).assert().code(255).stdout("");
        self.normalize_output(&get_stderr_string(&assert))
    }

    pub fn env_root(&self) -> &Path {
        &self.env_root
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn set_config_path(&mut self, config_path: PathBuf) {
        self.config_path = config_path;
    }

    pub fn add_config(&self, content: &str) {
        if self.config_path.is_file() {
            panic!("add_config not supported when config_path is a file");
        }
        // Concatenating two valid TOML files does not (generally) result in a valid
        // TOML file, so we create a new file every time instead.
        let mut config_file_number = self.config_file_number.borrow_mut();
        *config_file_number += 1;
        let config_file_number = *config_file_number;
        std::fs::write(
            self.config_path
                .join(format!("config{config_file_number:04}.toml")),
            content,
        )
        .unwrap();
    }

    pub fn add_env_var(&mut self, key: &str, val: &str) {
        self.env_vars.insert(key.to_string(), val.to_string());
    }

    /// Sets up the fake editor to read an edit script from the returned path
    /// Also sets up the fake editor as a merge tool named "fake-editor"
    pub fn set_up_fake_editor(&mut self) -> PathBuf {
        let editor_path = assert_cmd::cargo::cargo_bin("fake-editor");
        assert!(editor_path.is_file());
        // Simplified TOML escaping, hoping that there are no '"' or control characters
        // in it
        let escaped_editor_path = editor_path.to_str().unwrap().replace('\\', r"\\");
        self.add_env_var("EDITOR", &escaped_editor_path);
        self.add_config(&format!(
            r###"
                    [ui]
                    merge-editor = "fake-editor"

                    [merge-tools]
                    fake-editor.program="{escaped_editor_path}"
                    fake-editor.merge-args = ["$output"]
                "###
        ));
        let edit_script = self.env_root().join("edit_script");
        std::fs::write(&edit_script, "").unwrap();
        self.add_env_var("EDIT_SCRIPT", edit_script.to_str().unwrap());
        edit_script
    }

    /// Sets up the fake diff-editor to read an edit script from the returned
    /// path
    pub fn set_up_fake_diff_editor(&mut self) -> PathBuf {
        let diff_editor_path = assert_cmd::cargo::cargo_bin("fake-diff-editor");
        assert!(diff_editor_path.is_file());
        // Simplified TOML escaping, hoping that there are no '"' or control characters
        // in it
        let escaped_diff_editor_path = diff_editor_path.to_str().unwrap().replace('\\', r"\\");
        self.add_config(&format!(r#"ui.diff-editor = "{escaped_diff_editor_path}""#));
        let edit_script = self.env_root().join("diff_edit_script");
        std::fs::write(&edit_script, "").unwrap();
        self.add_env_var("DIFF_EDIT_SCRIPT", edit_script.to_str().unwrap());
        edit_script
    }

    pub fn normalize_output(&self, text: &str) -> String {
        let text = text.replace("jj.exe", "jj");
        let regex = Regex::new(&format!(
            r"{}(\S+)",
            regex::escape(&self.env_root.display().to_string())
        ))
        .unwrap();
        regex
            .replace_all(&text, |caps: &Captures| {
                format!("$TEST_ENV{}", caps[1].replace('\\', "/"))
            })
            .to_string()
    }
}

pub fn get_stdout_string(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

pub fn get_stderr_string(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}
