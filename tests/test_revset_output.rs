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

use common::TestEnvironment;

pub mod common;

#[test]
fn test_syntax_error() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "x &"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:4
      |
    1 | x &
      |    ^---
      |
      = expected dag_range_pre_op, range_pre_op, or primary
    "###);
}

#[test]
fn test_bad_function_call() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "all(or:nothing)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:5
      |
    1 | all(or:nothing)
      |     ^--------^
      |
      = Invalid arguments to revset function "all": Expected 0 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "parents()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:9
      |
    1 | parents()
      |         ^
      |
      = Invalid arguments to revset function "parents": Expected 1 argument
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "parents(foo, bar)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:9
      |
    1 | parents(foo, bar)
      |         ^------^
      |
      = Invalid arguments to revset function "parents": Expected 1 argument
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "heads(foo, bar)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:7
      |
    1 | heads(foo, bar)
      |       ^------^
      |
      = Invalid arguments to revset function "heads": Expected 0 or 1 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "file()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:6
      |
    1 | file()
      |      ^
      |
      = Invalid arguments to revset function "file": Expected at least 1 argument
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "file(a, not:a-string)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:9
      |
    1 | file(a, not:a-string)
      |         ^----------^
      |
      = Invalid arguments to revset function "file": Expected function argument of type string
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", r#"file(a, "../out")"#]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:9
      |
    1 | file(a, "../out")
      |         ^------^
      |
      = Invalid file pattern: Path "../out" is not in the repo
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "root:whatever()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset:  --> 1:6
      |
    1 | root:whatever()
      |      ^------^
      |
      = Revset function "whatever" doesn't exist
    "###);
}
