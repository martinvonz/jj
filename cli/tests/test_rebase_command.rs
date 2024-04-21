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

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_ok(repo_path, &["new", "root()", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_ok(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_ok(repo_path, &["branch", "create", name]);
}

#[test]
fn test_rebase_invalid() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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

    // Both -r and --skip-empty
    let stderr = test_env.jj_cmd_cli_error(
        &repo_path,
        &["rebase", "-r", "a", "-d", "b", "--skip-empty"],
    );
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--revision <REVISION>' cannot be used with '--skip-empty'

    Usage: jj rebase --destination <DESTINATION> --revision <REVISION>

    For more information, try '--help'.
    "###);

    // Rebase onto self with -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "a"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 2443ea76b0b1 onto itself
    "###);

    // Rebase root with -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "root()", "-d", "a"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b", "c", "-d", "e"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
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
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b=e", "-b=d", "-d=b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit vruxwmqv 514fa6b2 d | d
    Rebased 1 commits
    Working copy now at: znkkpsqq 9ca2a154 e | e
    Parent commit      : zsuskuln 1394f625 b | b
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    │ ◉  d
    ├─╯
    │ ◉  c
    ├─╯
    ◉  b
    ◉  a
    ◉
    "###);

    // Same test but with more than one revision per argument
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-b=e|d", "-d=b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "e|d" resolved to more than one revision
    Hint: The revset "e|d" resolved to these revisions:
      znkkpsqq e52756c8 e | e
      vruxwmqv 514fa6b2 d | d
    Hint: Prefix the expression with 'all:' to allow any number of revisions (i.e. 'all:e|d').
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b=all:e|d", "-d=b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit vruxwmqv 514fa6b2 d | d
    Rebased 1 commits
    Working copy now at: znkkpsqq 817e3fb0 e | e
    Parent commit      : zsuskuln 1394f625 b | b
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    │ ◉  d
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b", "d", "-d", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: znkkpsqq 5f8a3db2 e | e
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Parent commit      : vruxwmqv 1677f795 d | d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  d
    │ ◉  c
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: znkkpsqq a331ac11 e | e
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Parent commit      : vruxwmqv 3d0f3644 d | d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  d
    │ ◉  c
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);
}

#[test]
fn test_rebase_single_revision() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a"]);
    create_commit(&test_env, &repo_path, "d", &["b", "c"]);
    create_commit(&test_env, &repo_path, "e", &["d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    ◉    d
    ├─╮
    │ ◉  c
    ◉ │  b
    ├─╯
    ◉  a
    ◉
    "###);

    // Descendants of the rebased commit "c" should be rebased onto parents. First
    // we test with a non-merge commit. Normally, the descendant "d" would still
    // have 2 parents afterwards: the parent of "c" -- the root commit -- and
    // "b". However, since the root commit is an ancestor of "b", we don't
    // actually want both to be parents of the same commit. So, only "b" becomes
    // a parent.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "-d", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 2 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq 147710be e | e
    Parent commit      : vruxwmqv 01e39f11 d | d
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  c
    │ @  e
    │ ◉  d
    ├─╯
    ◉  b
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Now, let's try moving the merge commit. After, both parents of "d" ("b" and
    // "c") should become parents of "e".
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "d", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq ed210c15 e | e
    Parent commit      : zsuskuln 1394f625 b | b
    Parent commit      : royxmykx c0cb3a0b c | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  d
    │ @    e
    │ ├─╮
    │ │ ◉  c
    ├───╯
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);
}

#[test]
fn test_rebase_single_revision_merge_parent() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: vruxwmqv a37531e8 d | d
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Parent commit      : zsuskuln d370aee1 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  c
    │ @  d
    ╭─┤
    │ ◉  b
    ◉ │  a
    ├─╯
    ◉
    "###);
}

