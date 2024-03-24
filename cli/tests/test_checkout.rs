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

use crate::common::TestEnvironment;

#[test]
fn test_checkout() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "second"]);

    // Check out current commit
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["checkout", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj checkout` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj checkout` will be removed in a future version, and this will be a hard error
    Working copy now at: zsuskuln 05ce7118 (empty) (no description set)
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  05ce7118568d3007efc9163b055f9cb4a6becfde
    ◉  5c52832c3483e0ace06d047a806024984f28f1d7 second
    ◉  69542c1984c1f9d91f7c6c9c9e6941782c944bd9 first
    ◉  0000000000000000000000000000000000000000
    "###);

    // Can provide a description
    test_env.jj_cmd_ok(&repo_path, &["checkout", "@--", "-m", "my message"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  1191baaf276e3d0b96b1747e885b3a517be80d6f my message
    │ ◉  5c52832c3483e0ace06d047a806024984f28f1d7 second
    ├─╯
    ◉  69542c1984c1f9d91f7c6c9c9e6941782c944bd9 first
    ◉  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_checkout_not_single_rev() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fifth"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "root()..@"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj checkout` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj checkout` will be removed in a future version, and this will be a hard error
    Error: Revset "root()..@" resolved to more than one revision
    Hint: The revset "root()..@" resolved to these revisions:
      royxmykx 2f859371 (empty) (no description set)
      mzvwutvl 5c1afd8b (empty) fifth
      zsuskuln 009f88bf (empty) fourth
      kkmpptxz 3fa8931e (empty) third
      rlvkpnrz 5c52832c (empty) second
      ...
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "root()..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj checkout` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj checkout` will be removed in a future version, and this will be a hard error
    Error: Revset "root()..@-" resolved to more than one revision
    Hint: The revset "root()..@-" resolved to these revisions:
      mzvwutvl 5c1afd8b (empty) fifth
      zsuskuln 009f88bf (empty) fourth
      kkmpptxz 3fa8931e (empty) third
      rlvkpnrz 5c52832c (empty) second
      qpvuntsm 69542c19 (empty) first
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "@-|@--"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj checkout` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj checkout` will be removed in a future version, and this will be a hard error
    Error: Revset "@-|@--" resolved to more than one revision
    Hint: The revset "@-|@--" resolved to these revisions:
      mzvwutvl 5c1afd8b (empty) fifth
      zsuskuln 009f88bf (empty) fourth
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "none()"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj checkout` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj checkout` will be removed in a future version, and this will be a hard error
    Error: Revset "none()" didn't resolve to any revisions
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
