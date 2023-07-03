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
    branch1: 19e00bf64429 modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): 45a3aa29e907 description 1
    branch2: 10ee3363b259 foo
      @origin (behind by 1 commits): 8476341eb395 description 2
    my-branch: 10ee3363b259 foo
    "###);
    // First dry-run. `branch1` should not get pushed.
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch2 from 8476341eb395 to 10ee3363b259
      Add branch my-branch to 10ee3363b259
    Dry-run requested, not pushing.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch2 from 8476341eb395 to 10ee3363b259
      Add branch my-branch to 10ee3363b259
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch1: 19e00bf64429 modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): 45a3aa29e907 description 1
    branch2: 10ee3363b259 foo
    my-branch: 10ee3363b259 foo
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
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Force branch branch1 from 45a3aa29e907 to d47326d59ee1
    "###);
}

#[test]
fn test_git_no_push_parent_branch_non_empty_commit() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["edit", "branch1"]);
    test_env.jj_cmd_success(
        &workspace_root,
        &["describe", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    std::fs::write(workspace_root.join("file"), "file").unwrap();
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No current branch.
    "###);
}

#[test]
fn test_git_no_push_parent_branch_description() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["edit", "branch1"]);
    test_env.jj_cmd_success(
        &workspace_root,
        &["describe", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "non-empty description"]);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No current branch.
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
fn test_git_push_current_branch_unchanged() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["co", "branch1"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_multiple() {
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
      @origin: 45a3aa29e907 description 1
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    branch2: 15dcdaa4f12f foo
      @origin (ahead by 1 commits, behind by 1 commits): 8476341eb395 description 2
    my-branch: 15dcdaa4f12f foo
    "###);
    // First dry-run
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 45a3aa29e907
      Force branch branch2 from 8476341eb395 to 15dcdaa4f12f
      Add branch my-branch to 15dcdaa4f12f
    Dry-run requested, not pushing.
    "###);
    // Dry run requesting two specific branches
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "push", "-b=branch1", "-b=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 45a3aa29e907
      Add branch my-branch to 15dcdaa4f12f
    Dry-run requested, not pushing.
    "###);
    // Dry run requesting two specific branches twice
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &[
            "git",
            "push",
            "-b=branch1",
            "-b=my-branch",
            "-b=branch1",
            "-b=my-branch",
            "--dry-run",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 45a3aa29e907
      Add branch my-branch to 15dcdaa4f12f
    Dry-run requested, not pushing.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 45a3aa29e907
      Force branch branch2 from 8476341eb395 to 15dcdaa4f12f
      Add branch my-branch to 15dcdaa4f12f
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    branch2: 15dcdaa4f12f foo
    my-branch: 15dcdaa4f12f foo
    "###);
}

#[test]
fn test_git_push_changes() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "bar"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();

    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--change", "@"]);
    insta::assert_snapshot!(stdout, @r###"
    Creating branch push-yostqsxwqrlt for revision @
    Branch changes to push to origin:
      Add branch push-yostqsxwqrlt to 28d7620ea63a
    "###);
    // test pushing two changes at once
    std::fs::write(workspace_root.join("file"), "modified2").unwrap();
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "push", "--change", "@", "--change", "@-"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Creating branch push-yqosqzytrlsw for revision @-
    Branch changes to push to origin:
      Force branch push-yostqsxwqrlt from 28d7620ea63a to 48d8c7948133
      Add branch push-yqosqzytrlsw to fa16a14170fb
    "###);
    // specifying the same change twice doesn't break things
    std::fs::write(workspace_root.join("file"), "modified3").unwrap();
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "push", "--change", "@", "--change", "@"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Force branch push-yostqsxwqrlt from 48d8c7948133 to b5f030322b1d
    "###);
}

#[test]
fn test_git_push_revisions() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "bar"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "branch-1"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "baz"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "branch-2a"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "branch-2b"]);
    std::fs::write(workspace_root.join("file"), "modified again").unwrap();

    // Push an empty set
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-r=none()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Empty revision set
    "###);
    // Push a revision with no branches
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-r=@--"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No branches point to the specified revisions.
    "###);
    // Push a revision with a single branch
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "-r=@-", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Add branch branch-1 to 7decc7932d9c
    Dry-run requested, not pushing.
    "###);
    // Push multiple revisions of which some have branches
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "push", "-r=@--", "-r=@-", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Add branch branch-1 to 7decc7932d9c
    Dry-run requested, not pushing.
    "###);
    // Push a revision with a multiple branches
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "-r=@", "--dry-run"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Add branch branch-2a to 1b45449e18d0
      Add branch branch-2b to 1b45449e18d0
    Dry-run requested, not pushing.
    "###);
    // Repeating a commit doesn't result in repeated messages about the branch
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "push", "-r=@-", "-r=@-", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Add branch branch-1 to 7decc7932d9c
    Dry-run requested, not pushing.
    "###);
}