#[test]
fn test_rebase_revision_onto_descendant() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[]);
    create_commit(&test_env, &repo_path, "a", &["base"]);
    create_commit(&test_env, &repo_path, "b", &["base"]);
    create_commit(&test_env, &repo_path, "merge", &["b", "a"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    merge
    ├─╮
    │ ◉  a
    ◉ │  b
    ├─╯
    ◉  base
    ◉
    "###);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Simpler example
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 3 descendant commits onto parent of rebased commit
    Working copy now at: vruxwmqv bff4a4eb merge | merge
    Parent commit      : royxmykx c84e900d b | b
    Parent commit      : zsuskuln d57db87b a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  base
    │ @  merge
    ╭─┤
    ◉ │  a
    │ ◉  b
    ├─╯
    ◉
    "###);

    // Now, let's rebase onto the descendant merge
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv b05964d1 merge | merge
    Parent commit      : royxmykx cea87a87 b | b
    Parent commit      : zsuskuln 2c5b7858 a | a
    Added 1 files, modified 0 files, removed 0 files
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "merge"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 3 descendant commits onto parent of rebased commit
    Working copy now at: vruxwmqv 986b7a49 merge | merge
    Parent commit      : royxmykx c07c677c b | b
    Parent commit      : zsuskuln abc90087 a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  base
    @    merge
    ├─╮
    │ ◉  a
    ◉ │  b
    ├─╯
    ◉
    "###);

    // TODO(ilyagr): These will be good tests for `jj rebase --insert-after` and
    // `--insert-before`, once those are implemented.
}

#[test]
fn test_rebase_multiple_destinations() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "a", "-d", "b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
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
      royxmykx fe2e8e8b c | c
      zsuskuln d370aee1 b | b
    Hint: Prefix the expression with 'all:' to allow any number of revisions (i.e. 'all:b|c').
    "###);

    // try with 'all:' and succeed
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "a", "-d", "all:b|c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    a
    ├─╮
    │ ◉  b
    @ │  c
    ├─╯
    ◉
    "###);

    // undo and do it again, but with 'ui.always-allow-large-revsets'
    let (_, _) = test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (_, _) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "rebase",
            "--config-toml=ui.always-allow-large-revsets=true",
            "-r=a",
            "-d=b|c",
        ],
    );
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

    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["rebase", "-r", "a", "-d", "b", "-d", "root()"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: The Git backend does not support creating merge commits with the root commit as one of the parents.
    "###);
}

#[test]
fn test_rebase_with_descendants() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: vruxwmqv 705832bd d | d
    Parent commit      : royxmykx 57c7246a c | c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d
    ◉    c
    ├─╮
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);

    // Rebase several subtrees at once.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=c", "-s=d", "-d=a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits
    Working copy now at: vruxwmqv 92c2bc9a d | d
    Parent commit      : rlvkpnrz 2443ea76 a | a
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

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=b", "-s=d", "-d=a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: vruxwmqv f1e71cb7 d | d
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    c
    ├─╮
    │ ◉  b
    ├─╯
    │ @  d
    ├─╯
    ◉  a
    ◉
    "###);

    // Same test as above, but with multiple commits per argument
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s=b|d", "-d=a"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "b|d" resolved to more than one revision
    Hint: The revset "b|d" resolved to these revisions:
      vruxwmqv df54a9fd d | d
      zsuskuln d370aee1 b | b
    Hint: Prefix the expression with 'all:' to allow any number of revisions (i.e. 'all:b|d').
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=all:b|d", "-d=a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: vruxwmqv d17539f7 d | d
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    c
    ├─╮
    │ ◉  b
    ├─╯
    │ @  d
    ├─╯
    ◉  a
    ◉
    "###);
}

#[test]
fn test_rebase_error_revision_does_not_exist() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "one"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b-one"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "@-", "-m", "two"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-b", "b-one", "-d", "this"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "this" doesn't exist
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-b", "this", "-d", "b-one"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "this" doesn't exist
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}

