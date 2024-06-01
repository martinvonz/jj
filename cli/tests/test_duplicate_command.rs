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
    test_env.jj_cmd_ok(repo_path, &["bookmark", "create", name]);
}

#[test]
fn test_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    17a00fc21654   c
    ├─╮
    │ ○  d370aee184ba   b
    ○ │  2443ea76b0b1   a
    ├─╯
    ◆  000000000000
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["duplicate", "all()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot duplicate the root commit
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "none()"]);
    insta::assert_snapshot!(stderr, @r###"
    No revisions to duplicate.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 2443ea76b0b1 as kpqxywon f5b1e687 a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○  f5b1e68729d6   a
    │ @    17a00fc21654   c
    │ ├─╮
    │ │ ○  d370aee184ba   b
    ├───╯
    │ ○  2443ea76b0b1   a
    ├─╯
    ◆  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Undid operation: b5bdbb51ab28 (2001-02-03 08:05:17) duplicate 1 commit(s)
    "#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate" /* duplicates `c` */]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 17a00fc21654 as lylxulpl ef3b0f3d c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○    ef3b0f3d1046   c
    ├─╮
    │ │ @  17a00fc21654   c
    ╭─┬─╯
    │ ○  d370aee184ba   b
    ○ │  2443ea76b0b1   a
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_duplicate_many() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["b", "d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    921dde6e55c0   e
    ├─╮
    │ ○  ebd06dba20ec   d
    │ ○  c0cb3a0b73e7   c
    ○ │  1394f625cbbd   b
    ├─╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b::"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as wqnwkozp 3b74d969 b
    Duplicated 921dde6e55c0 as mouksmqu 8348ddce e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○    8348ddcec733   e
    ├─╮
    ○ │  3b74d9691015   b
    │ │ @  921dde6e55c0   e
    │ ╭─┤
    │ ○ │  ebd06dba20ec   d
    │ ○ │  c0cb3a0b73e7   c
    ├─╯ │
    │   ○  1394f625cbbd   b
    ├───╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "###);

    // Try specifying the same commit twice directly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as nkmrtpmo 0276d3d7 b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○  0276d3d7c24d   b
    │ @    921dde6e55c0   e
    │ ├─╮
    │ │ ○  ebd06dba20ec   d
    │ │ ○  c0cb3a0b73e7   c
    ├───╯
    │ ○  1394f625cbbd   b
    ├─╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "###);

    // Try specifying the same commit twice indirectly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b::", "d::"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as xtnwkqum fa167d18 b
    Duplicated ebd06dba20ec as pqrnrkux 2181781b d
    Duplicated 921dde6e55c0 as ztxkyksq 0f7430f2 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○    0f7430f2727a   e
    ├─╮
    │ ○  2181781b4f81   d
    ○ │  fa167d18a83a   b
    │ │ @    921dde6e55c0   e
    │ │ ├─╮
    │ │ │ ○  ebd06dba20ec   d
    │ ├───╯
    │ ○ │  c0cb3a0b73e7   c
    ├─╯ │
    │   ○  1394f625cbbd   b
    ├───╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Reminder of the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    921dde6e55c0   e
    ├─╮
    │ ○  ebd06dba20ec   d
    │ ○  c0cb3a0b73e7   c
    ○ │  1394f625cbbd   b
    ├─╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "d::", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 2443ea76b0b1 as nlrtlrxv c6f7f8c4 a
    Duplicated ebd06dba20ec as plymsszl d94e4c55 d
    Duplicated 921dde6e55c0 as urrlptpw 9bd4389f e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○    9bd4389f5d47   e
    ├─╮
    │ ○  d94e4c55a68b   d
    │ │ @  921dde6e55c0   e
    ╭───┤
    │ │ ○  ebd06dba20ec   d
    │ ├─╯
    │ ○  c0cb3a0b73e7   c
    ○ │  1394f625cbbd   b
    ├─╯
    ○  2443ea76b0b1   a
    │ ○  c6f7f8c4512e   a
    ├─╯
    ◆  000000000000
    "###);

    // Check for BUG -- makes too many 'a'-s, etc.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a::"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 2443ea76b0b1 as uuuvxpvw 0fe67a05 a
    Duplicated 1394f625cbbd as nmpuuozl e13ac0ad b
    Duplicated c0cb3a0b73e7 as kzpokyyw df53fa58 c
    Duplicated ebd06dba20ec as yxrlprzz 2f2442db d
    Duplicated 921dde6e55c0 as mvkzkxrl ee8fe64e e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○    ee8fe64ed254   e
    ├─╮
    │ ○  2f2442db08eb   d
    │ ○  df53fa589286   c
    ○ │  e13ac0adabdf   b
    ├─╯
    ○  0fe67a05989e   a
    │ @    921dde6e55c0   e
    │ ├─╮
    │ │ ○  ebd06dba20ec   d
    │ │ ○  c0cb3a0b73e7   c
    │ ○ │  1394f625cbbd   b
    │ ├─╯
    │ ○  2443ea76b0b1   a
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_duplicate_destination() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a1", &[]);
    create_commit(&test_env, &repo_path, "a2", &["a1"]);
    create_commit(&test_env, &repo_path, "a3", &["a2"]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &[]);
    create_commit(&test_env, &repo_path, "d", &[]);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  f7550bb42c6f   d
    │ ○  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);

    // Duplicate a single commit onto a single destination.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as nkmrtpmo 4587e554 a1
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  4587e554fef9   a1
    ○  b75b7aa4b90e   c
    │ @  f7550bb42c6f   d
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit onto multiple destinations.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "-d", "c", "-d", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as xtnwkqum b82e6252 a1
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    b82e62526e11   a1
    ├─╮
    │ @  f7550bb42c6f   d
    ○ │  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit onto its descendant.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "-d", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as wvuyspvk 5b3cf5a5 a1
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  5b3cf5a5cbc2   a1
    ○  17072aa2b823   a3
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  f7550bb42c6f   d
    ├─╯
    │ ○  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    ◆  000000000000
    "#);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    // Duplicate multiple commits without a direct ancestry relationship onto a
    // single destination.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as xlzxqlsl 30bff9b1 a1
    Duplicated 9a27d5939bef as vnkwvqxw c7016240 b
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  c7016240cc66   b
    │ ○  30bff9b13575   a1
    ├─╯
    ○  b75b7aa4b90e   c
    │ @  f7550bb42c6f   d
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship onto
    // multiple destinations.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b", "-d", "c", "-d", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as oupztwtk 8fd646d0 a1
    Duplicated 9a27d5939bef as yxsqzptr 7d7269ca b
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    7d7269ca124a   b
    ├─╮
    │ │ ○  8fd646d085a9   a1
    ╭─┬─╯
    │ @  f7550bb42c6f   d
    ○ │  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship onto a
    // single destination.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as wtszoswq 58411bed a1
    Duplicated 17072aa2b823 as qmykwtmu 86842c96 a3
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  86842c96d8c8   a3
    ○  58411bed3598   a1
    ○  b75b7aa4b90e   c
    │ @  f7550bb42c6f   d
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship onto
    // multiple destinations.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "-d", "c", "-d", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as rkoyqlrv 57d65d68 a1
    Duplicated 17072aa2b823 as zxvrqtmq 144cd2f3 a3
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  144cd2f3a5ab   a3
    ○    57d65d688a47   a1
    ├─╮
    │ @  f7550bb42c6f   d
    ○ │  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
}

