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

use std::io::Write;

use jujutsu::testutils::{get_stdout_string, TestEnvironment};

#[test]
fn test_no_commit_working_copy() {
    let test_env = TestEnvironment::default();
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo", "--git"])
        .assert()
        .success();

    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "initial").unwrap();
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id"])
        .assert()
        .success();
    let stdout_string = get_stdout_string(&assert);
    insta::assert_snapshot!(stdout_string, @r###"
    @ 438471f3fbf1004298d8fb01eeb13663a051a643
    o 0000000000000000000000000000000000000000
    "###);

    // Modify the file. With --no-commit-working-copy, we still get the same commit
    // ID.
    std::fs::write(repo_path.join("file"), "modified").unwrap();
    test_env
        .jj_cmd(
            &repo_path,
            &["log", "-T", "commit_id", "--no-commit-working-copy"],
        )
        .assert()
        .success()
        .stdout(stdout_string);

    // But without --no-commit-working-copy, we get a new commit ID.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    @ fab22d1acf5bb9c5aa48cb2c3dd2132072a359ca
    o 0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_repo_arg_with_init() {
    let test_env = TestEnvironment::default();
    let assert = test_env
        .jj_cmd(test_env.env_root(), &["init", "-R=.", "repo"])
        .assert()
        .failure();
    insta::assert_snapshot!(get_stdout_string(&assert), @"Error: '--repository' cannot be used with 'init'
");
}

#[test]
fn test_repo_arg_with_git_clone() {
    let test_env = TestEnvironment::default();
    let assert = test_env
        .jj_cmd(test_env.env_root(), &["git", "clone", "-R=.", "remote"])
        .assert()
        .failure();
    insta::assert_snapshot!(get_stdout_string(&assert), @"Error: '--repository' cannot be used with 'git clone'
");
}

#[test]
fn test_color_config() {
    let test_env = TestEnvironment::default();
    let mut config_file = std::fs::File::options()
        .append(true)
        .open(test_env.config_path())
        .unwrap();
    config_file
        .write_all(
            br#"[ui]
color="always""#,
        )
        .unwrap();
    config_file.flush().unwrap();
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo", "--git"])
        .assert()
        .success();

    let repo_path = test_env.env_root().join("repo");
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    @ [1;34m230dd059e1b059aefc0da06a2e5a7dbf22362f22[0m
    o [34m0000000000000000000000000000000000000000[0m
    "###);
}
