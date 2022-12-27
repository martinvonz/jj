// Copyright 2023 The Jujutsu Authors
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
use regex::Regex;

pub mod common;

#[test]
fn test_log_author_timestamp() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp()"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 2001-02-03 04:05:09.000 +07:00
    o 2001-02-03 04:05:07.000 +07:00
    o 1970-01-01 00:00:00.000 +00:00
    "###);
}

#[test]
fn test_log_author_timestamp_ago() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().ago()"]);
    let line_re = Regex::new(r"@|o [0-9]+ years ago").unwrap();
    assert!(
        stdout.lines().all(|x| line_re.is_match(x)),
        "expected every line to match regex"
    );
}

#[test]
fn test_log_default() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "description 1"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "my-branch"]);

    // Test default log output format
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["log"]), @r###"
    @ ffdaa62087a2 test.user@example.com 2001-02-03 04:05:09.000 +07:00 my-branch 9de54178d59d
    | description 1
    o 9a45c67d3e96 test.user@example.com 2001-02-03 04:05:08.000 +07:00 4291e264ae97
    | add a file
    o 000000000000  1970-01-01 00:00:00.000 +00:00 000000000000
      (no description set)
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    @ [1m[38;5;13mffdaa62087a2[0m[39m [1m[38;5;11mtest.user@example.com[0m[39m [1m[38;5;14m2001-02-03 04:05:09.000 +07:00[0m[39m [1m[38;5;13mmy-branch[0m[39m [1m[38;5;12m9de54178d59d[0m[39m
    | [1m[38;5;15mdescription 1[0m[39m
    o [38;5;5m9a45c67d3e96[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:08.000 +07:00[39m [38;5;4m4291e264ae97[39m
    | add a file
    o [38;5;5m000000000000[39m [38;5;3m[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [38;5;4m000000000000[39m
      (no description set)
    "###);

    // Color without graph
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;13mffdaa62087a2[0m[39m [1m[38;5;11mtest.user@example.com[0m[39m [1m[38;5;14m2001-02-03 04:05:09.000 +07:00[0m[39m [1m[38;5;13mmy-branch[0m[39m [1m[38;5;12m9de54178d59d[0m[39m
    [1m[38;5;15mdescription 1[0m[39m
    [38;5;5m9a45c67d3e96[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:08.000 +07:00[39m [38;5;4m4291e264ae97[39m
    add a file
    [38;5;5m000000000000[39m [38;5;3m[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [38;5;4m000000000000[39m
    (no description set)
    "###);
}

#[test]
fn test_log_default_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description 1"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    // No divergence
    insta::assert_snapshot!(stdout, @r###"
    @ 9a45c67d3e96 test.user@example.com 2001-02-03 04:05:08.000 +07:00 7a17d52e633c
    | description 1
    o 000000000000  1970-01-01 00:00:00.000 +00:00 000000000000
      (no description set)
    "###);

    // Create divergence
    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "description 2", "--at-operation", "@-"],
    );
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    o 9a45c67d3e96?? test.user@example.com 2001-02-03 04:05:10.000 +07:00 8979953d4c67
    | description 2
    | @ 9a45c67d3e96?? test.user@example.com 2001-02-03 04:05:08.000 +07:00 7a17d52e633c
    |/  description 1
    o 000000000000  1970-01-01 00:00:00.000 +00:00 000000000000
      (no description set)
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    o [38;5;1m9a45c67d3e96??[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:10.000 +07:00[39m [38;5;4m8979953d4c67[39m
    | description 2
    | @ [1m[38;5;9m9a45c67d3e96??[0m[39m [1m[38;5;11mtest.user@example.com[0m[39m [1m[38;5;14m2001-02-03 04:05:08.000 +07:00[0m[39m [1m[38;5;12m7a17d52e633c[0m[39m
    |/  [1m[38;5;15mdescription 1[0m[39m
    o [38;5;5m000000000000[39m [38;5;3m[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [38;5;4m000000000000[39m
      (no description set)
    "###);
}
