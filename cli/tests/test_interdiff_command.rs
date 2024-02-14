// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use crate::common::TestEnvironment;

#[test]
fn test_interdiff_basic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "left"]);

    test_env.jj_cmd_ok(&repo_path, &["checkout", "root()"]);
    std::fs::write(repo_path.join("file3"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file2"), "foo\nbar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "right"]);

    // implicit --to
    let stdout = test_env.jj_cmd_success(&repo_path, &["interdiff", "--from", "left"]);
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file2:
       1    1: foo
            2: bar
    "###);

    // explicit --to
    test_env.jj_cmd_ok(&repo_path, &["checkout", "@-"]);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["interdiff", "--from", "left", "--to", "right"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file2:
       1    1: foo
            2: bar
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // formats specifiers
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["interdiff", "--from", "left", "--to", "right", "-s"],
    );
    insta::assert_snapshot!(stdout, @r###"
    M file2
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["interdiff", "--from", "left", "--to", "right", "--git"],
    );
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file2 b/file2
    index 257cc5642c...3bd1f0e297 100644
    --- a/file2
    +++ b/file2
    @@ -1,1 +1,2 @@
     foo
    +bar
    "###);
}

#[test]
fn test_interdiff_paths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "bar\n").unwrap();
    std::fs::write(repo_path.join("file2"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "left"]);

    test_env.jj_cmd_ok(&repo_path, &["checkout", "root()"]);
    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "baz\n").unwrap();
    std::fs::write(repo_path.join("file2"), "baz\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "right"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["interdiff", "--from", "left", "--to", "right", "file1"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file1:
       1    1: barbaz
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "interdiff",
            "--from",
            "left",
            "--to",
            "right",
            "file1",
            "file2",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file1:
       1    1: barbaz
    Modified regular file file2:
       1    1: barbaz
    "###);
}

#[test]
fn test_interdiff_conflicting() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "left"]);

    test_env.jj_cmd_ok(&repo_path, &["checkout", "root()"]);
    std::fs::write(repo_path.join("file"), "abc\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "def\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "right"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["interdiff", "--from", "left", "--to", "right", "--git"],
    );
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file b/file
    index 0000000000...24c5735c3e 100644
    --- a/file
    +++ b/file
    @@ -1,7 +1,1 @@
    -<<<<<<<
    -%%%%%%%
    --foo
    -+abc
    -+++++++
    -bar
    ->>>>>>>
    +def
    "###);
}
