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
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "second"]);

    // Check out current commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["checkout", "@"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 66f7f3f8235b (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 66f7f3f8235beaed90345fe93c5a86c30f4f026f (no description set)
    o 91043abe9d0385a279102350df38807f4aa053b7 second
    o 85a1e2839620cf0b354d1ccb970927d040c2a4a7 first
    o 0000000000000000000000000000000000000000 (no description set)
    "###);

    // Can provide a description
    test_env.jj_cmd_success(&repo_path, &["checkout", "@--", "-m", "my message"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 44f21384b2b12735d9477ec8b406bd4e48047c41 my message
    | o 91043abe9d0385a279102350df38807f4aa053b7 second
    |/  
    o 85a1e2839620cf0b354d1ccb970927d040c2a4a7 first
    o 0000000000000000000000000000000000000000 (no description set)
    "###);
}

#[test]
fn test_checkout_not_single_rev() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "fourth"]);
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "fifth"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "root..@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "root..@" resolved to more than one revision
    Hint: The revset resolved to these revisions:
    eb01ec3263be ()
    17b6965c1349 (fifth)
    f8381c76d1d3 (fourth)
    ca3820f77363 (third)
    91043abe9d03 (second)
    ...
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "root..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "root..@-" resolved to more than one revision
    Hint: The revset resolved to these revisions:
    17b6965c1349 (fifth)
    f8381c76d1d3 (fourth)
    ca3820f77363 (third)
    91043abe9d03 (second)
    85a1e2839620 (first)
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "@-|@--"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "@-|@--" resolved to more than one revision
    Hint: The revset resolved to these revisions:
    17b6965c1349 (fifth)
    f8381c76d1d3 (fourth)
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["checkout", "none()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "none()" didn't resolve to any revisions
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    test_env.jj_cmd_success(cwd, &["log", "-T", r#"commit_id " " description"#])
}
