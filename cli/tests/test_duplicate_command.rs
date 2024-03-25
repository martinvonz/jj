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
    test_env.jj_cmd_ok(repo_path, &["branch", "create", name]);
}

#[test]
fn test_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    17a00fc21654   c
    ├─╮
    │ ◉  d370aee184ba   b
    ◉ │  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
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
    ◉  f5b1e68729d6   a
    │ @    17a00fc21654   c
    │ ├─╮
    │ │ ◉  d370aee184ba   b
    ├───╯
    │ ◉  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate" /* duplicates `c` */]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 17a00fc21654 as lylxulpl ef3b0f3d c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    ef3b0f3d1046   c
    ├─╮
    │ │ @  17a00fc21654   c
    ╭─┬─╯
    │ ◉  d370aee184ba   b
    ◉ │  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);
}

#[test]
fn test_duplicate_many() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    │ ◉  ebd06dba20ec   d
    │ ◉  c0cb3a0b73e7   c
    ◉ │  1394f625cbbd   b
    ├─╯
    ◉  2443ea76b0b1   a
    ◉  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b::"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as wqnwkozp 3b74d969 b
    Duplicated 921dde6e55c0 as mouksmqu 8348ddce e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    8348ddcec733   e
    ├─╮
    ◉ │  3b74d9691015   b
    │ │ @  921dde6e55c0   e
    │ ╭─┤
    │ ◉ │  ebd06dba20ec   d
    │ ◉ │  c0cb3a0b73e7   c
    ├─╯ │
    │   ◉  1394f625cbbd   b
    ├───╯
    ◉  2443ea76b0b1   a
    ◉  000000000000
    "###);

    // Try specifying the same commit twice directly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "b", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 1394f625cbbd as nkmrtpmo 0276d3d7 b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  0276d3d7c24d   b
    │ @    921dde6e55c0   e
    │ ├─╮
    │ │ ◉  ebd06dba20ec   d
    │ │ ◉  c0cb3a0b73e7   c
    ├───╯
    │ ◉  1394f625cbbd   b
    ├─╯
    ◉  2443ea76b0b1   a
    ◉  000000000000
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
    ◉    0f7430f2727a   e
    ├─╮
    │ ◉  2181781b4f81   d
    ◉ │  fa167d18a83a   b
    │ │ @    921dde6e55c0   e
    │ │ ├─╮
    │ │ │ ◉  ebd06dba20ec   d
    │ ├───╯
    │ ◉ │  c0cb3a0b73e7   c
    ├─╯ │
    │   ◉  1394f625cbbd   b
    ├───╯
    ◉  2443ea76b0b1   a
    ◉  000000000000
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Reminder of the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    921dde6e55c0   e
    ├─╮
    │ ◉  ebd06dba20ec   d
    │ ◉  c0cb3a0b73e7   c
    ◉ │  1394f625cbbd   b
    ├─╯
    ◉  2443ea76b0b1   a
    ◉  000000000000
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "d::", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 2443ea76b0b1 as nlrtlrxv c6f7f8c4 a
    Duplicated ebd06dba20ec as plymsszl d94e4c55 d
    Duplicated 921dde6e55c0 as urrlptpw 9bd4389f e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    9bd4389f5d47   e
    ├─╮
    │ ◉  d94e4c55a68b   d
    │ │ @  921dde6e55c0   e
    ╭───┤
    │ │ ◉  ebd06dba20ec   d
    │ ├─╯
    │ ◉  c0cb3a0b73e7   c
    ◉ │  1394f625cbbd   b
    ├─╯
    ◉  2443ea76b0b1   a
    │ ◉  c6f7f8c4512e   a
    ├─╯
    ◉  000000000000
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
    ◉    ee8fe64ed254   e
    ├─╮
    │ ◉  2f2442db08eb   d
    │ ◉  df53fa589286   c
    ◉ │  e13ac0adabdf   b
    ├─╯
    ◉  0fe67a05989e   a
    │ @    921dde6e55c0   e
    │ ├─╮
    │ │ ◉  ebd06dba20ec   d
    │ │ ◉  c0cb3a0b73e7   c
    │ ◉ │  1394f625cbbd   b
    │ ├─╯
    │ ◉  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);
}

// https://github.com/martinvonz/jj/issues/1050
#[test]
fn test_undo_after_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1   a
    ◉  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Duplicated 2443ea76b0b1 as mzvwutvl f5cefcbb a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  f5cefcbb65a4   a
    │ @  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1   a
    ◉  000000000000
    "###);
}

// https://github.com/martinvonz/jj/issues/694
#[test]
fn test_rebase_duplicates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    @  7e4fbf4f2759   c @ 2001-02-03 04:05:13.000 +07:00
    ◉  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ◉  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◉  000000000000    @ 1970-01-01 00:00:00.000 +00:00
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
    ◉  ce5f4eeb69d1   c @ 2001-02-03 04:05:16.000 +07:00
    │ ◉  0ac2063b1bee   c @ 2001-02-03 04:05:15.000 +07:00
    ├─╯
    │ @  7e4fbf4f2759   c @ 2001-02-03 04:05:13.000 +07:00
    ├─╯
    ◉  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ◉  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◉  000000000000    @ 1970-01-01 00:00:00.000 +00:00
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
    ◉  b86e9f27d085   c @ 2001-02-03 04:05:16.000 +07:00
    │ ◉  8033590fe04d   c @ 2001-02-03 04:05:17.000 +07:00
    ├─╯
    │ @  ed671a3cbf35   c @ 2001-02-03 04:05:18.000 +07:00
    ├─╯
    ◉  4c6f1569e2a9   b @ 2001-02-03 04:05:18.000 +07:00
    │ ◉  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ├─╯
    ◉  000000000000    @ 1970-01-01 00:00:00.000 +00:00
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
