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
fn test_basics() {
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
    @    [znk] e
    ├─╮
    │ ◉  [vru] d
    │ ◉  [roy] c
    │ │ ◉  [zsu] b
    ├───╯
    ◉ │  [rlv] a
    ├─╯
    ◉  [zzz]
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit vruxwmqv b7c62f28 d | d
    Rebased 1 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq 11a2e10e e | e
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Parent commit      : royxmykx fe2e8e8b c d | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    [znk] e
    ├─╮
    │ ◉  [roy] c d
    │ │ ◉  [zsu] b
    ├───╯
    ◉ │  [rlv] a
    ├─╯
    ◉  [zzz]
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon"] /* abandons `e` */);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit znkkpsqq 5557ece3 e | e
    Working copy now at: nkmrtpmo 6b527513 (empty) (no description set)
    Parent commit      : rlvkpnrz 2443ea76 a e?? | a
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [nkm]
    │ ◉  [zsu] b
    ├─╯
    ◉  [rlv] a e??
    │ ◉  [vru] d e??
    │ ◉  [roy] c
    ├─╯
    ◉  [zzz]
    "###);

    // Abandoning `a` would normally result in its descendant merge commit, `e`,
    // still having two parents. However, since one of those parents (the root
    // commit) would be the ancestor of another, only one of the parents is kept.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit rlvkpnrz 2443ea76 a | a
    Rebased 2 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq b0af79c3 e | e
    Parent commit      : vruxwmqv b7c62f28 d | d
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] e
    ◉  [vru] d
    ◉  [roy] c
    │ ◉  [zsu] b
    ├─╯
    ◉  [zzz] a
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "descendants(c)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned the following commits:
      znkkpsqq 5557ece3 e | e
      vruxwmqv b7c62f28 d | d
      royxmykx fe2e8e8b c | c
    Working copy now at: wvuyspvk 3f93e69f (empty) (no description set)
    Parent commit      : rlvkpnrz 2443ea76 a e?? | a
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [wvu]
    │ ◉  [zsu] b
    ├─╯
    ◉  [rlv] a e??
    ◉  [zzz] c d e??
    "###);

    // Test abandoning the same commit twice directly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "b", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit zsuskuln 1394f625 b | b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    [znk] e
    ├─╮
    │ ◉  [vru] d
    │ ◉  [roy] c
    ◉ │  [rlv] a b
    ├─╯
    ◉  [zzz]
    "###);

    // Test abandoning the same commit twice indirectly
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "d::", "a::"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned the following commits:
      znkkpsqq 5557ece3 e | e
      vruxwmqv b7c62f28 d | d
      zsuskuln 1394f625 b | b
      rlvkpnrz 2443ea76 a | a
    Working copy now at: oupztwtk 304ae338 (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 a b e?? | (empty) (no description set)
    Added 0 files, modified 0 files, removed 4 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [oup]
    │ ◉  [roy] c d e??
    ├─╯
    ◉  [zzz] a b e??
    "###);
}

