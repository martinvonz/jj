// Copyright 2022 The Jujutsu Authors
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

use std::path::Path;

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_branch_multiple_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let assert = test_env
        .jj_cmd(&repo_path, &["branch", "set", "foo", "bar"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @"warning: Updating multiple branches (2).
");

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 230dd059e1b0
    ◉   000000000000
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "delete", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   230dd059e1b0
    ◉   000000000000
    "###);
}

#[test]
fn test_branch_empty_name() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["branch", "create", ""]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '<NAMES>...' but none was supplied

    For more information, try '--help'.
    "###);
}
#[test]
fn test_branch_forget_glob() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-1"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "bar-2"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-3"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-4"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 230dd059e1b0
    ◉   000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "forget", "--glob", "foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-4 230dd059e1b0
    ◉   000000000000
    "###);

    // Forgetting a branch via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "branch", "forget", "foo-4", "--glob", "foo-*", "--glob", "foo-*",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 230dd059e1b0
    ◉   000000000000
    "###);

    // Malformed glob
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "forget", "--glob", "foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to compile glob: Pattern syntax error near position 4: invalid range pattern
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"branches ++ " " ++ commit_id.short()"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
