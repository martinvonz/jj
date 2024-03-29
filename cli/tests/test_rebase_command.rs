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
      <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    Usage: jj rebase <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // Both -r and -s
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-r", "a", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--revisions <REVISIONS>' cannot be used with '--source <SOURCE>'

    Usage: jj rebase --revisions <REVISIONS> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // Both -b and -s
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-b", "a", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--branch <BRANCH>' cannot be used with '--source <SOURCE>'

    Usage: jj rebase --branch <BRANCH> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // Both -r and --skip-empty
    let stderr = test_env.jj_cmd_cli_error(
        &repo_path,
        &["rebase", "-r", "a", "-d", "b", "--skip-empty"],
    );
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--revisions <REVISIONS>' cannot be used with '--skip-empty'

    Usage: jj rebase --revisions <REVISIONS> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // Both -d and --after
    let stderr = test_env.jj_cmd_cli_error(
        &repo_path,
        &["rebase", "-r", "a", "-d", "b", "--after", "b"],
    );
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--destination <DESTINATION>' cannot be used with '--insert-after <INSERT_AFTER>'

    Usage: jj rebase --revisions <REVISIONS> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // -s with --after
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-s", "a", "--after", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--source <SOURCE>' cannot be used with '--insert-after <INSERT_AFTER>'

    Usage: jj rebase --source <SOURCE> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // -b with --after
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-b", "a", "--after", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--branch <BRANCH>' cannot be used with '--insert-after <INSERT_AFTER>'

    Usage: jj rebase --branch <BRANCH> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // Both -d and --before
    let stderr = test_env.jj_cmd_cli_error(
        &repo_path,
        &["rebase", "-r", "a", "-d", "b", "--before", "b"],
    );
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--destination <DESTINATION>' cannot be used with '--insert-before <INSERT_BEFORE>'

    Usage: jj rebase --revisions <REVISIONS> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // -s with --before
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-s", "a", "--before", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--source <SOURCE>' cannot be used with '--insert-before <INSERT_BEFORE>'

    Usage: jj rebase --source <SOURCE> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

    For more information, try '--help'.
    "###);

    // -b with --before
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-b", "a", "--before", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--branch <BRANCH>' cannot be used with '--insert-before <INSERT_BEFORE>'

    Usage: jj rebase --branch <BRANCH> <--destination <DESTINATION>|--insert-after <INSERT_AFTER>|--insert-before <INSERT_BEFORE>>

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
    // we test with a non-merge commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "-d", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 2 descendant commits
    Working copy now at: znkkpsqq 2668ffbe e | e
    Parent commit      : vruxwmqv 7b370c85 d | d
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e
    ◉    d
    ├─╮
    │ │ ◉  c
    ├───╯
    ◉ │  b
    ├─╯
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Now, let's try moving the merge commit. After, both parents of "d" ("b" and
    // "c") should become parents of "e".
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "d", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: znkkpsqq ed210c15 e | e
    Parent commit      : zsuskuln 1394f625 b | b
    Parent commit      : royxmykx c0cb3a0b c | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  c
    ◉ │  b
    ├─╯
    │ ◉  d
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
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: vruxwmqv a37531e8 d | d
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Parent commit      : zsuskuln d370aee1 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    d
    ├─╮
    │ ◉  b
    │ │ ◉  c
    ├───╯
    ◉ │  a
    ├─╯
    ◉
    "###);
}