#[test]
fn test_duplicate_insert_after() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a1", &[]);
    create_commit(&test_env, &repo_path, "a2", &["a1"]);
    create_commit(&test_env, &repo_path, "a3", &["a2"]);
    create_commit(&test_env, &repo_path, "a4", &["a3"]);
    create_commit(&test_env, &repo_path, "b1", &[]);
    create_commit(&test_env, &repo_path, "b2", &["b1"]);
    create_commit(&test_env, &repo_path, "c1", &[]);
    create_commit(&test_env, &repo_path, "c2", &["c1"]);
    create_commit(&test_env, &repo_path, "d1", &[]);
    create_commit(&test_env, &repo_path, "d2", &["d1"]);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);

    // Duplicate a single commit after a single commit with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "--after", "b1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as pzsxstzt b34eead0 a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  a384ab7ad1f6   b2
    ○  b34eead0fdf5   a1
    ○  dcc98bc8bbea   b1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after a single ancestor commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "--after", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qmkrwlvp c167d08f a3
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  8746d17a44cb   a4
    ○  15a695f5bf13   a3
    ○  73e26c9e22e7   a2
    ○  c167d08f8d9f   a3
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after a single descendant commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "--after", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as qwyusntz 074debdf a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  3fcf9fdec8f3   a4
    ○  074debdf330b   a1
    ○  17072aa2b823   a3
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after multiple commits with no direct
    // relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "b1", "--after", "c1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as soqnvnyz 671da6dc a1
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  35ccc31b58bd   c2
    │ ○  7951d1641b4b   b2
    ├─╯
    ○    671da6dc2d2e   a1
    ├─╮
    │ ○  b27346e9a9bd   c1
    ○ │  dcc98bc8bbea   b1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after multiple commits including an ancestor.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--after", "a2", "--after", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as nsrwusvy 727c43ec a3
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  5ae709b39efb   a4
    ○  ecb0aa61feab   a3
    ○    727c43ec8eaa   a3
    ├─╮
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after multiple commits including a descendant.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "a3", "--after", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as xpnwykqz 6944eeac a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  4fa1dfb1735f   a4
    ○    6944eeac206a   a1
    ├─╮
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ○ │  17072aa2b823   a3
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after a
    // single commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--after", "c1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as sryyqqkq d3dda93b a1
    Duplicated dcc98bc8bbea as pxnqtknr 21b26c06 b1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    e9f2b664654b   c2
    ├─╮
    │ ○  21b26c06639f   b1
    ○ │  d3dda93b8e6f   a1
    ├─╯
    ○  b27346e9a9bd   c1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after a
    // single commit which is an ancestor of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "b1", "--after", "a2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as pyoswmwk 0d11d466 a3
    Duplicated dcc98bc8bbea as yqnpwwmq f18498f2 b1
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  5b30b2d24181   a4
    ○    2725567328bd   a3
    ├─╮
    │ ○  f18498f24737   b1
    ○ │  0d11d4667aa9   a3
    ├─╯
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after a
    // single commit which is a descendant of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--after", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as tpmlxquz b7458ffe a1
    Duplicated dcc98bc8bbea as uukzylyy 7366036f b1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    b19d9559f21a   a4
    ├─╮
    │ ○  7366036f148d   b1
    ○ │  b7458ffedb08   a1
    ├─╯
    ○  17072aa2b823   a3
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after
    // multiple commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "c1", "--after", "d1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as knltnxnu a276dada a1
    Duplicated dcc98bc8bbea as krtqozmx aa76b8a7 b1
    Rebased 2 commits onto duplicated commits
    Working copy now at: nmzmmopx 0ad9462c d2 | d2
    Parent commit      : knltnxnu a276dada a1
    Parent commit      : krtqozmx aa76b8a7 b1
    Added 2 files, modified 0 files, removed 1 files
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    0ad9462c535b   d2
    ├─╮
    │ │ ○  16341f32c83b   c2
    ╭─┬─╯
    │ ○    aa76b8a78db1   b1
    │ ├─╮
    ○ │ │  a276dadabfc1   a1
    ╰─┬─╮
      │ ○  0f21c5e185c5   d1
      ○ │  b27346e9a9bd   c1
      ├─╯
    ○ │  7b44470918f4   b2
    ○ │  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after
    // multiple commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "b1", "--after", "a1", "--after", "c1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wxzmtyol ccda812e a3
    Duplicated dcc98bc8bbea as musouqkq 560e532e b1
    Rebased 4 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    c1d222b0e288   c2
    ├─╮
    │ │ ○  0a31f366f5a2   a4
    │ │ ○  06750de0d803   a3
    │ │ ○  031778a0e9f3   a2
    ╭─┬─╯
    │ ○    560e532ebd75   b1
    │ ├─╮
    ○ │ │  ccda812e23c4   a3
    ╰─┬─╮
      │ ○  b27346e9a9bd   c1
      ○ │  9e85a474f005   a1
      ├─╯
    @ │  0cdd923e993a   d2
    ○ │  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after
    // multiple commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "a3", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as quyylypw b6a5e31d a1
    Duplicated dcc98bc8bbea as prukwozq dfe5dcad b1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    2db9fa035611   a4
    ├─╮
    │ ○    dfe5dcad355b   b1
    │ ├─╮
    ○ │ │  b6a5e31daed5   a1
    ╰─┬─╮
      │ ○  09560d60cac4   c2
      │ ○  b27346e9a9bd   c1
      ○ │  17072aa2b823   a3
      ○ │  47df67757a64   a2
      ○ │  9e85a474f005   a1
      ├─╯
    @ │  0cdd923e993a   d2
    ○ │  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after a single
    // commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "--after", "c2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as vvvtksvt 940b5139 a1
    Duplicated 17072aa2b823 as yvrnrpnw 9d985606 a3
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  9d9856065046   a3
    ○  940b51398e5d   a1
    ○  09560d60cac4   c2
    ○  b27346e9a9bd   c1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after a single
    // ancestor commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a2", "a3", "--after", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Warning: Duplicating commit 47df67757a64 as an ancestor of itself
    Duplicated 47df67757a64 as sukptuzs 4324d289 a2
    Duplicated 17072aa2b823 as rxnrppxl 47586b09 a3
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  2174f54d55a9   a4
    ○  0224bfb4fc3d   a3
    ○  22d3bdc60967   a2
    ○  47586b09a555   a3
    ○  4324d289e62c   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after a single
    // descendant commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a2", "--after", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as rwkyzntp 08e917fe a1
    Duplicated 47df67757a64 as nqtyztop a80a88f5 a2
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  d1f47b881c72   a4
    ○  a80a88f5c6d6   a2
    ○  08e917fe904c   a1
    ○  17072aa2b823   a3
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after multiple
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a3", "--after", "c2", "--after", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as nwmqwkzz 3d3385e3 a1
    Duplicated 17072aa2b823 as uwrrnrtx 3404101d a3
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  3404101d5854   a3
    ○    3d3385e379be   a1
    ├─╮
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ○ │  09560d60cac4   c2
    ○ │  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after multiple
    // commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--after", "a2", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 196bc1f0efc1 as an ancestor of itself
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wunttkrp 9d8de4c3 a3
    Duplicated 196bc1f0efc1 as puxpuzrm 71d9b4a4 a4
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  fc18e2f00060   a4
    ○  bc2303a7d63e   a3
    ○  71d9b4a48273   a4
    ○    9d8de4c3ad3e   a3
    ├─╮
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after multiple
    // commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--after", "a3", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as zwvplpop cc0bfcbe a1
    Duplicated 47df67757a64 as znsksvls 0b619bbb a2
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  5006826a3086   a4
    ○  0b619bbbe823   a2
    ○    cc0bfcbe97fe   a1
    ├─╮
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ○ │  17072aa2b823   a3
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Should error if a loop will be created.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["duplicate", "a1", "--after", "b1", "--after", "b2"],
    );
    insta::assert_snapshot!(stderr, @r#"
    Error: Refusing to create a loop: commit 7b44470918f4 would be both an ancestor and a descendant of the duplicated commits
    "#);
}