#[test]
fn test_git_push_mixed() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "bar"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "branch-1"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "baz"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "branch-2a"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "branch-2b"]);
    std::fs::write(workspace_root.join("file"), "modified again").unwrap();

    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "push", "--change=@--", "--branch=branch-1", "-r=@"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Creating branch push-yqosqzytrlsw for revision @--
    Branch changes to push to origin:
      Add branch branch-1 to 7decc7932d9c
      Add branch push-yqosqzytrlsw to fa16a14170fb
      Add branch branch-2a to 1b45449e18d0
      Add branch branch-2b to 1b45449e18d0
    "###);
}

#[test]
fn test_git_push_existing_long_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(
        &workspace_root,
        &["branch", "create", "push-19b790168e73f7a73a98deae21e807c0"],
    );

    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--change=@"]);

    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Add branch push-19b790168e73f7a73a98deae21e807c0 to fa16a14170fb
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
    Error: Won't push commit 3a1497bff04c since it has conflicts
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
    Error: Won't push commit 574dffd73428 since it has no author and/or committer set
    "###);
    run_without_var("JJ_EMAIL", &["checkout", "root", "-m=initial"]);
    run_without_var("JJ_EMAIL", &["branch", "create", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit e6c50f13f197 since it has no author and/or committer set
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
    Error: Won't push commit e009726caa4a since it has no author and/or committer set
    "###);
    test_env.jj_cmd_success(&workspace_root, &["checkout", "root"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 27ec5f0793e6 since it has no author and/or committer set
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

#[test]
fn test_git_push_deleted() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_success(&workspace_root, &["branch", "delete", "branch1"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from 45a3aa29e907
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_conflicting_branches() {
    let (test_env, workspace_root) = set_up();
    let git_repo = {
        let mut git_repo_path = workspace_root.clone();
        git_repo_path.extend([".jj", "repo", "store", "git"]);
        git2::Repository::open(&git_repo_path).unwrap()
    };

    // Forget remote ref, move local ref, then fetch to create conflict.
    git_repo
        .find_reference("refs/remotes/origin/branch2")
        .unwrap()
        .delete()
        .unwrap();
    test_env.jj_cmd_success(&workspace_root, &["git", "import"]);
    test_env.jj_cmd_success(&workspace_root, &["new", "root", "-m=description 3"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["git", "fetch"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&workspace_root, &["branch", "list"]), @r###"
    branch1: 45a3aa29e907 description 1
    branch2 (conflicted):
      + 8e670e2d47e1 description 3
      + 8476341eb395 description 2
      @origin (behind by 1 commits): 8476341eb395 description 2
    "###);

    let bump_branch1 = || {
        test_env.jj_cmd_success(&workspace_root, &["new", "branch1", "-m=bump"]);
        test_env.jj_cmd_success(&workspace_root, &["branch", "set", "branch1"]);
    };

    // Conflicting branch at @
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Branch branch2 is conflicted
    "###);

    // --branch should be blocked by conflicting branch
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch", "branch2"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Branch branch2 is conflicted
    "###);

    // --all shouldn't be blocked by conflicting branch
    bump_branch1();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch1 from 45a3aa29e907 to fd1d63e031ea
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Branch branch2 is conflicted
    "###);

    // --revisions shouldn't be blocked by conflicting branch
    bump_branch1();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-rall()"]);
    insta::assert_snapshot!(stdout, @r###"
    Branch changes to push to origin:
      Move branch branch1 from fd1d63e031ea to 8263cf992d33
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Branch branch2 is conflicted
    "###);
}
