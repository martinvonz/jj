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

pub mod common;

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
    Working copy now at: zsuskuln 05ce7118 (empty) (no description set)
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ADDED TEST FAILURE
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
    Error: Revset "@-|@--" resolved to more than one revision
    Hint: The revset "@-|@--" resolved to these revisions:
    mzvwutvl 5c1afd8b (empty) fifth
    zsuskuln 009f88bf (empty) fourth
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "none()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "none()" didn't resolve to any revisions
    "###);
}

#[test]
fn test_checkout_conflicting_branches() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "one"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "two", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--at-op=@-",
            "branch",
            "create",
            "foo",
            "-r",
            r#"description("one")"#,
        ],
    );

    // Trigger resolution of concurrent operations
    test_env.jj_cmd_ok(&repo_path, &["st"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "foo" resolved to more than one revision
    Hint: Branch foo resolved to multiple revisions because it's conflicted.
    It resolved to these revisions:
    kkmpptxz 66c6502d foo?? | (empty) two
    qpvuntsm a9330854 foo?? | (empty) one
    Set which revision the branch points to with `jj branch set foo -r <REVISION>`.
    "###);
}

#[test]
fn test_checkout_conflicting_change_ids() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "one"]);
    test_env.jj_cmd_ok(&repo_path, &["--at-op=@-", "describe", "-m", "two"]);

    // Trigger resolution of concurrent operations
    test_env.jj_cmd_ok(&repo_path, &["st"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "qpvuntsm"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "qpvuntsm" resolved to more than one revision
    Hint: The revset "qpvuntsm" resolved to these revisions:
    qpvuntsm?? d2ae6806 (empty) two
    qpvuntsm?? a9330854 (empty) one
    Some of these commits have the same change id. Abandon one of them with `jj abandon -r <REVISION>`.
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
