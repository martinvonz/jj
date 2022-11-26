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

use std::path::PathBuf;

use crate::common::TestEnvironment;

pub mod common;

fn set_up() -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    test_env.jj_cmd_success(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch1"]);
    test_env.jj_cmd_success(&origin_path, &["new", "root", "-m=description 2"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch2"]);
    test_env.jj_cmd_success(&origin_path, &["git", "export"]);

    test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "git",
            "clone",
            origin_git_repo_path.to_str().unwrap(),
            "local",
        ],
    );
    let workspace_root = test_env.env_root().join("local");
    (test_env, workspace_root)
}

#[test]
fn test_git_push_nothing() {
    let (test_env, workspace_root) = set_up();
    // No branches to push yet
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_current_branch() {
    let (test_env, workspace_root) = set_up();
    // Update some branches. `branch1` is not a current branch, but `branch2` and
    // `my-branch` are.
    test_env.jj_cmd_success(
        &workspace_root,
        &["describe", "branch1", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_success(&workspace_root, &["co", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    // Check the setup
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch1: 73650434e2af modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): 828a683493c6 description 1
    branch2: a7ba797894a9 foo
      @origin (behind by 1 commits): 752dad8b1718 description 2
    my-branch: a7ba797894a9 foo
    "###);
    // First dry-run. `branch1` should not get pushed.
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch2 from 752dad8b1718 to a7ba797894a9
      Add branch my-branch to a7ba797894a9
    Dry-run requested, not pushing.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch2 from 752dad8b1718 to a7ba797894a9
      Add branch my-branch to a7ba797894a9
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch1: 73650434e2af modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): 828a683493c6 description 1
    branch2: a7ba797894a9 foo
    my-branch: a7ba797894a9 foo
    "###);
}

#[test]
fn test_git_push_parent_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["edit", "branch1"]);
    test_env.jj_cmd_success(
        &workspace_root,
        &["describe", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Force branch branch1 from 828a683493c6 to 83da0acb6a5a
    Dry-run requested, not pushing.
    "###);
}

#[test]
fn test_git_push_no_current_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No current branch.
    "###);
}

#[test]
fn test_git_push_all() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["branch", "delete", "branch1"]);
    test_env.jj_cmd_success(
        &workspace_root,
        &["branch", "set", "--allow-backwards", "branch2"],
    );
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    // Check the setup
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch1 (deleted)
      @origin: 828a683493c6 description 1
    branch2: afc3e612e744 foo
      @origin (ahead by 1 commits, behind by 1 commits): 752dad8b1718 description 2
    my-branch: afc3e612e744 foo
    "###);
    // First dry-run
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 828a683493c6
      Force branch branch2 from 752dad8b1718 to afc3e612e744
      Add branch my-branch to afc3e612e744
    Dry-run requested, not pushing.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 828a683493c6
      Force branch branch2 from 752dad8b1718 to afc3e612e744
      Add branch my-branch to afc3e612e744
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch2: afc3e612e744 foo
    my-branch: afc3e612e744 foo
    "###);
}

#[test]
fn test_git_push_unsnapshotted_change() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["git", "push", "--change", "@"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["git", "push", "--change", "@"]);
}

#[test]
fn test_git_push_conflict() {
    let (test_env, workspace_root) = set_up();
    std::fs::write(workspace_root.join("file"), "first").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["commit", "-m", "first"]);
    std::fs::write(workspace_root.join("file"), "second").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["commit", "-m", "second"]);
    std::fs::write(workspace_root.join("file"), "third").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["rebase", "-r", "@", "-d", "@--"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "third"]);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 139ce31b3772 since it has conflicts
    "###);
}

#[test]
fn test_git_push_no_description() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m="]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch", "my-branch"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 5b36783cd11c since it has no description
    "###);
}

#[test]
fn test_git_push_missing_author() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    run_without_var("JJ_USER", &["checkout", "root", "-m=initial"]);
    run_without_var("JJ_USER", &["branch", "create", "missing-name"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-name"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 83a72618d57e since it has no author and/or committer set
    "###);
    run_without_var("JJ_EMAIL", &["checkout", "root", "-m=initial"]);
    run_without_var("JJ_EMAIL", &["branch", "create", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 0ed7ef529ef4 since it has no author and/or committer set
    "###);
}

#[test]
fn test_git_push_missing_committer() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "missing-name"]);
    run_without_var("JJ_USER", &["describe", "-m=no committer name"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-name"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 3925a63f25e3 since it has no author and/or committer set
    "###);
    test_env.jj_cmd_success(&workspace_root, &["checkout", "root"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 6c08d8150d73 since it has no author and/or committer set
    "###);

    // Test message when there are multiple reasons (missing committer and
    // description)
    run_without_var("JJ_EMAIL", &["describe", "-m=", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit f73024ee65ec since it has no description and it has no author and/or committer set
    "###);
}