#[test]
fn test_duplicate_insert_before() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a1", &[]);
    create_commit(&test_env, &repo_path, "a2", &["a1"]);
    create_commit(&test_env, &repo_path, "a3", &["a2"]);
    create_commit(&test_env, &repo_path, "a4", &["a3"]);
    create_commit(&test_env, &repo_path, "b1", &[]);
    create_commit(&test_env, &repo_path, "b2", &["b1"]);
    create_commit(&test_env, &repo_path, "c1", &[]);
    create_commit(&test_env, &repo_path, "c2", &["c1"]);
    create_commit(&test_env, &repo_path, "d1", &[]);
    create_commit(&test_env, &repo_path, "d2", &["d1"]);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);

    // Duplicate a single commit before a single commit with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "--before", "b2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as pzsxstzt b34eead0 a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  a384ab7ad1f6   b2
    ○  b34eead0fdf5   a1
    ○  dcc98bc8bbea   b1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before a single ancestor commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "--before", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qmkrwlvp a982be78 a3
    Rebased 4 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  09981b821640   a4
    ○  7f96a38d7b7b   a3
    ○  d37b384f7ce9   a2
    ○  4a0df1f03819   a1
    ○  a982be787d28   a3
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before a single descendant commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "--before", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as qwyusntz 2b066074 a1
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  34812a9db795   a4
    ○  b42fc445deeb   a3
    ○  2b0660740e57   a1
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before multiple commits with no direct
    // relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--before", "b2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as soqnvnyz 671da6dc a1
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  35ccc31b58bd   c2
    │ ○  7951d1641b4b   b2
    ├─╯
    ○    671da6dc2d2e   a1
    ├─╮
    │ ○  b27346e9a9bd   c1
    ○ │  dcc98bc8bbea   b1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before multiple commits including an ancestor.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--before", "a2", "--before", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as nsrwusvy 851a34a3 a3
    Rebased 4 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  3a9373464406   b2
    │ ○  8774e5674831   a4
    │ ○  f3d3a1617059   a3
    │ ○  f207ecb81650   a2
    ├─╯
    ○    851a34a36354   a3
    ├─╮
    │ ○  dcc98bc8bbea   b1
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before multiple commits including a descendant.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--before", "a3", "--before", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as xpnwykqz af64c5e4 a1
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  f9f4cbe12efc   b2
    │ ○  e8057839c645   a4
    │ ○  aa3ce5a43997   a3
    ├─╯
    ○    af64c5e44fc7   a1
    ├─╮
    │ ○  dcc98bc8bbea   b1
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before a
    // single commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--before", "c1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as sryyqqkq fa625d74 a1
    Duplicated dcc98bc8bbea as pxnqtknr 2233b9a8 b1
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  cf7c4c4cc8bc   c2
    ○    6412acdac711   c1
    ├─╮
    │ ○  2233b9a87d86   b1
    ○ │  fa625d74e0ae   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before a
    // single commit which is an ancestor of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "b1", "--before", "a2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as pyoswmwk 0a102776 a3
    Duplicated dcc98bc8bbea as yqnpwwmq 529ab44a b1
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  eb4ddce3bfef   a4
    ○  b0b76f7bedf8   a3
    ○    b5fdef30de16   a2
    ├─╮
    │ ○  529ab44a81ed   b1
    ○ │  0a1027765fdd   a3
    ├─╯
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before a
    // single commit which is a descendant of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--before", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as tpmlxquz 7502d241 a1
    Duplicated dcc98bc8bbea as uukzylyy 63ba24cf b1
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  84d66cf1a667   a4
    ○    733e5aa5ee67   a3
    ├─╮
    │ ○  63ba24cf71df   b1
    ○ │  7502d2419a00   a1
    ├─╯
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before
    // multiple commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--before", "c1", "--before", "d1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as knltnxnu 056a0cb3 a1
    Duplicated dcc98bc8bbea as krtqozmx fb68a539 b1
    Rebased 4 commits onto duplicated commits
    Working copy now at: nmzmmopx 89f9b379 d2 | d2
    Parent commit      : xznxytkn 771d0e16 d1 | d1
    Added 2 files, modified 0 files, removed 0 files
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  89f9b37923a9   d2
    ○    771d0e16b40c   d1
    ├─╮
    │ │ ○  7e7653d32cf1   c2
    │ │ ○  a83b8a44f3fc   c1
    ╭─┬─╯
    │ ○  fb68a539aea7   b1
    ○ │  056a0cb391f8   a1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before
    // multiple commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "b1", "--before", "a1", "--before", "c1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wxzmtyol 4aef0293 a3
    Duplicated dcc98bc8bbea as musouqkq 4748cf83 b1
    Rebased 6 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  a86830bda155   c2
    ○    dfa992eb0c5b   c1
    ├─╮
    │ │ ○  2a975bb6fb8d   a4
    │ │ ○  bd65348afea2   a3
    │ │ ○  5aaf2e32fe6e   a2
    │ │ ○  c1841f6cb78b   a1
    ╭─┬─╯
    │ ○  4748cf83e26e   b1
    ○ │  4aef02939dcb   a3
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before
    // multiple commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--before", "a3", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as quyylypw 024440c4 a1
    Duplicated dcc98bc8bbea as prukwozq 8175fcec b1
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    7a485e3977a8   c2
    ├─╮
    │ │ ○  e5464cd6273d   a4
    │ │ ○  e7bb732c469e   a3
    ╭─┬─╯
    │ ○    8175fcec2ded   b1
    │ ├─╮
    ○ │ │  024440c4a5da   a1
    ╰─┬─╮
      │ ○  b27346e9a9bd   c1
      ○ │  47df67757a64   a2
      ○ │  9e85a474f005   a1
      ├─╯
    @ │  0cdd923e993a   d2
    ○ │  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before a single
    // commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "--before", "c2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as vvvtksvt ad5a3d82 a1
    Duplicated 17072aa2b823 as yvrnrpnw 441a2568 a3
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  756972984dac   c2
    ○  441a25683840   a3
    ○  ad5a3d824060   a1
    ○  b27346e9a9bd   c1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before a single
    // ancestor commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "--before", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Warning: Duplicating commit 9e85a474f005 as an ancestor of itself
    Duplicated 9e85a474f005 as sukptuzs ad0234a3 a1
    Duplicated 17072aa2b823 as rxnrppxl b72e2eaa a3
    Rebased 4 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  de1a87f140d9   a4
    ○  3b405d96fbfb   a3
    ○  41677a1f0572   a2
    ○  00c6a7cebcdb   a1
    ○  b72e2eaa3f7f   a3
    ○  ad0234a34661   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before a single
    // descendant commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a2", "--before", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as rwkyzntp 2fdd3c3d a1
    Duplicated 47df67757a64 as nqtyztop bddcdcd1 a2
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  13038f9969fa   a4
    ○  327c3bc13b75   a3
    ○  bddcdcd1ef61   a2
    ○  2fdd3c3dabfc   a1
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before multiple
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a3", "--before", "c2", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as nwmqwkzz aa5bda17 a1
    Duplicated 17072aa2b823 as uwrrnrtx 7a739397 a3
    Rebased 2 commits onto duplicated commits
    Working copy now at: nmzmmopx ba3800be d2 | d2
    Parent commit      : uwrrnrtx 7a739397 a3
    Added 3 files, modified 0 files, removed 1 files
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  ba3800bec255   d2
    │ ○  6052b049d679   c2
    ├─╯
    ○  7a73939747a8   a3
    ○    aa5bda171182   a1
    ├─╮
    │ ○  0f21c5e185c5   d1
    ○ │  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before multiple
    // commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--before", "a2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 196bc1f0efc1 as an ancestor of itself
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wunttkrp c7b7f78f a3
    Duplicated 196bc1f0efc1 as puxpuzrm 196c76cf a4
    Rebased 4 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  d7ea487131da   c2
    │ ○  f8d49609e8d8   a4
    │ ○  e3d75d821d33   a3
    │ ○  23d8d39dd2d1   a2
    ├─╯
    ○  196c76cf739f   a4
    ○    c7b7f78f8924   a3
    ├─╮
    │ ○  b27346e9a9bd   c1
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before multiple
    // commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--before", "a3", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as zwvplpop 26d71f93 a1
    Duplicated 47df67757a64 as znsksvls 37c5c955 a2
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  d269d405ab74   c2
    │ ○  175de6d6b816   a4
    │ ○  cdd9df354b86   a3
    ├─╯
    ○  37c5c955a90a   a2
    ○    26d71f93323b   a1
    ├─╮
    │ ○  b27346e9a9bd   c1
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Should error if a loop will be created.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["duplicate", "a1", "--before", "b1", "--before", "b2"],
    );
    insta::assert_snapshot!(stderr, @r#"
    Error: Refusing to create a loop: commit dcc98bc8bbea would be both an ancestor and a descendant of the duplicated commits
    "#);
}

