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
fn test_op_log() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(redact_op_log(&stdout), @r###"
    @ 
    | add workspace 'default'
    o 
      initialize repo
    "###);
    let add_workspace_id = stdout[2..14].to_string();
    let initialize_repo_id = stdout.lines().nth(2).unwrap()[2..14].to_string();

    // Can load the repo at a specific operation ID
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, &initialize_repo_id), @r###"
    o 0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, &add_workspace_id), @r###"
    @ 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o 0000000000000000000000000000000000000000
    "###);
    // "@" resolves to the head operation
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@"), @r###"
    @ 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    o 0000000000000000000000000000000000000000
    "###);
    // "@-" resolves to the parent of the head operation
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@-"), @r###"
    o 0000000000000000000000000000000000000000
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
            &add_workspace_id,
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

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path, op_id: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "-T", "commit_id", "--at-op", op_id, "-r", "all()"],
    )
}

fn redact_op_log(stdout: &str) -> String {
    let mut lines = vec![];
    for line in stdout.lines() {
        if line.starts_with("@ ") || line.starts_with("o ") {
            // Redact everything -- operation ID, user, host, timestamps
            lines.push(line[..2].to_string());
        } else {
            lines.push(line.to_string());
        }
    }
    lines.join("\n")
}
