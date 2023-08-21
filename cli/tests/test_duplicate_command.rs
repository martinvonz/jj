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
fn test_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    // Test the setup
    let base_operation_id = test_env.current_operation_id(&repo_path);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    17a00fc21654   c
    ├─╮
    │ ◉  d370aee184ba   b
    ◉ │  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["duplicate", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rewrite the root commit
    "###);

    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 2443ea76b0b1 as snltkkzs fc15166f a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  fc15166fed23   a
    │ @    17a00fc21654   c
    │ ├─╮
    │ │ ◉  d370aee184ba   b
    ├───╯
    │ ◉  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]), @"");
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate" /* duplicates `c` */]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 17a00fc21654 as rupkowyz fe5b2a8f c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    fe5b2a8fbc39   c
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
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["b", "d"]);
    let base_operation_id = test_env.current_operation_id(&repo_path);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
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

    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b::"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as snltkkzs 528b626f b
    Duplicated 921dde6e55c0 as vwpvxnxt 6f523f94 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    6f523f94081b   e
    ├─╮
    ◉ │  528b626f3460   b
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
    test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as rupkowyz a674a5ee b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  a674a5ee9a9f   b
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
    test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b::", "d::"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as lulsmzln 451f6198 b
    Duplicated ebd06dba20ec as pnsnxuxl 4b8937cf d
    Duplicated 921dde6e55c0 as xlvlvppo 47cf3da3 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    47cf3da3a0e2   e
    ├─╮
    │ ◉  4b8937cf8736   d
    ◉ │  451f619834bf   b
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

    test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
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
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "d::", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 2443ea76b0b1 as zxlqlsry fc054cc3 a
    Duplicated ebd06dba20ec as tkmsxyoo 506910c9 d
    Duplicated 921dde6e55c0 as sntoynvw 3a94bbb4 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    3a94bbb4d36b   e
    ├─╮
    │ ◉  506910c99bdb   d
    │ │ @  921dde6e55c0   e
    ╭───┤
    │ │ ◉  ebd06dba20ec   d
    │ ├─╯
    │ ◉  c0cb3a0b73e7   c
    ◉ │  1394f625cbbd   b
    ├─╯
    ◉  2443ea76b0b1   a
    │ ◉  fc054cc36761   a
    ├─╯
    ◉  000000000000
    "###);

    // Check for BUG -- makes too many 'a'-s, etc.
    test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a::"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 2443ea76b0b1 as lyntwxtv 38574b0a a
    Duplicated 1394f625cbbd as pympstqw edc5436d b
    Duplicated c0cb3a0b73e7 as zyzynvxr 0ee42c3f c
    Duplicated ebd06dba20ec as tknxwlno 0be0769b d
    Duplicated 921dde6e55c0 as yqtrmkok 9d48ed7d e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    9d48ed7d40b0   e
    ├─╮
    │ ◉  0be0769bcabe   d
    │ ◉  0ee42c3fc967   c
    ◉ │  edc5436dbeec   b
    ├─╯
    ◉  38574b0af5eb   a
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
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1   a
    ◉  000000000000
    "###);

    let base_operation_id = test_env.current_operation_id(&repo_path);
    test_env.advance_test_rng_seed_to_multiple_of(200_000);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 2443ea76b0b1 as vorwnozm e22f2d74 a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  e22f2d74a67d   a
    │ @  2443ea76b0b1   a
    ├─╯
    ◉  000000000000
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]), @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1   a
    ◉  000000000000
    "###);
}

// https://github.com/martinvonz/jj/issues/694
#[test]
fn test_rebase_duplicates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    @  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ◉  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◉  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as yqosqzyt fdaaf395 b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as vruxwmqv 870cf438 b
    "###);
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    ◉  870cf438ccbb   b @ 2001-02-03 04:05:14.000 +07:00
    │ ◉  fdaaf3950f07   b @ 2001-02-03 04:05:13.000 +07:00
    ├─╯
    │ @  1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    ├─╯
    ◉  2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    ◉  000000000000    @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s", "a", "-d", "a-"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 4 commits
    Working copy now at: zsuskuln 29bd36b6 b | b
    Parent commit      : rlvkpnrz 2f6dc5a1 a | a
    "###);
    // Some of the duplicate commits' timestamps were changed a little to make them
    // have distinct commit ids.
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    ◉  b43fe7354758   b @ 2001-02-03 04:05:14.000 +07:00
    │ ◉  08beb14c3ead   b @ 2001-02-03 04:05:15.000 +07:00
    ├─╯
    │ @  29bd36b60e60   b @ 2001-02-03 04:05:16.000 +07:00
    ├─╯
    ◉  2f6dc5a1ffc2   a @ 2001-02-03 04:05:16.000 +07:00
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