#[test]
fn test_duplicate_insert_after_before() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a1", &[]);
    create_commit(&test_env, &repo_path, "a2", &["a1"]);
    create_commit(&test_env, &repo_path, "a3", &["a2"]);
    create_commit(&test_env, &repo_path, "a4", &["a3"]);
    create_commit(&test_env, &repo_path, "b1", &[]);
    create_commit(&test_env, &repo_path, "b2", &["b1"]);
    create_commit(&test_env, &repo_path, "c1", &[]);
    create_commit(&test_env, &repo_path, "c2", &["c1"]);
    create_commit(&test_env, &repo_path, "d1", &[]);
    create_commit(&test_env, &repo_path, "d2", &["d1"]);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);

    // Duplicate a single commit in between commits with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--before", "b2", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as pzsxstzt d5ebd2c8 a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    20cc68b3be82   b2
    ├─╮
    │ ○  d5ebd2c814fb   a1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ○ │  dcc98bc8bbea   b1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between ancestor commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--before", "a2", "--after", "a1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qmkrwlvp c167d08f a3
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  8746d17a44cb   a4
    ○  15a695f5bf13   a3
    ○  73e26c9e22e7   a2
    ○  c167d08f8d9f   a3
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between an ancestor commit and a commit with no
    // direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--before", "a2", "--after", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qwyusntz 0481e43c a3
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  68632a4645b3   a4
    ○  61736eaab064   a3
    ○    b8822ec79abf   a2
    ├─╮
    │ ○  0481e43c0ba7   a3
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between descendant commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "a3", "--before", "a4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as soqnvnyz 981c26cf a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  53de53f5df1d   a4
    ○  981c26cf1d8c   a1
    ○  17072aa2b823   a3
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between a descendant commit and a commit with no
    // direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "a3", "--before", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as nsrwusvy e4ec1bed a1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    0ec3be87fae7   b2
    ├─╮
    │ ○  e4ec1bed0e7c   a1
    ○ │  dcc98bc8bbea   b1
    │ │ @  0cdd923e993a   d2
    │ │ ○  0f21c5e185c5   d1
    ├───╯
    │ │ ○  09560d60cac4   c2
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ │ ○  196bc1f0efc1   a4
    │ ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between an ancestor commit and a descendant
    // commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a2", "--after", "a1", "--before", "a4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 47df67757a64 as xpnwykqz 54cc0161 a2
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    b08d6199fab9   a4
    ├─╮
    │ ○  54cc0161a5db   a2
    ○ │  17072aa2b823   a3
    ○ │  47df67757a64   a2
    ├─╯
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between
    // commits without a direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "c1", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as sryyqqkq d3dda93b a1
    Duplicated dcc98bc8bbea as pxnqtknr 21b26c06 b1
    Rebased 1 commits onto duplicated commits
    Working copy now at: nmzmmopx 16aa6cc4 d2 | d2
    Parent commit      : xznxytkn 0f21c5e1 d1 | d1
    Parent commit      : sryyqqkq d3dda93b a1
    Parent commit      : pxnqtknr 21b26c06 b1
    Added 2 files, modified 0 files, removed 0 files
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @      16aa6cc4b9ff   d2
    ├─┬─╮
    │ │ ○  21b26c06639f   b1
    │ ○ │  d3dda93b8e6f   a1
    │ ├─╯
    ○ │  0f21c5e185c5   d1
    │ │ ○  09560d60cac4   c2
    │ ├─╯
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between a
    // commit which is an ancestor of one of the duplicated commits and a commit
    // with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "b1", "--after", "a2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 17072aa2b823 as pyoswmwk 0d11d466 a3
    Duplicated dcc98bc8bbea as yqnpwwmq f18498f2 b1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○      da87b56a17e4   c2
    ├─┬─╮
    │ │ ○  f18498f24737   b1
    │ ○ │  0d11d4667aa9   a3
    │ ├─╯
    ○ │  b27346e9a9bd   c1
    │ │ @  0cdd923e993a   d2
    │ │ ○  0f21c5e185c5   d1
    ├───╯
    │ │ ○  7b44470918f4   b2
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ │ ○  196bc1f0efc1   a4
    │ │ ○  17072aa2b823   a3
    │ ├─╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between a
    // commit which is a descendant of one of the duplicated commits and a
    // commit with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "a3", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as tpmlxquz b7458ffe a1
    Duplicated dcc98bc8bbea as uukzylyy 7366036f b1
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○      61237f8ed16f   c2
    ├─┬─╮
    │ │ ○  7366036f148d   b1
    │ ○ │  b7458ffedb08   a1
    │ ├─╯
    ○ │  b27346e9a9bd   c1
    │ │ @  0cdd923e993a   d2
    │ │ ○  0f21c5e185c5   d1
    ├───╯
    │ │ ○  7b44470918f4   b2
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ │ ○  196bc1f0efc1   a4
    │ ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "c1", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as knltnxnu 8d6944d2 a1
    Duplicated dcc98bc8bbea as krtqozmx b75e34da b1
    Rebased 1 commits onto duplicated commits
    Working copy now at: nmzmmopx 559d8248 d2 | d2
    Parent commit      : xznxytkn 0f21c5e1 d1 | d1
    Parent commit      : knltnxnu 8d6944d2 a1
    Parent commit      : krtqozmx b75e34da b1
    Added 2 files, modified 0 files, removed 0 files
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @      559d82485798   d2
    ├─┬─╮
    │ │ ○  b75e34daf1e8   b1
    │ ○ │  8d6944d2344d   a1
    │ ├─╯
    ○ │  0f21c5e185c5   d1
    │ │ ○  09560d60cac4   c2
    │ ├─╯
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a3", "--after", "c1", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as wxzmtyol db340447 a1
    Duplicated 17072aa2b823 as musouqkq 73e5fec0 a3
    Rebased 1 commits onto duplicated commits
    Working copy now at: nmzmmopx dfbf0b36 d2 | d2
    Parent commit      : xznxytkn 0f21c5e1 d1 | d1
    Parent commit      : musouqkq 73e5fec0 a3
    Added 3 files, modified 0 files, removed 0 files
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    dfbf0b367dee   d2
    ├─╮
    │ ○  73e5fec0d840   a3
    │ ○  db340447c78a   a1
    ○ │  0f21c5e185c5   d1
    │ │ ○  09560d60cac4   c2
    │ ├─╯
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between a commit
    // which is an ancestor of one of the duplicated commits and a commit
    // without a direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--after", "a2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 17072aa2b823 as quyylypw d4d3c907 a3
    Duplicated 196bc1f0efc1 as prukwozq 96798f1b a4
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    267f3c6f05a2   c2
    ├─╮
    │ ○  96798f1b59fc   a4
    │ ○  d4d3c9073a3b   a3
    ○ │  b27346e9a9bd   c1
    │ │ @  0cdd923e993a   d2
    │ │ ○  0f21c5e185c5   d1
    ├───╯
    │ │ ○  7b44470918f4   b2
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ │ ○  196bc1f0efc1   a4
    │ │ ○  17072aa2b823   a3
    │ ├─╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between a commit
    // which is a a descendant of one of the duplicated commits and a commit
    // with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--before", "a3", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 9e85a474f005 as vvvtksvt 940b5139 a1
    Duplicated 47df67757a64 as yvrnrpnw 72eb571c a2
    Rebased 2 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  b5ab4b26d9a2   a4
    ○    64f9306ab0d0   a3
    ├─╮
    │ ○  72eb571caee0   a2
    │ ○  940b51398e5d   a1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ○ │  47df67757a64   a2
    ○ │  9e85a474f005   a1
    ├─╯
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between descendant
    // commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--after", "a1", "--before", "a2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 196bc1f0efc1 as an ancestor of itself
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as sukptuzs 54dec05c a3
    Duplicated 196bc1f0efc1 as rxnrppxl 53c4e5dd a4
    Rebased 3 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  7668841ec9b9   a4
    ○  223fd997dec0   a3
    ○  9750bf965aff   a2
    ○  53c4e5ddca56   a4
    ○  54dec05c42f1   a3
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between ancestor
    // commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--after", "a3", "--before", "a4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as rwkyzntp 08e917fe a1
    Duplicated 47df67757a64 as nqtyztop a80a88f5 a2
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○  d1f47b881c72   a4
    ○  a80a88f5c6d6   a2
    ○  08e917fe904c   a1
    ○  17072aa2b823   a3
    ○  47df67757a64   a2
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between an ancestor
    // commit and a descendant commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a2", "a3", "--after", "a1", "--before", "a4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Duplicated 47df67757a64 as nwmqwkzz 8517eaa7 a2
    Duplicated 17072aa2b823 as uwrrnrtx 3ce18231 a3
    Rebased 1 commits onto duplicated commits
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    ○    0855137fa398   a4
    ├─╮
    │ ○  3ce182317a5b   a3
    │ ○  8517eaa73536   a2
    ○ │  17072aa2b823   a3
    ○ │  47df67757a64   a2
    ├─╯
    ○  9e85a474f005   a1
    │ @  0cdd923e993a   d2
    │ ○  0f21c5e185c5   d1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    "#);
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Should error if a loop will be created.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["duplicate", "a1", "--after", "b2", "--before", "b1"],
    );
    insta::assert_snapshot!(stderr, @r#"
    Error: Refusing to create a loop: commit 7b44470918f4 would be both an ancestor and a descendant of the duplicated commits
    "#);
}