// This behavior illustrates https://github.com/martinvonz/jj/issues/2600
#[test]
fn test_rebase_with_child_and_descendant_bug_2600() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "notroot", &[]);
    create_commit(&test_env, &repo_path, "base", &["notroot"]);
    create_commit(&test_env, &repo_path, "a", &["base"]);
    create_commit(&test_env, &repo_path, "b", &["base", "a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    // ===================== rebase -s tests =================
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "base", "-d", "notroot"]);
    insta::assert_snapshot!(stdout, @"");
    // This should be a no-op
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit zsuskuln 0a7fb8f6 base | base
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "a", "-d", "base"]);
    insta::assert_snapshot!(stdout, @"");
    // This should be a no-op
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit royxmykx 86a06598 a | a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "a", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: znkkpsqq cf8ecff5 c | c
    Parent commit      : vruxwmqv 24e1a270 b | b
    "###);
    // Commit "a" should be rebased onto the root commit. Commit "b" should have
    // "base" and "a" as parents as before.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ◉ │  base
    ◉ │  notroot
    ├─╯
    ◉
    "###);

    // ===================== rebase -b tests =================
    // ====== Reminder of the setup =========
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b", "c", "-d", "base"]);
    insta::assert_snapshot!(stdout, @"");
    // The commits in roots(base..c), i.e. commit "a" should be rebased onto "base",
    // which is a no-op
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit royxmykx 86a06598 a | a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b", "c", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits
    Working copy now at: znkkpsqq 76914dcc c | c
    Parent commit      : vruxwmqv f73f03c7 b | b
    "###);
    // The commits in roots(a..c), i.e. commit "b" should be rebased onto "a",
    // which means "b" loses its "base" parent
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉  b
    ◉  a
    ◉  base
    ◉  notroot
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b", "a", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    // This should be a no-op
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit rlvkpnrz 39f28e63 notroot | notroot
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    // ===================== rebase -r tests =================
    // ====== Reminder of the setup =========
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 3 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq c1c67bb6 c | c
    Parent commit      : vruxwmqv 3b2f92d4 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // The user would expect unsimplified ancestry here.
    // ◉  base
    // │ @  c
    // │ ◉    b
    // │ ├─╮
    // │ │ ◉  a
    // │ ├─╯
    // ├─╯
    // ◉
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  base
    │ @  c
    │ ◉  b
    │ ◉  a
    │ ◉  notroot
    ├─╯
    ◉
    "###);

    // This tests the algorithm for rebasing onto descendants. The result should be
    // simplified if and only if it's simplified in the above case.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 3 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq 452e1ed1 c | c
    Parent commit      : vruxwmqv fe65c22c b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // Unsimlified ancestry would look like
    // @  c
    // │ ◉  base
    // ├─╯
    // ◉    b
    // ├─╮
    // │ ◉  a
    // ├─╯
    // ◉
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  base
    │ @  c
    ├─╯
    ◉  b
    ◉  a
    ◉  notroot
    ◉
    "###);

    // This tests the algorithm for rebasing onto descendants. The result should be
    // simplified if and only if it's simplified in the above case.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 3 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq 8da82da4 c | c
    Parent commit      : vruxwmqv 44ff6d25 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  base
    │ @  c
    │ ◉  b
    ├─╯
    ◉  a
    ◉  notroot
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    // ====== Reminder of the setup =========
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "a", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 2 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq 7210b05e c | c
    Parent commit      : vruxwmqv da3f7511 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // In this case, it is unclear whether the user would always prefer unsimplified
    // ancestry (whether `b` should also be a direct child of the root commit).
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  a
    │ @  c
    │ ◉  b
    │ ◉  base
    │ ◉  notroot
    ├─╯
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "b", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq a646b0c4 c | c
    Parent commit      : royxmykx 86a06598 a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // The user would expect unsimplified ancestry here.
    // ◉  b
    // │ @  c
    // │ ├─╮
    // │ │ ◉  a
    // │ ├─╯
    // │ ◉  base
    // ├─╯
    // ◉
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  b
    │ @  c
    │ ◉  a
    │ ◉  base
    │ ◉  notroot
    ├─╯
    ◉
    "###);

    // This tests the algorithm for rebasing onto descendants. The result should be
    // simplified if and only if it's simplified in the above case.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: znkkpsqq 82356675 c | c
    Parent commit      : royxmykx 86a06598 a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  b
    @  c
    ◉  a
    ◉  base
    ◉  notroot
    ◉
    "###);

    // In this test, the commit with weird ancestry is not rebased (neither directly
    // nor indirectly).
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: znkkpsqq 7a3bc050 c | c
    Parent commit      : royxmykx 86a06598 a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    │ ◉  b
    ╭─┤
    ◉ │  a
    ├─╯
    ◉  base
    ◉  notroot
    ◉
    "###);
}

