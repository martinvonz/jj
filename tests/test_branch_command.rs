// Copyright 2022 Google LLC
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
fn test_branch_mutually_exclusive_actions() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_failure(&repo_path, &["branch", "--forget", "--delete", "foo"]);
    test_env.jj_cmd_failure(
        &repo_path,
        &["branch", "--delete", "--allow-backwards", "foo"],
    );
}

#[test]
fn test_branch_multiple_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let assert = test_env
        .jj_cmd(&repo_path, &["branch", "foo", "bar"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @"warning: Updating multiple branches (2).
");

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ bar foo 230dd059e1b0
    o  000000000000
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "--delete", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  230dd059e1b0
    o  000000000000
    "###);
}

#[test]
fn test_branch_hint_no_branches() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let assert = test_env
        .jj_cmd(&repo_path, &["branch", "--delete"])
        .assert()
        .success();
    let stderr = get_stderr_string(&assert);
    insta::assert_snapshot!(stderr, @"warning: No branches provided.
");
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    test_env.jj_cmd_success(cwd, &["log", "-T", r#"branches " " commit_id.short()"#])
}
