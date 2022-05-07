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

use std::ffi::OsString;

use crate::common::{get_stderr_string, TestEnvironment};

pub mod common;

#[test]
fn test_non_utf8_arg() {
    let test_env = TestEnvironment::default();
    #[cfg(unix)]
    let invalid_utf = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![0x66, 0x6f, 0x80, 0x6f])
    };
    #[cfg(windows)]
    let invalid_utf = {
        use std::os::windows::prelude::*;
        OsString::from_wide(&[0x0066, 0x006f, 0xD800, 0x006f])
    };
    let assert = test_env
        .jj_cmd(test_env.env_root(), &[])
        .args(&[invalid_utf])
        .assert()
        .code(2);
    let stderr = get_stderr_string(&assert);
    insta::assert_snapshot!(stderr, @r###"
    Error: Non-utf8 argument
    "###);
}

#[test]
fn test_no_commit_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);

    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "initial").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 438471f3fbf1004298d8fb01eeb13663a051a643
    o 0000000000000000000000000000000000000000
    "###);

    // Modify the file. With --no-commit-working-copy, we still get the same commit
    // ID.
    std::fs::write(repo_path.join("file"), "modified").unwrap();
    let stdout_again = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "commit_id", "--no-commit-working-copy"],
    );
    assert_eq!(stdout_again, stdout);

    // But without --no-commit-working-copy, we get a new commit ID.
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @ fab22d1acf5bb9c5aa48cb2c3dd2132072a359ca
    o 0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_repo_arg_with_init() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["init", "-R=.", "repo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: '--repository' cannot be used with 'init'
    "###);
}

#[test]
fn test_repo_arg_with_git_clone() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["git", "clone", "-R=.", "remote"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: '--repository' cannot be used with 'git clone'
    "###);
}

#[test]
fn test_color_config() {
    let mut test_env = TestEnvironment::default();

    // Test that color is used if it's requested in the config file
    test_env.add_config(
        br#"[ui]
color="always""#,
    );
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);

    let repo_path = test_env.env_root().join("repo");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @ [1;34m230dd059e1b059aefc0da06a2e5a7dbf22362f22[0m
    o [34m0000000000000000000000000000000000000000[0m
    "###);

    // Test that NO_COLOR overrides the request for color in the config file
    test_env.add_env_var("NO_COLOR", "");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o 0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_invalid_config() {
    // Test that we get a reasonable error if the config is invalid (#55)
    let test_env = TestEnvironment::default();

    test_env.add_config(b"[section]key = value-missing-quotes");
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["init", "repo"]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Invalid config: expected newline, found an identifier at line 1 column 10 in config/config0001.toml
    "###);
}

#[test]
fn test_help() {
    // Test that global options are separated out in the help output
    let test_env = TestEnvironment::default();

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["edit", "-h"]);
    insta::assert_snapshot!(stdout.replace(".exe", ""), @r###"
    jj-edit 
    Edit the content changes in a revision

    USAGE:
        jj edit [OPTIONS]

    OPTIONS:
        -r, --revision <REVISION>    The revision to edit [default: @]

    GLOBAL OPTIONS:
            --at-operation <AT_OPERATION>    Operation to load the repo at [default: @]
        -h, --help                           Print help information, more help with --help than with -h
            --no-commit-working-copy         Don't commit the working copy
        -R, --repository <REPOSITORY>        Path to repository to operate on
    "###);
}
