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
    @  2001-02-03 04:05:09.000 +07:00
    o  2001-02-03 04:05:07.000 +07:00
    o  1970-01-01 00:00:00.000 +00:00
    "###);
}

#[test]
fn test_log_author_timestamp_ago() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--no-graph",
            "-T",
            r#"author.timestamp().ago() "\\n""#,
        ],
    );
    let line_re = Regex::new(r"[0-9]+ years ago").unwrap();
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
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  ffdaa62087a2 test.user@example.com 2001-02-03 04:05:09.000 +07:00 my-branch 9de54178d59d
    │  (empty) description 1
    o  9a45c67d3e96 test.user@example.com 2001-02-03 04:05:08.000 +07:00 4291e264ae97
    │  add a file
    o  000000000000 1970-01-01 00:00:00.000 +00:00 000000000000
       (empty) (no description set)
    "###);

    // Test default log output format with bracket prefixes
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "--config-toml", "ui.unique-prefixes='brackets'"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  f[fdaa62087a2] test.user@example.com 2001-02-03 04:05:09.000 +07:00 my-branch 9d[e54178d59d]
    │  (empty) description 1
    o  9a[45c67d3e96] test.user@example.com 2001-02-03 04:05:08.000 +07:00 4[291e264ae97]
    │  add a file
    o  0[00000000000] 1970-01-01 00:00:00.000 +00:00 0[00000000000]
       (empty) (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--config-toml",
            "ui.unique-prefixes='brackets'",
            "--config-toml",
            "ui.log-id-preferred-length=2",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  f[f] test.user@example.com 2001-02-03 04:05:09.000 +07:00 my-branch 9d
    │  (empty) description 1
    o  9a test.user@example.com 2001-02-03 04:05:08.000 +07:00 4[2]
    │  add a file
    o  0[0] 1970-01-01 00:00:00.000 +00:00 0[0]
       (empty) (no description set)
    "###);

    // Test default log output format with styled prefixes and color
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--color=always",
            "--config-toml",
            "ui.unique-prefixes='styled'",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;13mf[38;5;8mfdaa62087a2[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 04:05:09.000 +07:00[39m [38;5;13mmy-branch[39m [38;5;12m9d[38;5;8me54178d59d[39m[0m
    │  [1m[38;5;10m(empty)[39m description 1[0m
    o  [1m[38;5;5m9a[0m[38;5;8m45c67d3e96[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:08.000 +07:00[39m [1m[38;5;4m4[0m[38;5;8m291e264ae97[39m
    │  add a file
    o  [1m[38;5;5m0[0m[38;5;8m00000000000[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [1m[38;5;4m0[0m[38;5;8m00000000000[39m
       [38;5;2m(empty)[39m (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--color=always",
            "--config-toml",
            "ui.unique-prefixes='styled'",
            "--config-toml",
            "ui.log-id-preferred-length=1",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;13mf[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 04:05:09.000 +07:00[39m [38;5;13mmy-branch[39m [38;5;12m9d[39m[0m
    │  [1m[38;5;10m(empty)[39m description 1[0m
    o  [1m[38;5;5m9a[0m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:08.000 +07:00[39m [1m[38;5;4m4[0m
    │  add a file
    o  [1m[38;5;5m0[0m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [1m[38;5;4m0[0m
       [38;5;2m(empty)[39m (no description set)
    "###);

    // Test default log output format with prefixes explicitly disabled
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "--config-toml", "ui.unique-prefixes='none'"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  ffdaa62087a2 test.user@example.com 2001-02-03 04:05:09.000 +07:00 my-branch 9de54178d59d
    │  (empty) description 1
    o  9a45c67d3e96 test.user@example.com 2001-02-03 04:05:08.000 +07:00 4291e264ae97
    │  add a file
    o  000000000000 1970-01-01 00:00:00.000 +00:00 000000000000
       (empty) (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--config-toml",
            "ui.unique-prefixes='none'",
            "--config-toml",
            "ui.log-id-preferred-length=1",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  f test.user@example.com 2001-02-03 04:05:09.000 +07:00 my-branch 9
    │  (empty) description 1
    o  9 test.user@example.com 2001-02-03 04:05:08.000 +07:00 4
    │  add a file
    o  0 1970-01-01 00:00:00.000 +00:00 0
       (empty) (no description set)
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;13mf[38;5;8mfdaa62087a2[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 04:05:09.000 +07:00[39m [38;5;13mmy-branch[39m [38;5;12m9d[38;5;8me54178d59d[39m[0m
    │  [1m[38;5;10m(empty)[39m description 1[0m
    o  [1m[38;5;5m9a[0m[38;5;8m45c67d3e96[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:08.000 +07:00[39m [1m[38;5;4m4[0m[38;5;8m291e264ae97[39m
    │  add a file
    o  [1m[38;5;5m0[0m[38;5;8m00000000000[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [1m[38;5;4m0[0m[38;5;8m00000000000[39m
       [38;5;2m(empty)[39m (no description set)
    "###);

    // Color without graph
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;13mf[38;5;8mfdaa62087a2[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 04:05:09.000 +07:00[39m [38;5;13mmy-branch[39m [38;5;12m9d[38;5;8me54178d59d[39m[0m
    [1m[38;5;10m(empty)[39m description 1[0m
    [1m[38;5;5m9a[0m[38;5;8m45c67d3e96[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:08.000 +07:00[39m [1m[38;5;4m4[0m[38;5;8m291e264ae97[39m
    add a file
    [1m[38;5;5m0[0m[38;5;8m00000000000[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [1m[38;5;4m0[0m[38;5;8m00000000000[39m
    [38;5;2m(empty)[39m (no description set)
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
    @  9a45c67d3e96 test.user@example.com 2001-02-03 04:05:08.000 +07:00 7a17d52e633c
    │  description 1
    o  000000000000 1970-01-01 00:00:00.000 +00:00 000000000000
       (empty) (no description set)
    "###);

    // Create divergence
    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "description 2", "--at-operation", "@-"],
    );
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    o  9a45c67d3e96?? test.user@example.com 2001-02-03 04:05:10.000 +07:00 8979953d4c67
    │  description 2
    │ @  9a45c67d3e96?? test.user@example.com 2001-02-03 04:05:08.000 +07:00 7a17d52e633c
    ├─╯  description 1
    o  000000000000 1970-01-01 00:00:00.000 +00:00 000000000000
       (empty) (no description set)
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    o  [1m[4m[38;5;1m9[0m[38;5;1ma45c67d3e96??[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:10.000 +07:00[39m [1m[38;5;4m8[0m[38;5;8m979953d4c67[39m
    │  description 2
    │ @  [1m[4m[38;5;1m9[24ma45c67d3e96[38;5;9m??[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 04:05:08.000 +07:00[39m [38;5;12m7[38;5;8ma17d52e633c[39m[0m
    ├─╯  [1mdescription 1[0m
    o  [1m[38;5;5m0[0m[38;5;8m00000000000[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [1m[38;5;4m0[0m[38;5;8m00000000000[39m
       [38;5;2m(empty)[39m (no description set)
    "###);
}

#[test]
fn test_log_git_head() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_success(&repo_path, &["init", "--git-repo=."]);

    test_env.jj_cmd_success(&repo_path, &["new", "-m=initial"]);
    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;13m8[38;5;8me4fac809cbb[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 04:05:09.000 +07:00[39m [38;5;12m5[38;5;8m0aaf4754c1e[39m[0m
    │  [1minitial[0m
    o  [1m[38;5;5m9[0m[38;5;8ma45c67d3e96[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 04:05:07.000 +07:00[39m [38;5;5mmaster[39m [38;5;5mHEAD@git[39m [1m[38;5;4m23[0m[38;5;8m0dd059e1b0[39m
    │  [38;5;2m(empty)[39m (no description set)
    o  [1m[38;5;5m0[0m[38;5;8m00000000000[39m [38;5;6m1970-01-01 00:00:00.000 +00:00[39m [1m[38;5;4m0[0m[38;5;8m00000000000[39m
       [38;5;2m(empty)[39m (no description set)
    "###);
}
