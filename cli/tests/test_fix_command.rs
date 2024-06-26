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

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use itertools::Itertools;
use jj_lib::file_util::try_symlink;

use crate::common::TestEnvironment;

/// Set up a repo where the `jj fix` command uses the fake editor with the given
/// flags.
fn init_with_fake_formatter(args: &[&str]) -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r#"fix.tool-command = ["{}"]"#,
        [escaped_formatter_path.as_str()]
            .iter()
            .chain(args)
            .join(r#"", ""#)
    ));
    (test_env, repo_path)
}

#[test]
fn test_fix_no_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: Invalid `fix.tool-command`
    Caused by: configuration property "fix.tool-command" not found
    For help, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
    "###);
}

#[test]
fn test_fix_empty_commit() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
}

#[test]
fn test_fix_leaf_commit() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "unaffected").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "affected").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: rlvkpnrz 8b02703b (no description set)
    Parent commit      : qpvuntsm fda57e40 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@-"]);
    insta::assert_snapshot!(content, @"unaffected");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"AFFECTED");
}

#[test]
fn test_fix_parent_commit() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    // Using one file name for all commits adds coverage of some possible bugs.
    std::fs::write(repo_path.join("file"), "parent").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "parent"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "child1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "child1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "parent"]);
    std::fs::write(repo_path.join("file"), "child2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "child2"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "parent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 3 commits of 3 checked.
    Working copy now at: mzvwutvl d6abb1f4 child2 | (no description set)
    Parent commit      : qpvuntsm 4f4d2103 parent | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "parent"]);
    insta::assert_snapshot!(content, @"PARENT");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child1"]);
    insta::assert_snapshot!(content, @"CHILD1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child2"]);
    insta::assert_snapshot!(content, @"CHILD2");
}

#[test]
fn test_fix_sibling_commit() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "parent").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "parent"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "child1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "child1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "parent"]);
    std::fs::write(repo_path.join("file"), "child2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "child2"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "child1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "parent"]);
    insta::assert_snapshot!(content, @"parent");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child1"]);
    insta::assert_snapshot!(content, @"CHILD1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child2"]);
    insta::assert_snapshot!(content, @"child2");
}

#[test]
fn test_default_revset() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "trunk1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "trunk1"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "trunk2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "trunk2"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "trunk1"]);
    std::fs::write(repo_path.join("file"), "foo").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "trunk1"]);
    std::fs::write(repo_path.join("file"), "bar1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "bar1"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "bar2"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar3").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "bar3"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "bar2"]);

    // With no args and no revset configuration, we fix `reachable(@, mutable())`,
    // which includes bar{1,2,3} and excludes trunk{1,2} (which is immutable) and
    // foo (which is mutable but not reachable).
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "trunk2""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 3 commits of 3 checked.
    Working copy now at: yostqsxw 0bd830d2 bar2 | (no description set)
    Parent commit      : yqosqzyt 4747dd17 bar1 | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "trunk1"]);
    insta::assert_snapshot!(content, @"trunk1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "trunk2"]);
    insta::assert_snapshot!(content, @"trunk2");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "foo"]);
    insta::assert_snapshot!(content, @"foo");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar1"]);
    insta::assert_snapshot!(content, @"BAR1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar2"]);
    insta::assert_snapshot!(content, @"BAR2");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar3"]);
    insta::assert_snapshot!(content, @"BAR3");
}

#[test]
fn test_custom_default_revset() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);

    std::fs::write(repo_path.join("file"), "foo").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "bar"]);

    // Check out a different commit so that the schema default `reachable(@,
    // mutable())` would behave differently from our customized default.
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "foo"]);
    test_env.add_config(r#"revsets.fix = "bar""#);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "foo"]);
    insta::assert_snapshot!(content, @"foo");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar"]);
    insta::assert_snapshot!(content, @"BAR");
}

#[test]
fn test_fix_immutable_commit() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "immutable").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "immutable"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "mutable").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "mutable"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "immutable""#);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix", "-s", "immutable"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 83eee3c8dce2 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "immutable"]);
    insta::assert_snapshot!(content, @"immutable");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "mutable"]);
    insta::assert_snapshot!(content, @"mutable");
}

#[test]
fn test_fix_empty_file() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"");
}

#[test]
fn test_fix_some_paths() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    std::fs::write(repo_path.join("file2"), "bar").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 3f72f723 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(content, @r###"
    FOO
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
    insta::assert_snapshot!(content, @"bar");
}

#[test]
fn test_fix_cyclic() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--reverse"]);
    std::fs::write(repo_path.join("file"), "content\n").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm affcf432 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"tnetnoc\n");

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 2de05835 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content\n");
}