#[test]
fn test_rebase_multiple_revisions() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    create_commit(&test_env, &repo_path, "d", &["a"]);
    create_commit(&test_env, &repo_path, "e", &["d"]);
    create_commit(&test_env, &repo_path, "f", &["c", "e"]);
    create_commit(&test_env, &repo_path, "g", &["f"]);
    create_commit(&test_env, &repo_path, "h", &["g"]);
    create_commit(&test_env, &repo_path, "i", &["f"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  i
    │ ◉  h
    │ ◉  g
    ├─╯
    ◉    f
    ├─╮
    │ ◉  e
    │ ◉  d
    ◉ │  c
    ◉ │  b
    ├─╯
    ◉  a
    ◉
    "###);

    // Test with two non-related non-merge commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "-r", "e", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 4 descendant commits
    Working copy now at: xznxytkn 016685dc i | i
    Parent commit      : kmkuslsw e04d3932 f | f
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  i
    │ ◉  h
    │ ◉  g
    ├─╯
    ◉    f
    ├─╮
    │ ◉  d
    ◉ │  b
    ├─╯
    │ ◉  e
    ├─╯
    │ ◉  c
    ├─╯
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Test with two related non-merge commits. Since "b" is a parent of "c", when
    // rebasing commits "b" and "c", their ancestry relationship should be
    // preserved.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "b", "-r", "c", "-d", "e"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 4 descendant commits
    Working copy now at: xznxytkn 94538385 i | i
    Parent commit      : kmkuslsw dae8d293 f | f
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  i
    │ ◉  h
    │ ◉  g
    ├─╯
    ◉    f
    ├─╮
    │ │ ◉  c
    │ │ ◉  b
    │ ├─╯
    │ ◉  e
    │ ◉  d
    ├─╯
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Test with a subgraph containing a merge commit. Since the merge commit "f"
    // was extracted, its descendants which are not part of the subgraph will
    // inherit its descendants which are not in the subtree ("c" and "d").
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "e::g", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits onto destination
    Rebased 2 descendant commits
    Working copy now at: xznxytkn 1868ded4 i | i
    Parent commit      : royxmykx 7e4fbf4f c | c
    Parent commit      : vruxwmqv 4cc44fbf d | d
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    i
    ├─╮
    │ │ ◉  h
    ╭─┬─╯
    │ ◉  d
    ◉ │  c
    ◉ │  b
    ├─╯
    │ ◉  g
    │ ◉  f
    │ ◉  e
    ├─╯
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Test with commits in a disconnected subgraph. The subgraph has the
    // relationship d->e->f->g->h, but only "d", "f" and "h" are in the set of
    // rebased commits. "d" should be a new parent of "f", and "f" should be a
    // new parent of "g".
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "d", "-r", "f", "-r", "h", "-d", "b"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn 9cfd1635 i | i
    Parent commit      : royxmykx 7e4fbf4f c | c
    Parent commit      : znkkpsqq ecf9a1d5 e | e
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    i
    ├─╮
    │ │ ◉  g
    ╭─┬─╯
    │ ◉  e
    ◉ │  c
    │ │ ◉  h
    │ │ ◉  f
    │ │ ◉  d
    ├───╯
    ◉ │  b
    ├─╯
    ◉  a
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Test rebasing a subgraph onto its descendants.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "d::e", "-d", "i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 4 descendant commits
    Working copy now at: xznxytkn 5d911e5c i | i
    Parent commit      : kmkuslsw d1bfda8c f | f
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  h
    ◉  g
    │ ◉  e
    │ ◉  d
    │ @  i
    ├─╯
    ◉    f
    ├─╮
    ◉ │  c
    ◉ │  b
    ├─╯
    ◉  a
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
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: vruxwmqv bff4a4eb merge | merge
    Parent commit      : royxmykx c84e900d b | b
    Parent commit      : zsuskuln d57db87b a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    merge
    ├─╮
    ◉ │  b
    │ │ ◉  base
    │ ├─╯
    │ ◉  a
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
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
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
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    "###);
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
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    "###);
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
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: znkkpsqq 45371aaf c | c
    Parent commit      : vruxwmqv c0a76bf4 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // The user would expect unsimplified ancestry here.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  notroot
    │ ◉  base
    ├─╯
    ◉
    "###);

    // This tests the algorithm for rebasing onto descendants. The result should
    // have unsimplified ancestry.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: znkkpsqq e28fa972 c | c
    Parent commit      : vruxwmqv 8d0eeb6a b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    │ ◉  base
    ├─╯
    ◉    b
    ├─╮
    │ ◉  a
    ├─╯
    ◉  notroot
    ◉
    "###);

    // This tests the algorithm for rebasing onto descendants. The result should
    // have unsimplified ancestry.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "base", "-d", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: znkkpsqq a9da974c c | c
    Parent commit      : vruxwmqv 0072139c b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉    b
    ├─╮
    │ │ ◉  base
    │ ├─╯
    │ ◉  a
    ├─╯
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
    Rebased 1 commits onto destination
    Rebased 2 descendant commits
    Working copy now at: znkkpsqq 7210b05e c | c
    Parent commit      : vruxwmqv da3f7511 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // In this case, it is unclear whether the user would always prefer unsimplified
    // ancestry (whether `b` should also be a direct child of the root commit).
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c
    ◉  b
    ◉  base
    ◉  notroot
    │ ◉  a
    ├─╯
    ◉
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "b", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: znkkpsqq f280545e c | c
    Parent commit      : zsuskuln 0a7fb8f6 base | base
    Parent commit      : royxmykx 86a06598 a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // The user would expect unsimplified ancestry here.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    c
    ├─╮
    │ ◉  a
    ├─╯
    ◉  base
    ◉  notroot
    │ ◉  b
    ├─╯
    ◉
    "###);

    // This tests the algorithm for rebasing onto descendants. The result should
    // have unsimplified ancestry.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: znkkpsqq c0a7cd80 c | c
    Parent commit      : zsuskuln 0a7fb8f6 base | base
    Parent commit      : royxmykx 86a06598 a | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  b
    @    c
    ├─╮
    │ ◉  a
    ├─╯
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
    Rebased 1 commits onto destination
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
fn test_rebase_revisions_after() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b1", &["a"]);
    create_commit(&test_env, &repo_path, "b2", &["b1"]);
    create_commit(&test_env, &repo_path, "b3", &["a"]);
    create_commit(&test_env, &repo_path, "b4", &["b3"]);
    create_commit(&test_env, &repo_path, "c", &["b2", "b4"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["c"]);
    create_commit(&test_env, &repo_path, "f", &["e"]);
    // Test the setup
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e4a00798
    ◉  e  nkmrtpmo  858693f7
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Rebasing a commit after its parents should be a no-op.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "c", "--after", "b2", "--after", "b4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commits:
      kmkuslsw cd86b3e4 c | c
      lylxulpl 7d0512e5 d | d
      nkmrtpmo 858693f7 e | e
      xznxytkn e4a00798 f | f
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e4a00798
    ◉  e  nkmrtpmo  858693f7
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    // Rebasing a commit after itself should be a no-op.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "--after", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commits:
      kmkuslsw cd86b3e4 c | c
      lylxulpl 7d0512e5 d | d
      nkmrtpmo 858693f7 e | e
      xznxytkn e4a00798 f | f
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e4a00798
    ◉  e  nkmrtpmo  858693f7
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    // Rebase a commit after another commit. "c" has parents "b2" and "b4", so its
    // children "d" and "e" should be rebased onto "b2" and "b4" respectively.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "--after", "e"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn e0e873c8 f | f
    Parent commit      : kmkuslsw 754793f3 c | c
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e0e873c8
    ◉  c  kmkuslsw  754793f3
    ◉    e  nkmrtpmo  e0d7fb63
    ├─╮
    │ │ ◉  d  lylxulpl  5e9cb58d
    ╭─┬─╯
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit after a leaf commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "e", "--after", "f"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: xznxytkn 9804b742 f | f
    Parent commit      : kmkuslsw cd86b3e4 c | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  76ac6464
    @  f  xznxytkn  9804b742
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit after a commit in a branch of a merge commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--after", "b1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 4 descendant commits
    Working copy now at: xznxytkn 80c27408 f | f
    Parent commit      : zsuskuln 072d5ae1 b1 | b1
    Added 0 files, modified 0 files, removed 5 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  cee7a197
    │ ◉  d  lylxulpl  1eb960ec
    ├─╯
    ◉    c  kmkuslsw  305a7803
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  526481b4
    @ │  f  xznxytkn  80c27408
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit after the last commit in a branch of a merge commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--after", "b2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn ebbc24b1 f | f
    Parent commit      : royxmykx 2b8e1148 b2 | b2
    Added 0 files, modified 0 files, removed 4 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  3162ac52
    │ ◉  d  lylxulpl  6f7f3b2a
    ├─╯
    ◉    c  kmkuslsw  d33f69f1
    ├─╮
    │ @  f  xznxytkn  ebbc24b1
    │ ◉  b2  royxmykx  2b8e1148
    │ ◉  b1  zsuskuln  072d5ae1
    ◉ │  b4  znkkpsqq  a52a83a4
    ◉ │  b3  vruxwmqv  523e6a8b
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit after a commit with multiple children.
    // "c" has two children "d" and "e", so the rebased commit "f" will inherit the
    // two children.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--after", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 2 descendant commits
    Working copy now at: xznxytkn 8f8c91d3 f | f
    Parent commit      : kmkuslsw cd86b3e4 c | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  03ade273
    │ ◉  d  lylxulpl  8bccbeda
    ├─╯
    @  f  xznxytkn  8f8c91d3
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit after multiple commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "f", "--after", "e", "--after", "d"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Working copy now at: xznxytkn 7784e5a0 f | f
    Parent commit      : nkmrtpmo 858693f7 e | e
    Parent commit      : lylxulpl 7d0512e5 d | d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @    f  xznxytkn  7784e5a0
    ├─╮
    │ ◉  d  lylxulpl  7d0512e5
    ◉ │  e  nkmrtpmo  858693f7
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase two unrelated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "d", "-r", "e", "--after", "a"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 6 descendant commits
    Working copy now at: xznxytkn 0b53613e f | f
    Parent commit      : kmkuslsw 193687bb c | c
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  0b53613e
    ◉    c  kmkuslsw  193687bb
    ├─╮
    │ ◉  b4  znkkpsqq  e8d0f57b
    │ ◉    b3  vruxwmqv  cb48344c
    │ ├─╮
    ◉ │ │  b2  royxmykx  535f779d
    ◉ │ │  b1  zsuskuln  693186c0
    ╰─┬─╮
      │ ◉  e  nkmrtpmo  2bb4e0b6
      ◉ │  d  lylxulpl  0b921a1c
      ├─╯
      ◉  a  rlvkpnrz  2443ea76
      ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a subgraph with merge commit and two parents, which should preserve
    // the merge.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "b2", "-r", "b4", "-r", "c", "--after", "f"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn eaf1d6b8 f | f
    Parent commit      : nkmrtpmo 0d7e4ce9 e | e
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉    d  lylxulpl  16060da9
    ├─╮
    │ │ ◉    c  kmkuslsw  ef5ead27
    │ │ ├─╮
    │ │ │ ◉  b4  znkkpsqq  9c884b94
    │ │ ◉ │  b2  royxmykx  bdfea21d
    │ │ ├─╯
    │ │ @  f  xznxytkn  eaf1d6b8
    │ │ ◉  e  nkmrtpmo  0d7e4ce9
    ╭─┬─╯
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a subgraph with four commits after one of the commits itself.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "b1::d", "--after", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 4 commits onto destination
    Rebased 2 descendant commits
    Working copy now at: xznxytkn 084e0629 f | f
    Parent commit      : nkmrtpmo 563d78c6 e | e
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  084e0629
    ◉  e  nkmrtpmo  563d78c6
    ◉  d  lylxulpl  e67ba5c9
    ◉  c  kmkuslsw  049aa109
    ◉  b2  royxmykx  7af3d6cd
    ◉    b1  zsuskuln  cd84b343
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a subgraph with disconnected commits. Since "b2" is an ancestor of
    // "e", "b2" should be a parent of "e" after the rebase.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "e", "-r", "b2", "--after", "d"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn 4fb2bb60 f | f
    Parent commit      : kmkuslsw cebde86a c | c
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  4fb2bb60
    │ ◉  e  nkmrtpmo  1ea93588
    │ ◉  b2  royxmykx  064e3bcb
    │ ◉  d  lylxulpl  b46a9d31
    ├─╯
    ◉    c  kmkuslsw  cebde86a
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Should error if a loop will be created.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["rebase", "-r", "e", "--after", "a", "--after", "b2"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to create a loop: commit 2b8e1148290f would be both an ancestor and a descendant of the rebased commits
    "###);
}

#[test]
fn test_rebase_revisions_before() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b1", &["a"]);
    create_commit(&test_env, &repo_path, "b2", &["b1"]);
    create_commit(&test_env, &repo_path, "b3", &["a"]);
    create_commit(&test_env, &repo_path, "b4", &["b3"]);
    create_commit(&test_env, &repo_path, "c", &["b2", "b4"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["c"]);
    create_commit(&test_env, &repo_path, "f", &["e"]);
    // Test the setup
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e4a00798
    ◉  e  nkmrtpmo  858693f7
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Rebasing a commit before its children should be a no-op.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "c", "--before", "d", "--before", "e"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commits:
      kmkuslsw cd86b3e4 c | c
      lylxulpl 7d0512e5 d | d
      nkmrtpmo 858693f7 e | e
      xznxytkn e4a00798 f | f
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e4a00798
    ◉  e  nkmrtpmo  858693f7
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    // Rebasing a commit before itself should be a no-op.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "--before", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Skipping rebase of commits:
      kmkuslsw cd86b3e4 c | c
      lylxulpl 7d0512e5 d | d
      nkmrtpmo 858693f7 e | e
      xznxytkn e4a00798 f | f
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  e4a00798
    ◉  e  nkmrtpmo  858693f7
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);

    // Rebasing a commit before the root commit should error.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "c", "--before", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
    "###);

    // Rebase a commit before another commit. "c" has parents "b2" and "b4", so its
    // children "d" and "e" should be rebased onto "b2" and "b4" respectively.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "c", "--before", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 8 descendant commits
    Working copy now at: xznxytkn 24335685 f | f
    Parent commit      : nkmrtpmo e9a28d4b e | e
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  24335685
    ◉    e  nkmrtpmo  e9a28d4b
    ├─╮
    │ │ ◉  d  lylxulpl  6609e9c6
    ╭─┬─╯
    │ ◉  b4  znkkpsqq  4b39b18c
    │ ◉  b3  vruxwmqv  39f79dcc
    ◉ │  b2  royxmykx  ffcf6038
    ◉ │  b1  zsuskuln  85e90af6
    ├─╯
    ◉  a  rlvkpnrz  318ea816
    ◉  c  kmkuslsw  5f99791e
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit before its parent.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--before", "e"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: xznxytkn 8e3b728a f | f
    Parent commit      : kmkuslsw cd86b3e4 c | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  41706bd9
    @  f  xznxytkn  8e3b728a
    │ ◉  d  lylxulpl  7d0512e5
    ├─╯
    ◉    c  kmkuslsw  cd86b3e4
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit before a commit in a branch of a merge commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--before", "b2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 4 descendant commits
    Working copy now at: xznxytkn 2b4f48f8 f | f
    Parent commit      : zsuskuln 072d5ae1 b1 | b1
    Added 0 files, modified 0 files, removed 5 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  7cad61fd
    │ ◉  d  lylxulpl  526b6ab6
    ├─╯
    ◉    c  kmkuslsw  445f6927
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  972bfeb7
    @ │  f  xznxytkn  2b4f48f8
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit before the first commit in a branch of a merge commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--before", "b1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 5 descendant commits
    Working copy now at: xznxytkn 488ebb95 f | f
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Added 0 files, modified 0 files, removed 6 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  9d5fa6a2
    │ ◉  d  lylxulpl  ca323694
    ├─╯
    ◉    c  kmkuslsw  07426e1a
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  55376058
    ◉ │  b1  zsuskuln  cd5b1d04
    @ │  f  xznxytkn  488ebb95
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit before a merge commit. "c" has two parents "b2" and "b4", so
    // the rebased commit "f" will have the two commits "b2" and "b4" as its
    // parents.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "f", "--before", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn aae1bc10 f | f
    Parent commit      : royxmykx 2b8e1148 b2 | b2
    Parent commit      : znkkpsqq a52a83a4 b4 | b4
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  0ea67093
    │ ◉  d  lylxulpl  c079568d
    ├─╯
    ◉  c  kmkuslsw  6371742b
    @    f  xznxytkn  aae1bc10
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  2b8e1148
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit before multiple commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "b1", "--before", "d", "--before", "e"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 5 descendant commits
    Working copy now at: xznxytkn 8268ec4d f | f
    Parent commit      : nkmrtpmo fd26fbd4 e | e
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  8268ec4d
    ◉  e  nkmrtpmo  fd26fbd4
    │ ◉  d  lylxulpl  21da64b4
    ├─╯
    ◉  b1  zsuskuln  83e9b8ac
    ◉    c  kmkuslsw  a89354fc
    ├─╮
    │ ◉  b4  znkkpsqq  a52a83a4
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b2  royxmykx  b7f03180
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit before two commits in separate branches to create a merge
    // commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "f", "--before", "b2", "--before", "b4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 5 descendant commits
    Working copy now at: xznxytkn 7ba8014f f | f
    Parent commit      : zsuskuln 072d5ae1 b1 | b1
    Parent commit      : vruxwmqv 523e6a8b b3 | b3
    Added 0 files, modified 0 files, removed 4 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    ◉  e  nkmrtpmo  9436134a
    │ ◉  d  lylxulpl  534be1ee
    ├─╯
    ◉    c  kmkuslsw  bc3ed9f8
    ├─╮
    │ ◉  b4  znkkpsqq  3e59611b
    ◉ │  b2  royxmykx  148d7e50
    ├─╯
    @    f  xznxytkn  7ba8014f
    ├─╮
    │ ◉  b3  vruxwmqv  523e6a8b
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase two unrelated commits "b2" and "b4" before a single commit "a". This
    // creates a merge commit "a" with the two parents "b2" and "b4".
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "b2", "-r", "b4", "--before", "a"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 7 descendant commits
    Working copy now at: xznxytkn fabd8dd7 f | f
    Parent commit      : nkmrtpmo b5933877 e | e
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  fabd8dd7
    ◉  e  nkmrtpmo  b5933877
    │ ◉  d  lylxulpl  6b91dd66
    ├─╯
    ◉    c  kmkuslsw  d873acf7
    ├─╮
    │ ◉  b3  vruxwmqv  1fd332d8
    ◉ │  b1  zsuskuln  8e39430f
    ├─╯
    ◉    a  rlvkpnrz  414580f5
    ├─╮
    │ ◉  b4  znkkpsqq  ae3d5bdb
    ◉ │  b2  royxmykx  a225236e
    ├─╯
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a subgraph with a merge commit and two parents.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "b2", "-r", "b4", "-r", "c", "--before", "e"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: xznxytkn cbe2be58 f | f
    Parent commit      : nkmrtpmo e31053d1 e | e
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  cbe2be58
    ◉  e  nkmrtpmo  e31053d1
    ◉    c  kmkuslsw  23155860
    ├─╮
    │ ◉    b4  znkkpsqq  e50520ad
    │ ├─╮
    ◉ │ │  b2  royxmykx  54f03b06
    ╰─┬─╮
    ◉ │ │  d  lylxulpl  0c74206e
    ╰─┬─╮
      │ ◉  b3  vruxwmqv  523e6a8b
      ◉ │  b1  zsuskuln  072d5ae1
      ├─╯
      ◉  a  rlvkpnrz  2443ea76
      ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a subgraph with disconnected commits. Since "b1" is an ancestor of
    // "e", "b1" should be a parent of "e" after the rebase.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "b1", "-r", "e", "--before", "a"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 commits onto destination
    Rebased 7 descendant commits
    Working copy now at: xznxytkn 1c48b514 f | f
    Parent commit      : kmkuslsw c0fd979a c | c
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  xznxytkn  1c48b514
    │ ◉  d  lylxulpl  4dbbc808
    ├─╯
    ◉    c  kmkuslsw  c0fd979a
    ├─╮
    │ ◉  b4  znkkpsqq  4d5c61f4
    │ ◉  b3  vruxwmqv  d5699c24
    ◉ │  b2  royxmykx  e23ab998
    ├─╯
    ◉  a  rlvkpnrz  076f0094
    ◉  e  nkmrtpmo  20d1f131
    ◉  b1  zsuskuln  11db739a
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Should error if a loop will be created.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["rebase", "-r", "e", "--before", "b2", "--before", "c"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to create a loop: commit 2b8e1148290f would be both an ancestor and a descendant of the rebased commits
    "###);
}

#[test]
fn test_rebase_revisions_after_before() {
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
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Rebase a commit after another commit and before that commit's child to
    // insert directly between the two commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "d", "--after", "e", "--before", "f"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: lylxulpl fe3d8c30 f | f
    Parent commit      : znkkpsqq cca70ee1 d | d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  fe3d8c30
    ◉  d  znkkpsqq  cca70ee1
    ◉  e  kmkuslsw  48dd9e3f
    ◉    c  vruxwmqv  c41e416e
    ├─╮
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase a commit after another commit and before that commit's descendant to
    // create a new merge commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "d", "--after", "a", "--before", "f"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: lylxulpl 22f0323c f | f
    Parent commit      : kmkuslsw 48dd9e3f e | e
    Parent commit      : znkkpsqq 61388bb6 d | d
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @    f  lylxulpl  22f0323c
    ├─╮
    │ ◉  d  znkkpsqq  61388bb6
    ◉ │  e  kmkuslsw  48dd9e3f
    ◉ │    c  vruxwmqv  c41e416e
    ├───╮
    │ │ ◉  b2  royxmykx  903ab0d6
    │ ├─╯
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // "c" has parents "b1" and "b2", so when it is rebased, its children "d" and
    // "e" should have "b1" and "b2" as parents as well. "c" is then inserted in
    // between "d" and "e", making "e" a merge commit with 3 parents "b1", "b2",
    // and "c".
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-r", "c", "--after", "d", "--before", "e"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 3 descendant commits
    Working copy now at: lylxulpl e37682c5 f | f
    Parent commit      : kmkuslsw 9bbc9e53 e | e
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @  f  lylxulpl  e37682c5
    ◉      e  kmkuslsw  9bbc9e53
    ├─┬─╮
    │ │ ◉  c  vruxwmqv  e11c7c95
    │ │ ◉  d  znkkpsqq  37869bd5
    ╭─┬─╯
    │ ◉  b2  royxmykx  903ab0d6
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Rebase multiple commits and preserve their ancestry. Apart from the heads of
    // the target commits ("d" and "e"), "f" also has commits "b1" and "b2" as
    // parents since its parents "d" and "e" were in the target set and were
    // replaced by their closest ancestors outside the target set.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "rebase", "-r", "c", "-r", "d", "-r", "e", "--after", "a", "--before", "f",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits onto destination
    Rebased 1 descendant commits
    Working copy now at: lylxulpl 868f6c61 f | f
    Parent commit      : zsuskuln 072d5ae1 b1 | b1
    Parent commit      : royxmykx 903ab0d6 b2 | b2
    Parent commit      : znkkpsqq ae6181e6 d | d
    Parent commit      : kmkuslsw a55a6779 e | e
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_long_log_output(&test_env, &repo_path), @r###"
    @        f  lylxulpl  868f6c61
    ├─┬─┬─╮
    │ │ │ ◉  e  kmkuslsw  a55a6779
    │ │ ◉ │  d  znkkpsqq  ae6181e6
    │ │ ├─╯
    │ │ ◉  c  vruxwmqv  22540859
    │ ◉ │  b2  royxmykx  903ab0d6
    │ ├─╯
    ◉ │  b1  zsuskuln  072d5ae1
    ├─╯
    ◉  a  rlvkpnrz  2443ea76
    ◉    zzzzzzzz  00000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Should error if a loop will be created.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["rebase", "-r", "e", "--after", "c", "--before", "a"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to create a loop: commit c41e416ee4cf would be both an ancestor and a descendant of the rebased commits
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
    Nothing changed.
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
    Rebased 1 descendant commits
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
