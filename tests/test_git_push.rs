// Copyright 2022 Google LLC
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
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let empty_tree_oid = git_repo.treebuilder(None).unwrap().write().unwrap();
    let empty_tree = git_repo.find_tree(empty_tree_oid).unwrap();
    git_repo
        .commit(
            Some("refs/heads/branch1"),
            &signature,
            &signature,
            "description 1",
            &empty_tree,
            &[],
        )
        .unwrap();
    git_repo
        .commit(
            Some("refs/heads/branch2"),
            &signature,
            &signature,
            "description 2",
            &empty_tree,
            &[],
        )
        .unwrap();

    test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "git",
            "clone",
            test_env.env_root().join("git-repo").to_str().unwrap(),
            "jj-repo",
        ],
    );
    let workspace_root = test_env.env_root().join("jj-repo");
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
    branch1: 5d0d85ed3da7 modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): a3ccc578ea7b description 1
    branch2: 60db6d808983 foo
      @origin (behind by 1 commits): 7fd4b07286b3 description 2
    my-branch: 60db6d808983 foo
    "###);
    // First dry-run. `branch1` should not get pushed.
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch2 from 7fd4b07286b3 to 60db6d808983
      Add branch my-branch to 60db6d808983
    Dry-run requested, not pushing.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch2 from 7fd4b07286b3 to 60db6d808983
      Add branch my-branch to 60db6d808983
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch1: 5d0d85ed3da7 modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): a3ccc578ea7b description 1
    branch2: 60db6d808983 foo
    my-branch: 60db6d808983 foo
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
      Force branch branch1 from a3ccc578ea7b to ad7201b22c46
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
      @origin: a3ccc578ea7b description 1
    branch2: 7840c9885676 foo
      @origin (ahead by 1 commits, behind by 1 commits): 7fd4b07286b3 description 2
    my-branch: 7840c9885676 foo
    "###);
    // First dry-run
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from a3ccc578ea7b
      Force branch branch2 from 7fd4b07286b3 to 7840c9885676
      Add branch my-branch to 7840c9885676
    Dry-run requested, not pushing.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from a3ccc578ea7b
      Force branch branch2 from 7fd4b07286b3 to 7840c9885676
      Add branch my-branch to 7840c9885676
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch2: 7840c9885676 foo
    my-branch: 7840c9885676 foo
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
    Error: Won't push commit 50ccff1aeab0 since it has conflicts
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
    Error: Won't push commit 230dd059e1b0 since it has no description
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
    Error: Won't push commit 91a20b396803 since it has no author and/or committer set
    "###);
    run_without_var("JJ_EMAIL", &["checkout", "root", "-m=initial"]);
    run_without_var("JJ_EMAIL", &["branch", "create", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 7186423bd158 since it has no author and/or committer set
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
    Error: Won't push commit df8d9f6cf625 since it has no author and/or committer set
    "###);
    test_env.jj_cmd_success(&workspace_root, &["checkout", "root"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 61b8a14387d7 since it has no author and/or committer set
    "###);

    // Test message when there are multiple reasons (missing committer and
    // description)
    run_without_var("JJ_EMAIL", &["describe", "-m=", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 9e1aae45b6a3 since it has no description and it has no author and/or committer set
    "###);
}
