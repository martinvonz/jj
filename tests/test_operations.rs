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

use regex::Regex;

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_op_log() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description 0"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "op",
            "log",
            "--config-toml",
            "template-aliases.'format_time_range(x)' = 'x'",
        ],
    );
    insta::assert_snapshot!(&stdout, @r###"
    @  45108169c0f8 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    o  a99a3fd5c51e test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    o  56b94dfc38e7 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
    // Test op log with relative dates
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let regex = Regex::new(r"\d\d years").unwrap();
    insta::assert_snapshot!(regex.replace_all(&stdout, "NN years"), @r###"
    @  45108169c0f8 test-username@host.example.com NN years ago, lasted less than a microsecond
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    o  a99a3fd5c51e test-username@host.example.com NN years ago, lasted less than a microsecond
    │  add workspace 'default'
    o  56b94dfc38e7 test-username@host.example.com NN years ago, lasted less than a microsecond
       initialize repo
    "###);
    let add_workspace_id = "a99a3fd5c51e";
    let initialize_repo_id = "56b94dfc38e7";

    // Can load the repo at a specific operation ID
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, initialize_repo_id), @r###"
    o  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, add_workspace_id), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o  0000000000000000000000000000000000000000
    "###);
    // "@" resolves to the head operation
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@"), @r###"
    @  bc8f18aa6f396a93572811632313cbb5625d475d
    o  0000000000000000000000000000000000000000
    "###);
    // "@-" resolves to the parent of the head operation
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@-"), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@--"), @r###"
    o  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "@---"]), @r###"
    Error: The "@---" expression resolved to no operations
    "###);
    // "ID-" also resolves to the parent.
    insta::assert_snapshot!(
        get_log_output(&test_env, &repo_path, &format!("{add_workspace_id}-")), @r###"
    o  0000000000000000000000000000000000000000
    "###);

    // We get a reasonable message if an invalid operation ID is specified
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "foo"]), @r###"
    Error: Operation ID "foo" is not a valid hexadecimal prefix
    "###);
    // Odd length
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "123456789"]), @r###"
    Error: No operation ID matching "123456789"
    "###);
    // Even length
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "0123456789"]), @r###"
    Error: No operation ID matching "0123456789"
    "###);
    // Empty ID
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", ""]), @r###"
    Error: Operation ID "" is not a valid hexadecimal prefix
    "###);

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description 1"]);
    test_env.jj_cmd_success(
        &repo_path,
        &[
            "describe",
            "-m",
            "description 2",
            "--at-op",
            add_workspace_id,
        ],
    );
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "@-"]), @r###"
    Error: The "@-" expression resolved to more than one operation
    "###);
    test_env.jj_cmd_success(&repo_path, &["st"]);
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "@-"]), @r###"
    Error: The "@-" expression resolved to more than one operation
    "###);
}

#[test]
fn test_op_log_configurable() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"operation.hostname = "my-hostname"
        operation.username = "my-username"
        "#,
    );
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo", "--git"])
        .env_remove("JJ_OP_HOSTNAME")
        .env_remove("JJ_OP_USERNAME")
        .assert()
        .success();
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    assert!(stdout.contains("my-username@my-hostname"));
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path, op_id: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "-T", "commit_id", "--at-op", op_id, "-r", "all()"],
    )
}
