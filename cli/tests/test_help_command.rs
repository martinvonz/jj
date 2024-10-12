// Copyright 2024 The Jujutsu Authors
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

use crate::common::TestEnvironment;

#[test]
fn test_help() {
    let test_env = TestEnvironment::default();

    // Help command should work with commands
    let help_cmd_stdout = test_env.jj_cmd_success(test_env.env_root(), &["help", "log"]);
    let help_flag_stdout = test_env.jj_cmd_success(test_env.env_root(), &["log", "--help"]);
    assert_eq!(help_cmd_stdout, help_flag_stdout);

    // Help command should work with subcommands
    let help_cmd_stdout =
        test_env.jj_cmd_success(test_env.env_root(), &["help", "workspace", "root"]);
    let help_flag_stdout =
        test_env.jj_cmd_success(test_env.env_root(), &["workspace", "root", "--help"]);
    assert_eq!(help_cmd_stdout, help_flag_stdout);

    // Help command should not work recursively
    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["workspace", "help", "root"]);
    insta::assert_snapshot!(stderr, @r#"
    error: unrecognized subcommand 'help'

    Usage: jj workspace [OPTIONS] <COMMAND>

    For more information, try '--help'.
    "#);

    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["workspace", "add", "help"]);
    insta::assert_snapshot!(stderr, @r#"
    Error: There is no jj repo in "."
    "#);

    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["new", "help", "main"]);
    insta::assert_snapshot!(stderr, @r#"
    Error: There is no jj repo in "."
    "#);

    // Help command should output the same as --help for nonexistent commands
    let help_cmd_stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["help", "nonexistent"]);
    let help_flag_stderr =
        test_env.jj_cmd_cli_error(test_env.env_root(), &["nonexistent", "--help"]);
    assert_eq!(help_cmd_stderr, help_flag_stderr);

    // Some edge cases
    let help_cmd_stdout = test_env.jj_cmd_success(test_env.env_root(), &["help", "help"]);
    let help_flag_stdout = test_env.jj_cmd_success(test_env.env_root(), &["help", "--help"]);
    assert_eq!(help_cmd_stdout, help_flag_stdout);

    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["help", "unknown"]);
    insta::assert_snapshot!(stderr, @r#"
    error: unrecognized subcommand 'unknown'

      tip: a similar subcommand exists: 'undo'

    Usage: jj [OPTIONS] <COMMAND>

    For more information, try '--help'.
    "#);

    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["help", "log", "--", "-r"]);
    insta::assert_snapshot!(stderr, @r#"
    error: a value is required for '--revisions <REVISIONS>' but none was supplied

    For more information, try '--help'.
    "#);
}

#[test]
fn test_help_category() {
    let test_env = TestEnvironment::default();
    // Now that we have a custom help output, capture it
    let help_cmd_stdout = test_env.jj_cmd_success(test_env.env_root(), &["help"]);

    insta::with_settings!({filters => vec![
        (r"(?s).+(?<h>Help Categories:.+)", "$h"),
    ]}, {
        insta::assert_snapshot!(help_cmd_stdout, @r#"
        Help Categories:
          revsets           A functional language for selecting a set of revision
          tutorial          Show a tutorial to get started with jj
        "#);
    });

    // Help command should work with category commands
    let help_cmd_stdout = test_env.jj_cmd_success(test_env.env_root(), &["help", "revsets"]);
    // It should be equal to the docs
    assert_eq!(help_cmd_stdout, include_str!("../../docs/revsets.md"));
}
