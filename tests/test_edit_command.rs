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

use crate::common::{get_stderr_string, TestEnvironment};

pub mod common;

#[test]
fn test_edit() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    std::fs::write(repo_path.join("file1"), "0").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "second"]);
    std::fs::write(repo_path.join("file1"), "1").unwrap();

    // Errors out without argument
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["edit"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      <REVISION>

    Usage: jj edit <REVISION>

    For more information, try '--help'.
    "###);

    // Makes the specified commit the working-copy commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["edit", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: f41390a5efbf first
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ●  b2f7e9c549aa second
    @  f41390a5efbf first
    ●  000000000000
    "###);
    insta::assert_snapshot!(read_file(&repo_path.join("file1")), @"0");

    // Changes in the working copy are amended into the commit
    std::fs::write(repo_path.join("file2"), "0").unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    Rebased 1 descendant commits onto updated working copy
    ●  51d937a3eeb4 second
    @  409306de8f44 first
    ●  000000000000
    "###);
}

#[test]
// Windows says "Access is denied" when trying to delete the object file.
#[cfg(unix)]
fn test_edit_current_wc_commit_missing() {
    // Test that we get a reasonable error message when the current working-copy
    // commit is missing
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "second"]);
    test_env.jj_cmd_success(&repo_path, &["edit", "@-"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ●  5c52832c3483 second
    @  69542c1984c1 first
    ●  000000000000
    "###);

    // Make the Git backend fail to read the current working copy commit
    let commit_object_path = repo_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git")
        .join("objects")
        .join("69")
        .join("542c1984c1f9d91f7c6c9c9e6941782c944bd9");
    std::fs::remove_file(commit_object_path).unwrap();

    // Pass --ignore-working-copy to avoid triggering the error at snapshot time
    let assert = test_env
        .jj_cmd(
            &repo_path,
            &["edit", "--ignore-working-copy", "5c52832c3483"],
        )
        .assert()
        .code(255);
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Internal error: Failed to edit a commit: Current working-copy commit not found: Object 69542c1984c1f9d91f7c6c9c9e6941782c944bd9 of type commit not found: object not found - no match for id (69542c1984c1f9d91f7c6c9c9e6941782c944bd9); class=Odb (9); code=NotFound (-3)
    "###);
}

fn read_file(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

#[test]
fn test_edit_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root"]);
    insta::assert_snapshot!(stderr, @"Error: Cannot rewrite the root commit");
}
