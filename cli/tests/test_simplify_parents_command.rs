// Copyright 2024 The Jujutsu Authors
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
use std::path::PathBuf;

use test_case::test_case;

use crate::common::TestEnvironment;

fn create_repo() -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    (test_env, repo_path)
}

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    let mut args = vec!["new", "-m", name];
    args.extend(parents);
    test_env.jj_cmd_ok(repo_path, &args);

    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_ok(repo_path, &["bookmark", "create", name]);
}

#[test]
fn test_simplify_parents_no_commits() {
    let (test_env, repo_path) = create_repo();

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["simplify-parents", "-r", "root() ~ root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_simplify_parents_immutable() {
    let (test_env, repo_path) = create_repo();

    let stderr = test_env.jj_cmd_failure(&repo_path, &["simplify-parents", "-r", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
    "###);
}

#[test]
fn test_simplify_parents_no_change() {
    let (test_env, repo_path) = create_repo();

    create_commit(&test_env, &repo_path, "a", &["root()"]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  b
    ○  a
    ◆
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["simplify-parents", "-s", "@-"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  b
    ○  a
    ◆
    "###);
}

#[test]
fn test_simplify_parents_no_change_diamond() {
    let (test_env, repo_path) = create_repo();

    create_commit(&test_env, &repo_path, "a", &["root()"]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a"]);
    create_commit(&test_env, &repo_path, "d", &["b", "c"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    d
    ├─╮
    │ ○  c
    ○ │  b
    ├─╯
    ○  a
    ◆
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["simplify-parents", "-r", "all() ~ root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    d
    ├─╮
    │ ○  c
    ○ │  b
    ├─╯
    ○  a
    ◆
    "###);
}

#[test_case(&["simplify-parents", "-r", "@", "-r", "@-"] ; "revisions")]
#[test_case(&["simplify-parents", "-s", "@-"] ; "sources")]
fn test_simplify_parents_redundant_parent(args: &[&str]) {
    let (test_env, repo_path) = create_repo();

    create_commit(&test_env, &repo_path, "a", &["root()"]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::allow_duplicates! {
    insta::assert_snapshot!(stdout, @r###"
    @    c
    ├─╮
    │ ○  b
    ├─╯
    ○  a
    ◆
    "###);
    }
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, args);
    insta::allow_duplicates! {
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Removed 1 edges from 1 out of 3 commits.
    Working copy now at: royxmykx 0ac2063b c | c
    Parent commit      : zsuskuln 1394f625 b | b
    "###);
    }

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::allow_duplicates! {
    insta::assert_snapshot!(stdout, @r###"
    @  c
    ○  b
    ○  a
    ◆
    "###);
    }
}

#[test]
fn test_simplify_parents_multiple_redundant_parents() {
    let (test_env, repo_path) = create_repo();

    create_commit(&test_env, &repo_path, "a", &["root()"]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["d"]);
    create_commit(&test_env, &repo_path, "f", &["d", "e"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @    f
    ├─╮
    │ ○  e
    ├─╯
    ○  d
    ○    c
    ├─╮
    │ ○  b
    ├─╯
    ○  a
    ◆
    "#);
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Test with `-r`.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["simplify-parents", "-r", "c", "-r", "f"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Removed 2 edges from 2 out of 2 commits.
    Rebased 2 descendant commits
    Working copy now at: kmkuslsw 8cc01e1b f | f
    Parent commit      : znkkpsqq 040ae3a6 e | e
    "#);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @  f
    ○  e
    ○  d
    ○  c
    ○  b
    ○  a
    ◆
    "#);

    // Test with `-s`.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["simplify-parents", "-s", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Removed 2 edges from 2 out of 4 commits.
    Rebased 2 descendant commits
    Working copy now at: kmkuslsw 70a39dff f | f
    Parent commit      : znkkpsqq a021fee9 e | e
    "#);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @  f
    ○  e
    ○  d
    ○  c
    ○  b
    ○  a
    ◆
    "#);
}

#[test]
fn test_simplify_parents_no_args() {
    let (test_env, repo_path) = create_repo();

    create_commit(&test_env, &repo_path, "a", &["root()"]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["d"]);
    create_commit(&test_env, &repo_path, "f", &["d", "e"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @    f
    ├─╮
    │ ○  e
    ├─╯
    ○  d
    ○    c
    ├─╮
    │ ○  b
    ├─╯
    ○  a
    ◆
    "#);
    let setup_opid = test_env.current_operation_id(&repo_path);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["simplify-parents"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Removed 2 edges from 2 out of 6 commits.
    Rebased 2 descendant commits
    Working copy now at: kmkuslsw 8cc01e1b f | f
    Parent commit      : znkkpsqq 040ae3a6 e | e
    "#);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @  f
    ○  e
    ○  d
    ○  c
    ○  b
    ○  a
    ◆
    "#);

    // Test with custom `revsets.simplify-parents`.
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
    test_env.add_config(r#"revsets.simplify-parents = "d::""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["simplify-parents"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Removed 1 edges from 1 out of 3 commits.
    Working copy now at: kmkuslsw 0c6b4c43 f | f
    Parent commit      : znkkpsqq 6a679611 e | e
    "#);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @  f
    ○  e
    ○  d
    ○    c
    ├─╮
    │ ○  b
    ├─╯
    ○  a
    ◆
    "#);
}
