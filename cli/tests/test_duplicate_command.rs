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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    17a00fc21654   c
    ├─╮
    │ ○  d370aee184ba   b
    ○ │  2443ea76b0b1   a
    ├─╯
    │ ○  f5b1e68729d6   a
    ├─╯
    ◆  000000000000
    "#);

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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    17a00fc21654   c
    ├─╮
    │ │ ○  ef3b0f3d1046   c
    ╭─┬─╯
    │ ○  d370aee184ba   b
    ○ │  2443ea76b0b1   a
    ├─╯
    ◆  000000000000
    "#);
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    921dde6e55c0   e
    ├─╮
    ○ │  1394f625cbbd   b
    │ │ ○  8348ddcec733   e
    │ ╭─┤
    │ ○ │  ebd06dba20ec   d
    │ ○ │  c0cb3a0b73e7   c
    ├─╯ │
    │   ○  3b74d9691015   b
    ├───╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "#);

    // Try specifying the same commit twice directly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as nkmrtpmo 0276d3d7 b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    921dde6e55c0   e
    ├─╮
    │ ○  ebd06dba20ec   d
    │ ○  c0cb3a0b73e7   c
    ○ │  1394f625cbbd   b
    ├─╯
    │ ○  0276d3d7c24d   b
    ├─╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "#);

    // Try specifying the same commit twice indirectly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b::", "d::"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as xtnwkqum fa167d18 b
    Duplicated ebd06dba20ec as pqrnrkux 2181781b d
    Duplicated 921dde6e55c0 as ztxkyksq 0f7430f2 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    921dde6e55c0   e
    ├─╮
    │ ○  ebd06dba20ec   d
    ○ │  1394f625cbbd   b
    │ │ ○    0f7430f2727a   e
    │ │ ├─╮
    │ │ │ ○  2181781b4f81   d
    │ ├───╯
    │ ○ │  c0cb3a0b73e7   c
    ├─╯ │
    │   ○  fa167d18a83a   b
    ├───╯
    ○  2443ea76b0b1   a
    ◆  000000000000
    "#);

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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    921dde6e55c0   e
    ├─╮
    │ ○  ebd06dba20ec   d
    │ │ ○  9bd4389f5d47   e
    ╭───┤
    │ │ ○  d94e4c55a68b   d
    │ ├─╯
    │ ○  c0cb3a0b73e7   c
    ○ │  1394f625cbbd   b
    ├─╯
    ○  2443ea76b0b1   a
    │ ○  c6f7f8c4512e   a
    ├─╯
    ◆  000000000000
    "#);

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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @    921dde6e55c0   e
    ├─╮
    │ ○  ebd06dba20ec   d
    │ ○  c0cb3a0b73e7   c
    ○ │  1394f625cbbd   b
    ├─╯
    ○  2443ea76b0b1   a
    │ ○    ee8fe64ed254   e
    │ ├─╮
    │ │ ○  2f2442db08eb   d
    │ │ ○  df53fa589286   c
    │ ○ │  e13ac0adabdf   b
    │ ├─╯
    │ ○  0fe67a05989e   a
    ├─╯
    ◆  000000000000
    "#);
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
    insta::assert_snapshot!(stderr, @"Duplicated 9e85a474f005 as nkmrtpmo 2944a632 a1");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  f7550bb42c6f   d
    │ ○  2944a6324f14   a1
    │ ○  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit onto multiple destinations.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "-d", "c", "-d", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"Duplicated 9e85a474f005 as xtnwkqum 155f6a01 a1");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    ○    155f6a012334   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit onto its descendant.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "-d", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as wvuyspvk 95585bb2 (empty) a1
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  f7550bb42c6f   d
    │ ○  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  95585bb2fe05   a1
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    // Duplicate multiple commits without a direct ancestry relationship onto a
    // single destination.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "-r=a1", "-r=b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as xlzxqlsl da0996fd a1
    Duplicated 9a27d5939bef as vnkwvqxw 0af91ca8 b
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  f7550bb42c6f   d
    │ ○  0af91ca82d9c   b
    │ │ ○  da0996fda8ce   a1
    │ ├─╯
    │ ○  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship onto
    // multiple destinations.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "-r=a1", "b", "-d", "c", "-d", "d"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as oupztwtk 2f519daa a1
    Duplicated 9a27d5939bef as yxsqzptr c219a744 b
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    ○    c219a744e19c   b
    ├─╮
    │ │ ○  2f519daab24d   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship onto a
    // single destination.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "-d", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as wtszoswq 806f2b56 a1
    Duplicated 17072aa2b823 as qmykwtmu 161ce874 a3
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  f7550bb42c6f   d
    │ ○  161ce87408d5   a3
    │ ○  806f2b56207d   a1
    │ ○  b75b7aa4b90e   c
    ├─╯
    │ ○  9a27d5939bef   b
    ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship onto
    // multiple destinations.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "-d", "c", "-d", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as rkoyqlrv 02cbff23 a1
    Duplicated 17072aa2b823 as zxvrqtmq ddcfb95f a3
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    ○  ddcfb95ff7d8   a3
    ○    02cbff23a61d   a1
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
    ");
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
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as pzsxstzt b71e23da a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  af12531fa2dc   b2
    │ ○  b71e23da3559   a1
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after a single ancestor commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "--after", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qmkrwlvp fd3c891b a3
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  027d38df36fa   a4
    │ ○  6cb0f5884a35   a3
    │ ○  80e3e40b66f0   a2
    │ ○  fd3c891b8b97   a3
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after a single descendant commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "--after", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as qwyusntz a4d0b771 (empty) a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  9fe3808a9067   a4
    │ ○  a4d0b7715767   a1
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after multiple commits with no direct
    // relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "b1", "--after", "c1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as soqnvnyz 3449bde2 a1
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  c997a412ac93   c2
    │ │ ○  e570747744ed   b2
    │ ├─╯
    │ ○    3449bde20037   a1
    │ ├─╮
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after multiple commits including an ancestor.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--after", "a2", "--after", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as nsrwusvy 48764702 a3
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  aead471d6dc8   a4
    │ ○  07fb2a10b5de   a3
    │ ○    48764702c97c   a3
    │ ├─╮
    │ │ ○  7b44470918f4   b2
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit after multiple commits including a descendant.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "a3", "--after", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as xpnwykqz 43bcb4dc (empty) a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  92782f7d24fe   a4
    │ ○    43bcb4dc97f4   a1
    │ ├─╮
    │ │ ○  7b44470918f4   b2
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after a
    // single commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--after", "c1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as sryyqqkq 44f57f24 a1
    Duplicated dcc98bc8bbea as pxnqtknr bcee4b60 b1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○    215600d39fed   c2
    │ ├─╮
    │ │ ○  bcee4b6058e4   b1
    │ ○ │  44f57f247bf2   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after a
    // single commit which is an ancestor of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "b1", "--after", "a2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as pyoswmwk 0d11d466 a3
    Duplicated dcc98bc8bbea as yqnpwwmq c32d1ccc b1
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  955959f7bb42   a4
    │ ○    7b2b1ab433f0   a3
    │ ├─╮
    │ │ ○  c32d1ccc8d5b   b1
    │ ○ │  0d11d4667aa9   a3
    │ ├─╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after a
    // single commit which is a descendant of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--after", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as tpmlxquz 213aff50 (empty) a1
    Duplicated dcc98bc8bbea as uukzylyy 67b82bab b1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○    9457bd90ac07   a4
    │ ├─╮
    │ │ ○  67b82babd5f6   b1
    │ ○ │  213aff50a82b   a1
    │ ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after
    // multiple commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "c1", "--after", "d1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as knltnxnu ad0a80e9 a1
    Duplicated dcc98bc8bbea as krtqozmx 840bbbe5 b1
    Rebased 2 commits onto duplicated commits
    Working copy now at: nmzmmopx 9eeade97 d2 | d2
    Parent commit      : knltnxnu ad0a80e9 a1
    Parent commit      : krtqozmx 840bbbe5 b1
    Added 3 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @    9eeade97a2f7   d2
    ├─╮
    │ │ ○  cd045e3862be   c2
    ╭─┬─╯
    │ ○    840bbbe57acb   b1
    │ ├─╮
    ○ │ │  ad0a80e9b011   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after
    // multiple commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "b1", "--after", "a1", "--after", "c1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wxzmtyol ade2ae32 a3
    Duplicated dcc98bc8bbea as musouqkq e1eed3f1 b1
    Rebased 4 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○    12a208423aa9   c2
    │ ├─╮
    │ │ │ ○  c804d94310fd   a4
    │ │ │ ○  e22e44ff5f22   a3
    │ │ │ ○  6ee77bdfc821   a2
    │ ╭─┬─╯
    │ │ ○    e1eed3f1c77c   b1
    │ │ ├─╮
    │ ○ │ │  ade2ae32950a   a3
    │ ╰─┬─╮
    │   │ ○  b27346e9a9bd   c1
    ├─────╯
    │   ○  9e85a474f005   a1
    ├───╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship after
    // multiple commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "a3", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as quyylypw c4820edd (empty) a1
    Duplicated dcc98bc8bbea as prukwozq 20cfd11e b1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○    2d04909f04b5   a4
    │ ├─╮
    │ │ ○    20cfd11ee3c3   b1
    │ │ ├─╮
    │ ○ │ │  c4820eddcd3c   a1
    │ ╰─┬─╮
    │   │ ○  09560d60cac4   c2
    │   │ ○  b27346e9a9bd   c1
    ├─────╯
    │   ○  17072aa2b823   a3
    │   ○  47df67757a64   a2
    │   ○  9e85a474f005   a1
    ├───╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after a single
    // commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "--after", "c2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as vvvtksvt b44d23b4 a1
    Duplicated 17072aa2b823 as yvrnrpnw ca8f08f6 a3
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  ca8f08f66c5c   a3
    │ ○  b44d23b4c98e   a1
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
    ");
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  2174f54d55a9   a4
    │ ○  0224bfb4fc3d   a3
    │ ○  22d3bdc60967   a2
    │ ○  47586b09a555   a3
    │ ○  4324d289e62c   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after a single
    // descendant commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a2", "--after", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as rwkyzntp b68b9a00 (empty) a1
    Duplicated 47df67757a64 as nqtyztop 0dd00ded (empty) a2
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  4f02390e56aa   a4
    │ ○  0dd00dedd0c5   a2
    │ ○  b68b9a0073cb   a1
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after multiple
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a3", "--after", "c2", "--after", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as nwmqwkzz eb455287 a1
    Duplicated 17072aa2b823 as uwrrnrtx 94a1bd80 a3
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    ○  94a1bd8080c6   a3
    ○    eb455287f1eb   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after multiple
    // commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--after", "a2", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 196bc1f0efc1 as an ancestor of itself
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wunttkrp 1ce432e1 a3
    Duplicated 196bc1f0efc1 as puxpuzrm 14728ee8 a4
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  5fa41821880b   a4
    │ ○  52554e3e9729   a3
    │ ○  14728ee84976   a4
    │ ○    1ce432e1b0ea   a3
    │ ├─╮
    │ │ ○  09560d60cac4   c2
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship after multiple
    // commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--after", "a3", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as zwvplpop 67dd65d3 (empty) a1
    Duplicated 47df67757a64 as znsksvls 7536fd44 (empty) a2
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  83aa2cfb2448   a4
    │ ○  7536fd4475cd   a2
    │ ○    67dd65d3d47a   a1
    │ ├─╮
    │ │ ○  09560d60cac4   c2
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as pzsxstzt b71e23da a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  af12531fa2dc   b2
    │ ○  b71e23da3559   a1
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before a single ancestor commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "--before", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qmkrwlvp 2108707c a3
    Rebased 4 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  ef93a98b9dba   a4
    │ ○  5952e93b6237   a3
    │ ○  f9baa38681ce   a2
    │ ○  3096149ab785   a1
    │ ○  2108707c8d39   a3
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before a single descendant commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "--before", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as qwyusntz 2fe2d212 (empty) a1
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  664fce416f57   a4
    │ ○  547efe815e18   a3
    │ ○  2fe2d21257c9   a1
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before multiple commits with no direct
    // relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--before", "b2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as soqnvnyz 3449bde2 a1
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  c997a412ac93   c2
    │ │ ○  e570747744ed   b2
    │ ├─╯
    │ ○    3449bde20037   a1
    │ ├─╮
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before multiple commits including an ancestor.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--before", "a2", "--before", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as nsrwusvy 8648c1c8 a3
    Rebased 4 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  1722fb59dee6   b2
    │ │ ○  cdeff7751fb6   a4
    │ │ ○  28f70dc150b8   a3
    │ │ ○  f38e6d30913d   a2
    │ ├─╯
    │ ○    8648c1c894f0   a3
    │ ├─╮
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit before multiple commits including a descendant.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--before", "a3", "--before", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as xpnwykqz 72cf8983 (empty) a1
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  d78b124079a4   b2
    │ │ ○  490d6138ef36   a4
    │ │ ○  e349d271ef64   a3
    │ ├─╯
    │ ○    72cf89838d1a   a1
    │ ├─╮
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  cf7c4c4cc8bc   c2
    │ ○    6412acdac711   c1
    │ ├─╮
    │ │ ○  2233b9a87d86   b1
    ├───╯
    │ ○  fa625d74e0ae   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before a
    // single commit which is an ancestor of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a3", "b1", "--before", "a2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as pyoswmwk cad067c7 a3
    Duplicated dcc98bc8bbea as yqnpwwmq 6675be66 b1
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  17391b843937   a4
    │ ○  23f979220309   a3
    │ ○    15a3207cfa72   a2
    │ ├─╮
    │ │ ○  6675be66b280   b1
    │ ○ │  cad067c7d304   a3
    │ ├─╯
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before a
    // single commit which is a descendant of one of the duplicated commits.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "b1", "--before", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as tpmlxquz 4d4dc78c (empty) a1
    Duplicated dcc98bc8bbea as uukzylyy a065abc9 b1
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  adb92c147726   a4
    │ ○    fb156cb07e68   a3
    │ ├─╮
    │ │ ○  a065abc9c61f   b1
    │ ○ │  4d4dc78c70a7   a1
    │ ├─╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wxzmtyol 31ca96b8 a3
    Duplicated dcc98bc8bbea as musouqkq 4748cf83 b1
    Rebased 6 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  aa431fa5a467   c2
    │ ○    f99bc6bf1b1c   c1
    │ ├─╮
    │ │ │ ○  a38ca6dc28f3   a4
    │ │ │ ○  16e3d6c1562a   a3
    │ │ │ ○  84b5c2b584d1   a2
    │ │ │ ○  cc4ae3a9a31d   a1
    │ ╭─┬─╯
    │ │ ○  4748cf83e26e   b1
    ├───╯
    │ ○  31ca96b88527   a3
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship before
    // multiple commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--before", "a3", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as quyylypw 3eefd57d (empty) a1
    Duplicated dcc98bc8bbea as prukwozq ed86e70f b1
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○    1c0d40fa21ea   c2
    │ ├─╮
    │ │ │ ○  c31979bb15d4   a4
    │ │ │ ○  8daf2e842412   a3
    │ ╭─┬─╯
    │ │ ○    ed86e70f497f   b1
    │ │ ├─╮
    │ ○ │ │  3eefd57d676b   a1
    │ ╰─┬─╮
    │   │ ○  b27346e9a9bd   c1
    ├─────╯
    │   ○  47df67757a64   a2
    │   ○  9e85a474f005   a1
    ├───╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before a single
    // commit without a direct relationship.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "--before", "c2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as vvvtksvt baee09af a1
    Duplicated 17072aa2b823 as yvrnrpnw c17818c1 a3
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  4a25ce233a30   c2
    │ ○  c17818c175df   a3
    │ ○  baee09af0f75   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before a single
    // ancestor commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a3", "--before", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Warning: Duplicating commit 9e85a474f005 as an ancestor of itself
    Duplicated 9e85a474f005 as sukptuzs ad0234a3 a1
    Duplicated 17072aa2b823 as rxnrppxl e64dcdd1 a3
    Rebased 4 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  76cbe9641be2   a4
    │ ○  140c783a30c6   a3
    │ ○  940c74f17140   a2
    │ ○  d359f7d9dfe7   a1
    │ ○  e64dcdd1d1d1   a3
    │ ○  ad0234a34661   a1
    ├─╯
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before a single
    // descendant commit.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "a1", "a2", "--before", "a3"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as rwkyzntp e614bda1 (empty) a1
    Duplicated 47df67757a64 as nqtyztop 5de52186 (empty) a2
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  585cb65f6d57   a4
    │ ○  b75dd23ffef0   a3
    │ ○  5de52186bdf3   a2
    │ ○  e614bda1f2dc   a1
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before multiple
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a3", "--before", "c2", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as nwmqwkzz 9963be9b a1
    Duplicated 17072aa2b823 as uwrrnrtx a5eee87f a3
    Rebased 2 commits onto duplicated commits
    Working copy now at: nmzmmopx 8161bbbc d2 | d2
    Parent commit      : uwrrnrtx a5eee87f a3
    Added 3 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  8161bbbc1341   d2
    │ ○  62eea4c098aa   c2
    ├─╯
    ○  a5eee87f5120   a3
    ○    9963be9be4cd   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before multiple
    // commits including an ancestor of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--before", "a2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 196bc1f0efc1 as an ancestor of itself
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as wunttkrp 11fcc721 a3
    Duplicated 196bc1f0efc1 as puxpuzrm 3a0d76b0 a4
    Rebased 4 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  c7a0da69006c   c2
    │ │ ○  8f35827d9ec9   a4
    │ │ ○  1ac63ccfda31   a3
    │ │ ○  96b02cd292f9   a2
    │ ├─╯
    │ ○  3a0d76b0e8c2   a4
    │ ○    11fcc72145cc   a3
    │ ├─╮
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  9e85a474f005   a1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship before multiple
    // commits including a descendant of one of the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--before", "a3", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as zwvplpop 311e39e4 (empty) a1
    Duplicated 47df67757a64 as znsksvls fdaa673d (empty) a2
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  f1f4e0efe9fb   c2
    │ │ ○  a5af2ec2ff05   a4
    │ │ ○  5d98ceaab6a5   a3
    │ ├─╯
    │ ○  fdaa673dff14   a2
    │ ○    311e39e4de28   a1
    │ ├─╮
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as pzsxstzt afc97ea4 a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○    41f0321a79b8   b2
    │ ├─╮
    │ │ ○  afc97ea480c1   a1
    │ │ ○  09560d60cac4   c2
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  196bc1f0efc1   a4
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between ancestor commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--before", "a2", "--after", "a1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qmkrwlvp fd3c891b a3
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  027d38df36fa   a4
    │ ○  6cb0f5884a35   a3
    │ ○  80e3e40b66f0   a2
    │ ○  fd3c891b8b97   a3
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between an ancestor commit and a commit with no
    // direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "--before", "a2", "--after", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as qwyusntz 4d69f69c a3
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  1e4a9c0c8247   a4
    │ ○  416da6f255ef   a3
    │ ○    335701a7e2f7   a2
    │ ├─╮
    │ │ ○  4d69f69ca987   a3
    │ │ ○  7b44470918f4   b2
    │ │ ○  dcc98bc8bbea   b1
    ├───╯
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between descendant commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "a3", "--before", "a4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as soqnvnyz 00811f7c (empty) a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  d6d9a67a7882   a4
    │ ○  00811f7ccdb5   a1
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate a single commit in between a descendant commit and a commit with no
    // direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "--after", "a3", "--before", "b2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as nsrwusvy 0b89e8a3 (empty) a1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○    71f4a83f7122   b2
    │ ├─╮
    │ │ ○  0b89e8a32915   a1
    │ ○ │  dcc98bc8bbea   b1
    ├─╯ │
    │ ○ │  196bc1f0efc1   a4
    │ ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○    b08d6199fab9   a4
    │ ├─╮
    │ │ ○  54cc0161a5db   a2
    │ ○ │  17072aa2b823   a3
    │ ○ │  47df67757a64   a2
    │ ├─╯
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between
    // commits without a direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "c1", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as sryyqqkq 44f57f24 a1
    Duplicated dcc98bc8bbea as pxnqtknr bcee4b60 b1
    Rebased 1 commits onto duplicated commits
    Working copy now at: nmzmmopx 6a5a099f d2 | d2
    Parent commit      : xznxytkn 0f21c5e1 d1 | d1
    Parent commit      : sryyqqkq 44f57f24 a1
    Parent commit      : pxnqtknr bcee4b60 b1
    Added 3 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @      6a5a099f8a03   d2
    ├─┬─╮
    │ │ ○  bcee4b6058e4   b1
    │ ○ │  44f57f247bf2   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between a
    // commit which is an ancestor of one of the duplicated commits and a commit
    // with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "b1", "--after", "a2", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 17072aa2b823 as pyoswmwk 0d11d466 a3
    Duplicated dcc98bc8bbea as yqnpwwmq c32d1ccc b1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○      9feaad4c40f3   c2
    │ ├─┬─╮
    │ │ │ ○  c32d1ccc8d5b   b1
    │ │ ○ │  0d11d4667aa9   a3
    │ │ ├─╯
    │ ○ │  b27346e9a9bd   c1
    ├─╯ │
    │ ○ │  7b44470918f4   b2
    │ ○ │  dcc98bc8bbea   b1
    ├─╯ │
    │ ○ │  196bc1f0efc1   a4
    │ ○ │  17072aa2b823   a3
    │ ├─╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between a
    // commit which is a descendant of one of the duplicated commits and a
    // commit with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "a3", "--before", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as tpmlxquz 213aff50 (empty) a1
    Duplicated dcc98bc8bbea as uukzylyy 67b82bab b1
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○      7c6622beae40   c2
    │ ├─┬─╮
    │ │ │ ○  67b82babd5f6   b1
    │ │ ○ │  213aff50a82b   a1
    │ │ ├─╯
    │ ○ │  b27346e9a9bd   c1
    ├─╯ │
    │ ○ │  7b44470918f4   b2
    │ ○ │  dcc98bc8bbea   b1
    ├─╯ │
    │ ○ │  196bc1f0efc1   a4
    │ ├─╯
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits without a direct ancestry relationship between
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "b1", "--after", "c1", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as knltnxnu a2d38733 a1
    Duplicated dcc98bc8bbea as krtqozmx 2512c935 b1
    Rebased 1 commits onto duplicated commits
    Working copy now at: nmzmmopx 4678ad48 d2 | d2
    Parent commit      : xznxytkn 0f21c5e1 d1 | d1
    Parent commit      : knltnxnu a2d38733 a1
    Parent commit      : krtqozmx 2512c935 b1
    Added 3 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @      4678ad489eeb   d2
    ├─┬─╮
    │ │ ○  2512c9358cb7   b1
    │ ○ │  a2d387331978   a1
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
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between
    // commits without a direct relationship to the duplicated commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a3", "--after", "c1", "--before", "d2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as wxzmtyol 893a647a a1
    Duplicated 17072aa2b823 as musouqkq fb14bc1e a3
    Rebased 1 commits onto duplicated commits
    Working copy now at: nmzmmopx 21321795 d2 | d2
    Parent commit      : xznxytkn 0f21c5e1 d1 | d1
    Parent commit      : musouqkq fb14bc1e a3
    Added 3 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @    21321795f72f   d2
    ├─╮
    │ ○  fb14bc1e2c3c   a3
    │ ○  893a647a7f64   a1
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
    ");
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○    267f3c6f05a2   c2
    │ ├─╮
    │ │ ○  96798f1b59fc   a4
    │ │ ○  d4d3c9073a3b   a3
    │ ○ │  b27346e9a9bd   c1
    ├─╯ │
    │ ○ │  7b44470918f4   b2
    │ ○ │  dcc98bc8bbea   b1
    ├─╯ │
    │ ○ │  196bc1f0efc1   a4
    │ ○ │  17072aa2b823   a3
    │ ├─╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between a commit
    // which is a a descendant of one of the duplicated commits and a commit
    // with no direct relationship.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--before", "a3", "--after", "c2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Duplicated 9e85a474f005 as vvvtksvt b44d23b4 a1
    Duplicated 47df67757a64 as yvrnrpnw 4d0d41e2 a2
    Rebased 2 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  1ed8f9907f23   a4
    │ ○    c48cf7ac619c   a3
    │ ├─╮
    │ │ ○  4d0d41e2b74e   a2
    │ │ ○  b44d23b4c98e   a1
    │ │ ○  09560d60cac4   c2
    │ │ ○  b27346e9a9bd   c1
    ├───╯
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between descendant
    // commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a3", "a4", "--after", "a1", "--before", "a2"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 196bc1f0efc1 as an ancestor of itself
    Warning: Duplicating commit 17072aa2b823 as an ancestor of itself
    Duplicated 17072aa2b823 as sukptuzs 8678104c a3
    Duplicated 196bc1f0efc1 as rxnrppxl b6580274 a4
    Rebased 3 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  795c1625854d   a4
    │ ○  c3fbe644a16b   a3
    │ ○  af75098c676a   a2
    │ ○  b6580274470b   a4
    │ ○  8678104c14af   a3
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Duplicate multiple commits with an ancestry relationship between ancestor
    // commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["duplicate", "a1", "a2", "--after", "a3", "--before", "a4"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Warning: Duplicating commit 47df67757a64 as a descendant of itself
    Warning: Duplicating commit 9e85a474f005 as a descendant of itself
    Duplicated 9e85a474f005 as rwkyzntp b68b9a00 (empty) a1
    Duplicated 47df67757a64 as nqtyztop 0dd00ded (empty) a2
    Rebased 1 commits onto duplicated commits
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○  4f02390e56aa   a4
    │ ○  0dd00dedd0c5   a2
    │ ○  b68b9a0073cb   a1
    │ ○  17072aa2b823   a3
    │ ○  47df67757a64   a2
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  0cdd923e993a   d2
    ○  0f21c5e185c5   d1
    │ ○  09560d60cac4   c2
    │ ○  b27346e9a9bd   c1
    ├─╯
    │ ○  7b44470918f4   b2
    │ ○  dcc98bc8bbea   b1
    ├─╯
    │ ○    0855137fa398   a4
    │ ├─╮
    │ │ ○  3ce182317a5b   a3
    │ │ ○  8517eaa73536   a2
    │ ○ │  17072aa2b823   a3
    │ ○ │  47df67757a64   a2
    │ ├─╯
    │ ○  9e85a474f005   a1
    ├─╯
    ◆  000000000000
    ");
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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  2443ea76b0b1   a
    │ ○  f5cefcbb65a4   a
    ├─╯
    ◆  000000000000
    "#);

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
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r#"
    @  7e4fbf4f2759   c @ 2001-02-03 04:05:13.000 +07:00
    │ ○  ce5f4eeb69d1   c @ 2001-02-03 04:05:16.000 +07:00
    ├─╯
    │ ○  0ac2063b1bee   c @ 2001-02-03 04:05:15.000 +07:00
    ├─╯
    ○  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ○  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◆  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "#);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-s", "b", "-d", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Rebased 4 commits onto destination
    Working copy now at: royxmykx ed671a3c c | c
    Parent commit      : zsuskuln 4c6f1569 b | b
    Added 0 files, modified 0 files, removed 1 files
    "#);
    // Some of the duplicate commits' timestamps were changed a little to make them
    // have distinct commit ids.
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r#"
    @  ed671a3cbf35   c @ 2001-02-03 04:05:18.000 +07:00
    │ ○  b86e9f27d085   c @ 2001-02-03 04:05:16.000 +07:00
    ├─╯
    │ ○  8033590fe04d   c @ 2001-02-03 04:05:17.000 +07:00
    ├─╯
    ○  4c6f1569e2a9   b @ 2001-02-03 04:05:18.000 +07:00
    │ ○  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ├─╯
    ◆  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "#);
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
