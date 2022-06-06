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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_alias_basic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        br#"[alias]
    b = ["log", "-r", "@", "-T", "branches"]
    "#,
    );
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "my-branch"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["b"]);
    insta::assert_snapshot!(stdout, @r###"
    @ my-branch
    ~
    "###);
}

#[test]
fn test_alias_calls_unknown_command() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        br#"[alias]
    foo = ["nonexistent"]
    "#,
    );
    // Should get an error about the unknown command
    test_env.jj_cmd_cli_error(&repo_path, &["foo"]);
}

#[test]
fn test_alias_cannot_override_builtin() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        br#"[alias]
    log = ["rebase"]
    "#,
    );
    // Alias should be ignored
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    o 000000000000 000000000000  1970-01-01 00:00:00.000 +00:00   
      (no description set)
    "###);
}

#[test]
fn test_alias_recursive() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        br#"[alias]
    foo = ["foo"]
    bar = ["baz"]
    baz = ["bar"]
    "#,
    );
    // Alias should not cause infinite recursion or hang
    let stderr = test_env.jj_cmd_failure(&repo_path, &["foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Recursive alias definition involving "foo"
    "###);
    // Also test with mutual recursion
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bar"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Recursive alias definition involving "bar"
    "###);
}

#[test]
fn test_alias_global_args_before_and_after() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(
        br#"[alias]
    l = ["log", "-T", "commit_id"]
    "#,
    );
    // Test the setup
    let stdout = test_env.jj_cmd_success(&repo_path, &["l"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o 0000000000000000000000000000000000000000
    "###);

    // Can pass global args before
    let stdout = test_env.jj_cmd_success(&repo_path, &["l", "--at-op", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    o 0000000000000000000000000000000000000000
    "###);
    // Can pass global args after
    let stdout = test_env.jj_cmd_success(&repo_path, &["--at-op", "@-", "l"]);
    insta::assert_snapshot!(stdout, @r###"
    o 0000000000000000000000000000000000000000
    "###);
    // Test passing global args both before and after
    let stdout = test_env.jj_cmd_success(&repo_path, &["--at-op", "abc123", "l", "--at-op", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    o 0000000000000000000000000000000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["-R", "../nonexistent", "l", "-R", "."]);
    insta::assert_snapshot!(stdout, @r###"
    @ 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o 0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_alias_global_args_in_definition() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(
        br#"[alias]
    l = ["log", "-T", "commit_id", "--at-op", "@-"]
    "#,
    );

    // The global argument in the alias is respected
    let stdout = test_env.jj_cmd_success(&repo_path, &["l"]);
    insta::assert_snapshot!(stdout, @r###"
    o 0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_alias_invalid_definition() {
    let test_env = TestEnvironment::default();

    test_env.add_config(
        br#"[alias]
    non-list = 5
    non-string-list = [7]
    "#,
    );
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["non-list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias definition for "non-list" must be a string list
    "###);
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["non-string-list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias definition for "non-string-list" must be a string list
    "###);
}