// TODO(#2600): Make sure the results here become saner as #2600 is fixed. There
// is an simpler demo of #2600 at https://github.com/martinvonz/jj/pull/2655.
// However, fixing #2600 will likely change how `abandon` works. This test
// exists to track how that happens. See also the corresponding test in
// `test_rebase_command`
#[test]
fn test_bug_2600() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // We will not touch "nottherootcommit". See the
    // `test_bug_2600_rootcommit_special_case` for the one case where base being the
    // child of the root commit changes the expected behavior.
    create_commit(&test_env, &repo_path, "nottherootcommit", &[]);
    create_commit(&test_env, &repo_path, "base", &["nottherootcommit"]);
    create_commit(&test_env, &repo_path, "a", &["base"]);
    create_commit(&test_env, &repo_path, "b", &["base", "a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] c
    ◉    [vru] b
    ├─╮
    │ ◉  [roy] a
    ├─╯
    ◉  [zsu] base
    ◉  [rlv] nottherootcommit
    ◉  [zzz]
    "###);
    let setup_opid = test_env.current_operation_id(&repo_path);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "base"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit zsuskuln 73c929fc base | base
    Rebased 3 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq 510f8756 c | c
    Parent commit      : vruxwmqv 7301d9ab b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // BUG. The user would expect
    // @  c
    // ├─╮
    // │ ◉  a
    // ├─╯
    // ◉  base nottherootcommit
    // ◉
    // This is likely caused by DescendantRebaser
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] c
    ◉  [vru] b
    ◉  [roy] a
    ◉  [rlv] base nottherootcommit
    ◉  [zzz]
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit royxmykx 98f3b9ba a | a
    Rebased 2 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq 683b9435 c | c
    Parent commit      : vruxwmqv c10cb7b4 b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // This is likely what the user will expect.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] c
    ◉  [vru] b
    ◉  [zsu] a base
    ◉  [rlv] nottherootcommit
    ◉  [zzz]
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit vruxwmqv 8c0dced0 b | b
    Rebased 1 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq 924bdd1c c | c
    Parent commit      : royxmykx 98f3b9ba a b?? | a
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // BUG. The user would expect
    // @  c
    // ├─╮
    // │ ◉  a
    // ├─╯
    // ◉  base
    // ◉  nottherootcommit
    // ◉
    // This is likely caused by logic in `cmd_abandon`, not DescendantRebaser
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] c
    ◉  [roy] a b??
    ◉  [zsu] b?? base
    ◉  [rlv] nottherootcommit
    ◉  [zzz]
    "###);

    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    // ========= Reminder of the setup ===========
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] c
    ◉    [vru] b
    ├─╮
    │ ◉  [roy] a
    ├─╯
    ◉  [zsu] base
    ◉  [rlv] nottherootcommit
    ◉  [zzz]
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "a", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned the following commits:
      royxmykx 98f3b9ba a | a
      vruxwmqv 8c0dced0 b | b
    Rebased 1 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq 84fac1f8 c | c
    Parent commit      : zsuskuln 73c929fc a b base | base
    Added 0 files, modified 0 files, removed 2 files
    "###);
    // This is likely what the user would expect
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [znk] c
    ◉  [zsu] a b base
    ◉  [rlv] nottherootcommit
    ◉  [zzz]
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "list", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b: zsuskuln 73c929fc base
    "###);
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_bug_2600_rootcommit_special_case() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up like `test_bug_2600`, but without the `nottherootcommit` commit.
    create_commit(&test_env, &repo_path, "base", &[]);
    create_commit(&test_env, &repo_path, "a", &["base"]);
    create_commit(&test_env, &repo_path, "b", &["base", "a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);

    // Setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [vru] c
    ◉    [roy] b
    ├─╮
    │ ◉  [zsu] a
    ├─╯
    ◉  [rlv] base
    ◉  [zzz]
    "###);

    // Now, the test
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", "base"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit rlvkpnrz 0c61db1b base | base
    Rebased 3 descendant commits onto parents of abandoned commits
    Working copy now at: vruxwmqv 73e9185c c | c
    Parent commit      : royxmykx 80dd9cba b | b
    Added 0 files, modified 0 files, removed 1 files
    "###);
    // The current behavior is either correct or should be replaced with an error
    // message. Even though the user would expect `b` to still be a descendant of
    // `base`, it is impossible in the Git backend.
    // See also https://github.com/martinvonz/jj/issues/2600#issuecomment-1835418824
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  [vru] c
    ◉  [roy] b
    ◉  [zsu] a
    ◉  [zzz] base
    "###);
}

#[test]
fn test_double_abandon() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    // Test the setup
    insta::assert_snapshot!(
    test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r", "a"])
        , @r###"
    rlvkpnrz test.user@example.com 2001-02-03 04:05:09.000 +07:00 a 2443ea76
    a
    "###);

    let commit_id = test_env.jj_cmd_success(
        &repo_path,
        &["log", "--no-graph", "--color=never", "-T=commit_id", "-r=a"],
    );

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", &commit_id]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit rlvkpnrz 2443ea76 a | a
    Working copy now at: royxmykx f37b4afd (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 a | (empty) (no description set)
    Added 0 files, modified 0 files, removed 1 files
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["abandon", &commit_id]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Abandoned commit rlvkpnrz hidden 2443ea76 a
    Nothing changed.
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "-T",
            r#"separate(" ", "[" ++ change_id.short(3) ++ "]", branches)"#,
        ],
    )
}