// https://github.com/martinvonz/jj/issues/1050
#[test]
fn test_undo_after_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1   a
    ◆  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 2443ea76b0b1 as mzvwutvl f5cefcbb a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ○  f5cefcbb65a4   a
    │ @  2443ea76b0b1   a
    ├─╯
    ◆  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Undid operation: e3dbefa46ed5 (2001-02-03 08:05:11) duplicate 1 commit(s)
    "#);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1   a
    ◆  000000000000
    "###);
}

// https://github.com/martinvonz/jj/issues/694
#[test]
fn test_rebase_duplicates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    @  7e4fbf4f2759   c @ 2001-02-03 04:05:13.000 +07:00
    ○  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ○  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◆  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 7e4fbf4f2759 as yostqsxw 0ac2063b c
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 7e4fbf4f2759 as znkkpsqq ce5f4eeb c
    "###);
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    ○  ce5f4eeb69d1   c @ 2001-02-03 04:05:16.000 +07:00
    │ ○  0ac2063b1bee   c @ 2001-02-03 04:05:15.000 +07:00
    ├─╯
    │ @  7e4fbf4f2759   c @ 2001-02-03 04:05:13.000 +07:00
    ├─╯
    ○  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ○  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◆  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "b", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 4 commits
    Working copy now at: royxmykx ed671a3c c | c
    Parent commit      : zsuskuln 4c6f1569 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // Some of the duplicate commits' timestamps were changed a little to make them
    // have distinct commit ids.
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    ○  b86e9f27d085   c @ 2001-02-03 04:05:16.000 +07:00
    │ ○  8033590fe04d   c @ 2001-02-03 04:05:17.000 +07:00
    ├─╯
    │ @  ed671a3cbf35   c @ 2001-02-03 04:05:18.000 +07:00
    ├─╯
    ○  4c6f1569e2a9   b @ 2001-02-03 04:05:18.000 +07:00
    │ ○  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ├─╯
    ◆  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"commit_id.short() ++ "   " ++ description.first_line()"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}

fn get_log_output_with_ts(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"
    commit_id.short() ++ "   " ++ description.first_line() ++ " @ " ++ committer.timestamp()
    "#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}
