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

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

#[test]
fn test_rebase_invalid() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);

    // Missing destination
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["rebase"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      --destination <DESTINATION>

    Usage: jj rebase --destination <DESTINATION>

    For more information, try '--help'.
    "###);

    // Both -r and -s
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-r", "a", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--revision <REVISION>' cannot be used with '--source <SOURCE>'

    Usage: jj rebase --destination <DESTINATION> --revision <REVISION>

    For more information, try '--help'.
    "###);

    // Both -b and -s
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-b", "a", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--branch <BRANCH>' cannot be used with '--source <SOURCE>'

    Usage: jj rebase --destination <DESTINATION> --branch <BRANCH>

    For more information, try '--help'.
    "###);

    // Rebase onto descendant with -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 2443ea76b0b1 onto descendant 1394f625cbbd
    "###);

    // Rebase onto descendant with -s
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 2443ea76b0b1 onto descendant 1394f625cbbd
    "###);
}

#[test]
fn test_rebase_branch() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    create_commit(&test_env, &repo_path, "d", &["b"]);
    create_commit(&test_env, &repo_path, "e", &["a"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    │ ◉  d
    │ │ ◉  c
    │ ├─╯
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-b", "c", "-d", "e"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  d
    │ ◉  c
    ├─╯
    ◉  b
    @  e
    ◉  a
    ◉
    "###);

    // Test rebasing multiple branches at once
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-b=e", "-b=d", "-d=b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 commits
    Working copy now at: 9ca2a154 e
    Parent commit      : 1394f625 b
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  d
    │ @  e
    ├─╯
    │ ◉  c
    ├─╯
    ◉  b
    ◉  a
    ◉
    "###);

    // Same test but with more than one revision per argument
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-b=e|d", "-d=b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "e|d" resolved to more than one revision
    Hint: The revset "e|d" resolved to these revisions:
    e52756c8 e
    514fa6b2 d
    Prefix the expression with 'all' to allow any number of revisions (i.e. 'all:e|d').
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-b=all:e|d", "-d=b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 commits
    Working copy now at: 817e3fb0 e
    Parent commit      : 1394f625 b
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  d
    │ @  e
    ├─╯
    │ ◉  c
    ├─╯
    ◉  b
    ◉  a
    ◉
    "###);
}

#[test]
fn test_rebase_branch_with_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &[]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["a", "d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  d
    │ ◉  c
    │ │ ◉  b
    ├───╯
    ◉ │  a
    ├─╯
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-b", "d", "-d", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: 391c91a7 e
    Parent commit      : 1677f795 d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    ◉  d
    ◉  c
    ◉  b
    ◉  a
    ◉
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-d", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: 040ae3a6 e
    Parent commit      : 3d0f3644 d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    ◉  d
    ◉  c
    ◉  b
    ◉  a
    ◉
    "###);
}

#[test]
fn test_rebase_single_revision() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    ◉    c
    ├─╮
    │ ◉  b
    ◉ │  a
    ├─╯
    ◉
    "###);

    // Descendants of the rebased commit "b" should be rebased onto parents. First
    // we test with a non-merge commit. Normally, the descendant "c" would still
    // have 2 parents afterwards: the parent of "b" -- the root commit -- and
    // "a". However, since the root commit is an ancestor of "a", we don't
    // actually want both to be parents of the same commit. So, only "a" becomes
    // a parent.
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 2 descendant commits onto parent of rebased commit
    Working copy now at: 7e15b97a d
    Parent commit      : 934236c8 c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    ◉  c
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_success(&repo_path, &["undo"]);

    // Now, let's try moving the merge commit. After, both parents of "c" ("a" and
    // "b") should become parents of "d".
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "c", "-d", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: bf87078f d
    Parent commit      : d370aee1 b
    Parent commit      : 2443ea76 a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    d
    ├─╮
    │ ◉  a
    ◉ │  b
    ├─╯
    │ ◉  c
    ├─╯
    ◉
    "###);
}

#[test]
fn test_rebase_single_revision_merge_parent() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    create_commit(&test_env, &repo_path, "d", &["a", "c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    d
    ├─╮
    │ ◉  c
    │ ◉  b
    ◉ │  a
    ├─╯
    ◉
    "###);

    // Descendants of the rebased commit should be rebased onto parents, and if
    // the descendant is a merge commit, it shouldn't forget its other parents.
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "c", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: c62d0789 d
    Parent commit      : d370aee1 b
    Parent commit      : 2443ea76 a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    d
    ├─╮
    ◉ │  b
    │ │ ◉  c
    │ ├─╯
    │ ◉  a
    ├─╯
    ◉
    "###);
}

#[test]
fn test_rebase_multiple_destinations() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    │ ◉  b
    ├─╯
    │ ◉  a
    ├─╯
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "a", "-d", "b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @r###""###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    a
    ├─╮
    │ @  c
    ◉ │  b
    ├─╯
    ◉
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b|c"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "b|c" resolved to more than one revision
    Hint: The revset "b|c" resolved to these revisions:
    fe2e8e8b c
    d370aee1 b
    Prefix the expression with 'all' to allow any number of revisions (i.e. 'all:b|c').
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "a", "-d", "all:b|c"]);
    insta::assert_snapshot!(stdout, @r###""###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    a
    ├─╮
    │ ◉  b
    @ │  c
    ├─╯
    ◉
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: More than one revset resolved to revision d370aee184ba
    "###);

    // Same error with 'all:' if there is overlap.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["rebase", "-r", "a", "-d", "all:b|c", "-d", "b"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: More than one revset resolved to revision d370aee184ba
    "###);

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b", "-d", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot merge with root revision
    "###);
}

#[test]
fn test_rebase_with_descendants() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    ◉    c
    ├─╮
    │ ◉  b
    ◉ │  a
    ├─╯
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: 309336ff d
    Parent commit      : 244fa794 c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    ◉  c
    ◉  b
    ◉  a
    ◉
    "###);

    // Rebase several subtrees at once.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s=c", "-s=d", "-d=a"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 commits
    Working copy now at: 92c2bc9a d
    Parent commit      : 2443ea76 a
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    │ ◉  c
    ├─╯
    ◉  a
    │ ◉  b
    ├─╯
    ◉
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    // Reminder of the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    ◉    c
    ├─╮
    │ ◉  b
    ◉ │  a
    ├─╯
    ◉
    "###);

    // `d` was a descendant of `b`, and both are moved to be direct descendants of
    // `a`. `c` remains a descendant of `b`.
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s=b", "-s=d", "-d=a"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: f1e71cb7 d
    Parent commit      : 2443ea76 a
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  c
    ◉  b
    │ @  d
    ├─╯
    ◉  a
    ◉
    "###);

    // Same test as above, but with multiple commits per argument
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s=b|d", "-d=a"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "b|d" resolved to more than one revision
    Hint: The revset "b|d" resolved to these revisions:
    df54a9fd d
    d370aee1 b
    Prefix the expression with 'all' to allow any number of revisions (i.e. 'all:b|d').
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s=all:b|d", "-d=a"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: d17539f7 d
    Parent commit      : 2443ea76 a
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  c
    ◉  b
    │ @  d
    ├─╯
    ◉  a
    ◉
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}
