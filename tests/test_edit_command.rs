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

use crate::common::TestEnvironment;

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
    test_env.jj_cmd_cli_error(&repo_path, &["edit"]);

    // Makes the specified commit the working-copy commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["edit", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 5c9d6c787f29 first
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o 37ed5225d0fd second
    @ 5c9d6c787f29 first
    o 000000000000 (no description set)
    "###);
    insta::assert_snapshot!(read_file(&repo_path.join("file1")), @"0");

    // Changes in the working copy are amended into the commit
    std::fs::write(repo_path.join("file2"), "0").unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    Rebased 1 descendant commits onto updated working copy
    o 57e61f6b2ce1 second
    @ f1b9706b17d0 first
    o 000000000000 (no description set)
    "###);
}

fn read_file(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    test_env.jj_cmd_success(cwd, &["log", "-T", r#"commit_id.short() " " description"#])
}

#[test]
fn test_edit_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root"]);
    insta::assert_snapshot!(stderr, @"Error: Cannot rewrite the root commit");
}
