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

use std::fs;

use crate::common::TestEnvironment;

#[test]
fn test_git_remotes() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "--git", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "foo", "http://example.com/repo/foo"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "bar", "http://example.com/repo/bar"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    bar http://example.com/repo/bar
    foo http://example.com/repo/foo
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "remote", "remove", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @"bar http://example.com/repo/bar
");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "remove", "nonexistent"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No git remote named 'nonexistent'
    "###);
}

#[test]
fn test_git_remote_add() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "--git", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "foo", "http://example.com/repo/foo"],
    );
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &[
            "git",
            "remote",
            "add",
            "foo",
            "http://example.com/repo/foo2",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Git remote named 'foo' already exists
    "###);
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["git", "remote", "add", "git", "http://example.com/repo/git"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Git remote named 'git' is reserved for local Git repository
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    foo http://example.com/repo/foo
    "###);
}

#[test]
fn test_git_remote_rename() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "--git", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "foo", "http://example.com/repo/foo"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "baz", "http://example.com/repo/baz"],
    );
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "rename", "bar", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No git remote named 'bar'
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "rename", "foo", "baz"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Git remote named 'baz' already exists
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "rename", "foo", "git"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Git remote named 'git' is reserved for local Git repository
    "###);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["git", "remote", "rename", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    bar http://example.com/repo/foo
    baz http://example.com/repo/baz
    "###);
}

#[test]
fn test_git_remote_named_git() {
    let test_env = TestEnvironment::default();

    // Existing remote named 'git' shouldn't block the repo initialization.
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    git_repo
        .remote("git", "http://example.com/repo/repo")
        .unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);

    // The remote can be renamed.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["git", "remote", "rename", "git", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    bar http://example.com/repo/repo
    "###);
    // @git branch shouldn't be renamed.
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-rmain@git", "-Tbranches"]);
    insta::assert_snapshot!(stdout, @r###"
    @  main
    │
    ~
    "###);

    // The remote cannot be renamed back by jj.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "rename", "bar", "git"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Git remote named 'git' is reserved for local Git repository
    "###);

    // Reinitialize the repo with remote named 'git'.
    fs::remove_dir_all(repo_path.join(".jj")).unwrap();
    git_repo.remote_rename("bar", "git").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);

    // The remote can also be removed.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "remote", "remove", "git"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
    // @git branch shouldn't be removed.
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-rmain@git", "-Tbranches"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  main
    │
    ~
    "###);
}