#[test]
fn test_rebase_skip_empty() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "-m", "will become empty"]);
    test_env.jj_cmd_ok(&repo_path, &["restore", "--from=b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "already empty"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "also already empty"]);

    // Test the setup
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]), @r###"
    @  also already empty
    ◉  already empty
    ◉  will become empty
    │ ◉  b
    ├─╯
    ◉  a
    ◉
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=b", "--skip-empty"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: yostqsxw 6b74c840 (empty) also already empty
    Parent commit      : vruxwmqv 48a31526 (empty) already empty
    "###);

    // The parent commit became empty and was dropped, but the already empty commits
    // were kept
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]), @r###"
    @  also already empty
    ◉  already empty
    ◉  b
    ◉  a
    ◉
    "###);
}

#[test]
fn test_rebase_skip_if_on_destination() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b1", &["a"]);
    create_commit(&test_env, &repo_path, "b2", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["b1", "b2"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["c"]);
    create_commit(&test_env, &repo_path, "f", &["e"]);
    // Test the setup
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  88f778c5
    ◉  e  kmkuslsw  48dd9e3f
    │ ◉  d  znkkpsqq  92438fc9
    ├─╯
    ◉    c  vruxwmqv  c41e416e
    ├─╮
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-b", "d", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    // Skip rebase with -b
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commits:
      royxmykx 903ab0d6 b2 | b2
      zsuskuln 072d5ae1 b1 | b1
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  88f778c5
    ◉  e  kmkuslsw  48dd9e3f
    │ ◉  d  znkkpsqq  92438fc9
    ├─╯
    ◉    c  vruxwmqv  c41e416e
    ├─╮
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "c", "-d", "b1", "-d", "b2"]);
    insta::assert_snapshot!(stdout, @"");
    // Skip rebase with -s
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit vruxwmqv c41e416e c | c
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  88f778c5
    ◉  e  kmkuslsw  48dd9e3f
    │ ◉  d  znkkpsqq  92438fc9
    ├─╯
    ◉    c  vruxwmqv  c41e416e
    ├─╮
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "d", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    // Skip rebase with -r since commit has no children
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit znkkpsqq 92438fc9 d | d
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  88f778c5
    ◉  e  kmkuslsw  48dd9e3f
    │ ◉  d  znkkpsqq  92438fc9
    ├─╯
    ◉    c  vruxwmqv  c41e416e
    ├─╮
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "e", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    // Skip rebase of commit, but rebases children onto destination with -r
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commit kmkuslsw 48dd9e3f e | e
    Rebased 1 descendant commits onto parent of commit
    Working copy now at: lylxulpl 77cb229f f | f
    Parent commit      : vruxwmqv c41e416e c | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  77cb229f
    │ ◉  e  kmkuslsw  48dd9e3f
    ├─╯
    │ ◉  d  znkkpsqq  92438fc9
    ├─╯
    ◉    c  vruxwmqv  c41e416e
    ├─╮
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
}

fn get_long_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"description.first_line() ++ "  " ++ change_id.shortest(8) ++ "  " ++ commit_id.shortest(8)"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}