#[test]
fn test_deduplication() {
    // Append all fixed content to a log file. This assumes we're running the tool
    // in the root directory of the repo, which is worth reconsidering if we
    // establish a contract regarding cwd.
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase", "--tee", "$path-fixlog"]);

    // There are at least two interesting cases: the content is repeated immediately
    // in the child commit, or later in another descendant.
    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 4 commits of 4 checked.
    Working copy now at: yqosqzyt 5ac0edc4 d | (no description set)
    Parent commit      : mzvwutvl 90d9a032 c | (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "a"]);
    insta::assert_snapshot!(content, @"FOO\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "b"]);
    insta::assert_snapshot!(content, @"BAR\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "c"]);
    insta::assert_snapshot!(content, @"BAR\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "d"]);
    insta::assert_snapshot!(content, @"FOO\n");

    // Each new content string only appears once in the log, because all the other
    // inputs (like file name) were identical, and so the results were re-used. We
    // sort the log because the order of execution inside `jj fix` is undefined.
    insta::assert_snapshot!(sorted_lines(repo_path.join("file-fixlog")), @"BAR\nFOO\n");
}

fn sorted_lines(path: PathBuf) -> String {
    let mut log: Vec<_> = std::fs::read_to_string(path.as_os_str())
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    log.sort();
    log.join("\n")
}

#[test]
fn test_executed_but_nothing_changed() {
    // Show that the tool ran by causing a side effect with --tee, and test that we
    // do the right thing when the tool's output is exactly equal to its input.
    let (test_env, repo_path) = init_with_fake_formatter(&["--tee", "$path-copy"]);
    std::fs::write(repo_path.join("file"), "content\n").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content\n");
    let copy_content = std::fs::read_to_string(repo_path.join("file-copy").as_os_str()).unwrap();
    insta::assert_snapshot!(copy_content, @"content\n");
}

#[test]
fn test_failure() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--fail"]);
    std::fs::write(repo_path.join("file"), "content").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content");
}

#[test]
fn test_stderr_success() {
    let (test_env, repo_path) =
        init_with_fake_formatter(&["--stderr", "error", "--stdout", "new content"]);
    std::fs::write(repo_path.join("file"), "old content").unwrap();

    // TODO: Associate the stderr lines with the relevant tool/file/commit instead
    // of passing it through directly.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    errorFixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm e8c5cda3 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"new content");
}

#[test]
fn test_stderr_failure() {
    let (test_env, repo_path) =
        init_with_fake_formatter(&["--stderr", "error", "--stdout", "new content", "--fail"]);
    std::fs::write(repo_path.join("file"), "old content").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    errorFixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"old content");
}

#[test]
fn test_missing_command() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(r#"fix.tool-command = ["this_executable_shouldnt_exist"]"#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    // TODO: We should display a warning about invalid tool configurations. When we
    // support multiple tools, we should also keep going to see if any of the other
    // executions succeed.
    insta::assert_snapshot!(stderr, @r###"
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
}

#[test]
fn test_fix_file_types() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "content").unwrap();
    std::fs::create_dir(repo_path.join("dir")).unwrap();
    try_symlink("file", repo_path.join("link")).unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 72bf7048 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"CONTENT");
}

#[cfg(unix)]
#[test]
fn test_fix_executable() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    let path = repo_path.join("file");
    std::fs::write(&path, "content").unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(&path, permissions).unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm eea49ac9 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"CONTENT");
    let executable = std::fs::metadata(&path).unwrap().permissions().mode() & 0o111;
    assert_eq!(executable, 0o111);
}

#[test]
fn test_fix_trivial_merge_commit() {
    // All the changes are attributable to a parent, so none are fixed (in the same
    // way that none would be shown in `jj diff -r @`).
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file_a"), "content a").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file_b"), "content b").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_a", "-r", "@"]);
    insta::assert_snapshot!(content, @"content a");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_b", "-r", "@"]);
    insta::assert_snapshot!(content, @"content b");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_c", "-r", "@"]);
    insta::assert_snapshot!(content, @"content c");
}

#[test]
fn test_fix_adding_merge_commit() {
    // None of the changes are attributable to a parent, so they are all fixed (in
    // the same way that they would be shown in `jj diff -r @`).
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file_a"), "content a").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file_b"), "content b").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);
    std::fs::write(repo_path.join("file_a"), "change a").unwrap();
    std::fs::write(repo_path.join("file_b"), "change b").unwrap();
    std::fs::write(repo_path.join("file_c"), "change c").unwrap();
    std::fs::write(repo_path.join("file_d"), "change d").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 1 commits of 1 checked.
    Working copy now at: mzvwutvl 899a1398 (no description set)
    Parent commit      : qpvuntsm 34782c48 a | (no description set)
    Parent commit      : kkmpptxz 82e9bc6a b | (no description set)
    Added 0 files, modified 4 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_a", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE A");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_b", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE B");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_c", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE C");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_d", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE D");
}

#[test]
fn test_fix_both_sides_of_conflict() {
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "content a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file"), "content b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);

    // The conflicts are not different from the merged parent, so they would not be
    // fixed if we didn't fix the parents also.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "a", "-s", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 3 commits of 3 checked.
    Working copy now at: mzvwutvl b7967885 (conflict) (empty) (no description set)
    Parent commit      : qpvuntsm 06fe435a a | (no description set)
    Parent commit      : kkmpptxz ce7ee79e b | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "a"]);
    insta::assert_snapshot!(content, @r###"
    CONTENT A
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "b"]);
    insta::assert_snapshot!(content, @r###"
    CONTENT B
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    +CONTENT A
    +++++++ Contents of side #2
    CONTENT B
    >>>>>>> Conflict 1 of 1 ends
    "###);
}

#[test]
fn test_fix_resolve_conflict() {
    // If both sides of the conflict look the same after being fixed, the conflict
    // will be resolved.
    let (test_env, repo_path) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "Content\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file"), "cOnTeNt\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);

    // The conflicts are not different from the merged parent, so they would not be
    // fixed if we didn't fix the parents also.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "a", "-s", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 3 commits of 3 checked.
    Working copy now at: mzvwutvl 669396ce (empty) (no description set)
    Parent commit      : qpvuntsm 3c63716f a | (no description set)
    Parent commit      : kkmpptxz 82703f5e b | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    CONTENT
    "###);
}
