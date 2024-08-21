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

use std::path::Path;
use std::path::PathBuf;

use crate::common::TestEnvironment;

fn set_up() -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    test_env.jj_cmd_ok(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "branch1"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "root()", "-m=description 2"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "branch2"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    test_env.jj_cmd_ok(
        test_env.env_root(),
        &[
            "git",
            "clone",
            "--config-toml=git.auto-local-branch=true",
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
    // Show the setup. `insta` has trouble if this is done inside `set_up()`
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: xtvrqkyv d13ecdbd (empty) description 1
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);
    // No branches to push yet
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_current_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    // Update some branches. `branch1` is not a current branch, but `branch2` and
    // `my-branch` are.
    test_env.jj_cmd_ok(
        &workspace_root,
        &["describe", "branch1", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["new", "branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    // Check the setup
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: xtvrqkyv 96cc58bf (empty) modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv hidden d13ecdbd (empty) description 1
    branch2: yostqsxw bc7610b6 (empty) foo
      @origin (behind by 1 commits): rlzusymt 8476341e (empty) description 2
    my-branch: yostqsxw bc7610b6 (empty) foo
    "###);
    // First dry-run. `branch1` should not get pushed.
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch branch2 from 8476341eb395 to bc7610b65a91
      Add branch my-branch to bc7610b65a91
    Dry-run requested, not pushing.
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch branch2 from 8476341eb395 to bc7610b65a91
      Add branch my-branch to bc7610b65a91
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: xtvrqkyv 96cc58bf (empty) modified branch1 commit
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv hidden d13ecdbd (empty) description 1
    branch2: yostqsxw bc7610b6 (empty) foo
      @origin: yostqsxw bc7610b6 (empty) foo
    my-branch: yostqsxw bc7610b6 (empty) foo
      @origin: yostqsxw bc7610b6 (empty) foo
    "###);

    // Try pushing backwards
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "branch",
            "set",
            "branch2",
            "-rbranch2-",
            "--allow-backwards",
        ],
    );
    // This behavior is a strangeness of our definition of the default push revset.
    // We could consider changing it.
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches found in the default push revset: remote_branches(remote=origin)..@
    Nothing changed.
    "###);
    // We can move a branch backwards
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-bbranch2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move backward branch branch2 from bc7610b65a91 to 8476341eb395
    "###);
}

#[test]
fn test_git_push_parent_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    test_env.jj_cmd_ok(&workspace_root, &["edit", "branch1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["describe", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "non-empty description"]);
    std::fs::write(workspace_root.join("file"), "file").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch branch1 from d13ecdbda2a2 to 78bdfdea51c7
    "###);
}

#[test]
fn test_git_push_no_matching_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["new"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches found in the default push revset: remote_branches(remote=origin)..@
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_matching_branch_unchanged() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["new", "branch1"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches found in the default push revset: remote_branches(remote=origin)..@
    Nothing changed.
    "###);
}

/// Test that `jj git push` without arguments pushes a branch to the specified
/// remote even if it's already up to date on another remote
/// (`remote_branches(remote=<remote>)..@` vs. `remote_branches()..@`).
#[test]
fn test_git_push_other_remote_has_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    // Create another remote (but actually the same)
    let other_remote_path = test_env
        .env_root()
        .join("origin")
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "remote",
            "add",
            "other",
            other_remote_path.to_str().unwrap(),
        ],
    );
    // Modify branch1 and push it to `origin`
    test_env.jj_cmd_ok(&workspace_root, &["edit", "branch1"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m=modified"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch branch1 from d13ecdbda2a2 to a657f1b61b94
    "###);
    // Since it's already pushed to origin, nothing will happen if push again
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches found in the default push revset: remote_branches(remote=origin)..@
    Nothing changed.
    "###);
    // The branch was moved on the "other" remote as well (since it's actually the
    // same remote), but `jj` is not aware of that since it thinks this is a
    // different remote. So, the push should fail.
    //
    // But it succeeds! That's because the branch is created at the same location as
    // it is on the remote. This would also work for a descendant.
    //
    // TODO: Saner test?
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--remote=other"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to other:
      Add branch branch1 to a657f1b61b94
    "###);
}

#[test]
fn test_git_push_forward_unexpectedly_moved() {
    let (test_env, workspace_root) = set_up();

    // Move branch1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["new", "branch1", "-m=remote"]);
    std::fs::write(origin_path.join("remote"), "remote").unwrap();
    test_env.jj_cmd_ok(&origin_path, &["branch", "set", "branch1"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Move branch1 forward to another commit locally
    test_env.jj_cmd_ok(&workspace_root, &["new", "branch1", "-m=local"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch1"]);

    // Pushing should fail
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch branch1 from d13ecdbda2a2 to 6750425ff51c
    Error: Refusing to push a branch that unexpectedly moved on the remote. Affected refs: refs/heads/branch1
    Hint: Try fetching from the remote, then make the branch point to where you want it to be, and push again.
    "###);
}

#[test]
fn test_git_push_sideways_unexpectedly_moved() {
    let (test_env, workspace_root) = set_up();

    // Move branch1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["new", "branch1", "-m=remote"]);
    std::fs::write(origin_path.join("remote"), "remote").unwrap();
    test_env.jj_cmd_ok(&origin_path, &["branch", "set", "branch1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &origin_path), @r###"
    branch1: vruxwmqv 80284bec remote
      @git (behind by 1 commits): qpvuntsm d13ecdbd (empty) description 1
    branch2: zsuskuln 8476341e (empty) description 2
      @git: zsuskuln 8476341e (empty) description 2
    "###);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Move branch1 sideways to another commit locally
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=local"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &["branch", "set", "branch1", "--allow-backwards"],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: kmkuslsw 0f8bf988 local
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv d13ecdbd (empty) description 1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch branch1 from d13ecdbda2a2 to 0f8bf988588e
    Error: Refusing to push a branch that unexpectedly moved on the remote. Affected refs: refs/heads/branch1
    Hint: Try fetching from the remote, then make the branch point to where you want it to be, and push again.
    "###);
}

// This tests whether the push checks that the remote branches are in expected
// positions.
#[test]
fn test_git_push_deletion_unexpectedly_moved() {
    let (test_env, workspace_root) = set_up();

    // Move branch1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["new", "branch1", "-m=remote"]);
    std::fs::write(origin_path.join("remote"), "remote").unwrap();
    test_env.jj_cmd_ok(&origin_path, &["branch", "set", "branch1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &origin_path), @r###"
    branch1: vruxwmqv 80284bec remote
      @git (behind by 1 commits): qpvuntsm d13ecdbd (empty) description 1
    branch2: zsuskuln 8476341e (empty) description 2
      @git: zsuskuln 8476341e (empty) description 2
    "###);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Delete branch1 locally
    test_env.jj_cmd_ok(&workspace_root, &["branch", "delete", "branch1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1 (deleted)
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch", "branch1"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
    Error: Refusing to push a branch that unexpectedly moved on the remote. Affected refs: refs/heads/branch1
    Hint: Try fetching from the remote, then make the branch point to where you want it to be, and push again.
    "###);
}

#[test]
fn test_git_push_unexpectedly_deleted() {
    let (test_env, workspace_root) = set_up();

    // Delete branch1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["branch", "delete", "branch1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &origin_path), @r###"
    branch1 (deleted)
      @git: qpvuntsm d13ecdbd (empty) description 1
    branch2: zsuskuln 8476341e (empty) description 2
      @git: zsuskuln 8476341e (empty) description 2
    "###);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Move branch1 sideways to another commit locally
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=local"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &["branch", "set", "branch1", "--allow-backwards"],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: kpqxywon 1ebe27ba local
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv d13ecdbd (empty) description 1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    // Pushing a moved branch fails if deleted on remote
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch branch1 from d13ecdbda2a2 to 1ebe27ba04bf
    Error: Refusing to push a branch that unexpectedly moved on the remote. Affected refs: refs/heads/branch1
    Hint: Try fetching from the remote, then make the branch point to where you want it to be, and push again.
    "###);

    test_env.jj_cmd_ok(&workspace_root, &["branch", "delete", "branch1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1 (deleted)
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);
    // Pushing a *deleted* branch succeeds if deleted on remote, even if we expect
    // branch1@origin to exist and point somewhere.
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-bbranch1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
    "###);
}

#[test]
fn test_git_push_creation_unexpectedly_already_exists() {
    let (test_env, workspace_root) = set_up();

    // Forget branch1 locally
    test_env.jj_cmd_ok(&workspace_root, &["branch", "forget", "branch1"]);

    // Create a new branh1
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=new branch1"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: yostqsxw 4c595cf9 new branch1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch branch1 to 4c595cf9ac0a
    Error: Refusing to push a branch that unexpectedly moved on the remote. Affected refs: refs/heads/branch1
    Hint: Try fetching from the remote, then make the branch point to where you want it to be, and push again.
    "###);
}

#[test]
fn test_git_push_locally_created_and_rewritten() {
    let (test_env, workspace_root) = set_up();
    // Ensure that remote branches aren't tracked automatically
    test_env.add_config("git.auto-local-branch = false");

    // Push locally-created branch
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-mlocal 1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch my to fcc999921ce9
    "###);

    // Rewrite it and push again, which would fail if the pushed branch weren't
    // set to "tracking"
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-mlocal 2"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: xtvrqkyv d13ecdbd (empty) description 1
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    my: vruxwmqv bde1d2e4 (empty) local 2
      @origin (ahead by 1 commits, behind by 1 commits): vruxwmqv hidden fcc99992 (empty) local 1
    "###);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch my from fcc999921ce9 to bde1d2e44b2a
    "###);
}

#[test]
fn test_git_push_multiple() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "delete", "branch1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["branch", "set", "--allow-backwards", "branch2"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    // Check the setup
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1 (deleted)
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2: yqosqzyt c4a3c310 (empty) foo
      @origin (ahead by 1 commits, behind by 1 commits): rlzusymt 8476341e (empty) description 2
    my-branch: yqosqzyt c4a3c310 (empty) foo
    "###);
    // First dry-run
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
      Move sideways branch branch2 from 8476341eb395 to c4a3c3105d92
      Add branch my-branch to c4a3c3105d92
    Dry-run requested, not pushing.
    "###);
    // Dry run requesting two specific branches
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-b=branch1", "-b=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
      Add branch my-branch to c4a3c3105d92
    Dry-run requested, not pushing.
    "###);
    // Dry run requesting two specific branches twice
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "-b=branch1",
            "-b=my-branch",
            "-b=branch1",
            "-b=glob:my-*",
            "--dry-run",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
      Add branch my-branch to c4a3c3105d92
    Dry-run requested, not pushing.
    "###);
    // Dry run with glob pattern
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-b=glob:branch?", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
      Move sideways branch branch2 from 8476341eb395 to c4a3c3105d92
    Dry-run requested, not pushing.
    "###);

    // Unmatched branch name is error
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-b=foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such branch: foo
    "###);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "-b=foo", "-b=glob:?branch"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: No matching branches for patterns: foo, ?branch
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
      Move sideways branch branch2 from 8476341eb395 to c4a3c3105d92
      Add branch my-branch to c4a3c3105d92
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch2: yqosqzyt c4a3c310 (empty) foo
      @origin: yqosqzyt c4a3c310 (empty) foo
    my-branch: yqosqzyt c4a3c310 (empty) foo
      @origin: yqosqzyt c4a3c310 (empty) foo
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-rall()"]);
    insta::assert_snapshot!(stdout, @r#"
    @  yqosqzyt test.user@example.com 2001-02-03 08:05:17 branch2 my-branch c4a3c310
    │  (empty) foo
    │ ◌  rlzusymt test.user@example.com 2001-02-03 08:05:10 8476341e
    ├─╯  (empty) description 2
    │ ◌  xtvrqkyv test.user@example.com 2001-02-03 08:05:08 d13ecdbd
    ├─╯  (empty) description 1
    ◆  zzzzzzzz root() 00000000
    "#);
}

#[test]
fn test_git_push_changes() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "bar"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Creating branch push-yostqsxwqrlt for revision yostqsxwqrlt
    Branch changes to push to origin:
      Add branch push-yostqsxwqrlt to cf1a53a8800a
    "###);
    // test pushing two changes at once
    std::fs::write(workspace_root.join("file"), "modified2").unwrap();
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-c=(@|@-)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "(@|@-)" resolved to more than one revision
    Hint: The revset "(@|@-)" resolved to these revisions:
      yostqsxw 16c16966 push-yostqsxwqrlt* | bar
      yqosqzyt a050abf4 foo
    Hint: Prefix the expression with 'all:' to allow any number of revisions (i.e. 'all:(@|@-)').
    "###);
    // test pushing two changes at once, part 2
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-c=all:(@|@-)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Creating branch push-yqosqzytrlsw for revision yqosqzytrlsw
    Branch changes to push to origin:
      Move sideways branch push-yostqsxwqrlt from cf1a53a8800a to 16c169664e9f
      Add branch push-yqosqzytrlsw to a050abf4ff07
    "###);
    // specifying the same change twice doesn't break things
    std::fs::write(workspace_root.join("file"), "modified3").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-c=all:(@|@)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch push-yostqsxwqrlt from 16c169664e9f to ef6313d50ac1
    "###);

    // specifying the same branch with --change/--branch doesn't break things
    std::fs::write(workspace_root.join("file"), "modified4").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-c=@", "-b=push-yostqsxwqrlt"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch push-yostqsxwqrlt from ef6313d50ac1 to c1e65d3a64ce
    "###);

    // try again with --change that moves the branch forward
    std::fs::write(workspace_root.join("file"), "modified5").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "branch",
            "set",
            "-r=@-",
            "--allow-backwards",
            "push-yostqsxwqrlt",
        ],
    );
    let stdout = test_env.jj_cmd_success(&workspace_root, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    M file
    Working copy : yostqsxw 38cb417c bar
    Parent commit: yqosqzyt a050abf4 push-yostqsxwqrlt* push-yqosqzytrlsw | foo
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-c=@", "-b=push-yostqsxwqrlt"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move sideways branch push-yostqsxwqrlt from c1e65d3a64ce to 38cb417ce3a6
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    M file
    Working copy : yostqsxw 38cb417c push-yostqsxwqrlt | bar
    Parent commit: yqosqzyt a050abf4 push-yqosqzytrlsw | foo
    "###);

    // Test changing `git.push-branch-prefix`. It causes us to push again.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "--config-toml",
            r"git.push-branch-prefix='test-'",
            "--change=@",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Creating branch test-yostqsxwqrlt for revision yostqsxwqrlt
    Branch changes to push to origin:
      Add branch test-yostqsxwqrlt to 38cb417ce3a6
    "###);
}

#[test]
fn test_git_push_revisions() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "bar"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch-1"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "baz"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch-2a"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch-2b"]);
    std::fs::write(workspace_root.join("file"), "modified again").unwrap();

    // Push an empty set
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=none()"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches point to the specified revisions: none()
    Nothing changed.
    "###);
    // Push a revision with no branches
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=@--"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches point to the specified revisions: @--
    Nothing changed.
    "###);
    // Push a revision with a single branch
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=@-", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch branch-1 to 5f432a855e59
    Dry-run requested, not pushing.
    "###);
    // Push multiple revisions of which some have branches
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-r=@--", "-r=@-", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branches point to the specified revisions: @--
    Branch changes to push to origin:
      Add branch branch-1 to 5f432a855e59
    Dry-run requested, not pushing.
    "###);
    // Push a revision with a multiple branches
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=@", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch branch-2a to 84f499037f5c
      Add branch branch-2b to 84f499037f5c
    Dry-run requested, not pushing.
    "###);
    // Repeating a commit doesn't result in repeated messages about the branch
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-r=@-", "-r=@-", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch branch-1 to 5f432a855e59
    Dry-run requested, not pushing.
    "###);
}

#[test]
fn test_git_push_mixed() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "bar"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch-1"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "baz"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch-2a"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch-2b"]);
    std::fs::write(workspace_root.join("file"), "modified again").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--change=@--", "--branch=branch-1", "-r=@"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Creating branch push-yqosqzytrlsw for revision yqosqzytrlsw
    Branch changes to push to origin:
      Add branch push-yqosqzytrlsw to a050abf4ff07
      Add branch branch-1 to 5f432a855e59
      Add branch branch-2a to 84f499037f5c
      Add branch branch-2b to 84f499037f5c
    "###);
}

#[test]
fn test_git_push_existing_long_branch() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &["branch", "create", "push-19b790168e73f7a73a98deae21e807c0"],
    );

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change=@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch push-19b790168e73f7a73a98deae21e807c0 to a050abf4ff07
    "###);
}

#[test]
fn test_git_push_unsnapshotted_change() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change", "@"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change", "@"]);
}

#[test]
fn test_git_push_conflict() {
    let (test_env, workspace_root) = set_up();
    std::fs::write(workspace_root.join("file"), "first").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["commit", "-m", "first"]);
    std::fs::write(workspace_root.join("file"), "second").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["commit", "-m", "second"]);
    std::fs::write(workspace_root.join("file"), "third").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["rebase", "-r", "@", "-d", "@--"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "third"]);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 73c265a92cfd since it has conflicts
    "###);
}

#[test]
fn test_git_push_no_description() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m="]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch", "my-branch"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 5b36783cd11c since it has no description
    "###);
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "--branch",
            "my-branch",
            "--allow-empty-description",
        ],
    );
}

#[test]
fn test_git_push_no_description_in_immutable() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "imm"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m="]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);

    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 5b36783cd11c since it has no description
    "###);

    test_env.add_config(r#"revset-aliases."immutable_heads()" = "imm""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--branch=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch my-branch to ea7373507ad9
    Dry-run requested, not pushing.
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
    run_without_var("JJ_USER", &["checkout", "root()", "-m=initial"]);
    run_without_var("JJ_USER", &["branch", "create", "missing-name"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-name"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 944313939bbd since it has no author and/or committer set
    "###);
    run_without_var("JJ_EMAIL", &["checkout", "root()", "-m=initial"]);
    run_without_var("JJ_EMAIL", &["branch", "create", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 59354714f789 since it has no author and/or committer set
    "###);
}

#[test]
fn test_git_push_missing_author_in_immutable() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    run_without_var("JJ_USER", &["new", "root()", "-m=no author name"]);
    run_without_var("JJ_EMAIL", &["new", "-m=no author email"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "imm"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);

    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 011f740bf8b5 since it has no author and/or committer set
    "###);

    test_env.add_config(r#"revset-aliases."immutable_heads()" = "imm""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--branch=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch my-branch to 68fdae89de4f
    Dry-run requested, not pushing.
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
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "missing-name"]);
    run_without_var("JJ_USER", &["describe", "-m=no committer name"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-name"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 4fd190283d1a since it has no author and/or committer set
    "###);
    test_env.jj_cmd_ok(&workspace_root, &["checkout", "root()"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit eab97428a6ec since it has no author and/or committer set
    "###);

    // Test message when there are multiple reasons (missing committer and
    // description)
    run_without_var("JJ_EMAIL", &["describe", "-m=", "missing-email"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=missing-email"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 1143ed607f54 since it has no description and it has no author and/or committer set
    "###);
}

#[test]
fn test_git_push_missing_committer_in_immutable() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    run_without_var("JJ_USER", &["describe", "-m=no committer name"]);
    test_env.jj_cmd_ok(&workspace_root, &["new"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "imm"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "my-branch"]);

    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 7e61dc727a8f since it has no author and/or committer set
    "###);

    test_env.add_config(r#"revset-aliases."immutable_heads()" = "imm""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--branch=my-branch", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch my-branch to c79f85e90b4a
    Dry-run requested, not pushing.
    "###);
}

#[test]
fn test_git_push_deleted() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["branch", "delete", "branch1"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch1 from d13ecdbda2a2
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-rall()"]);
    insta::assert_snapshot!(stdout, @r#"
    ◌  rlzusymt test.user@example.com 2001-02-03 08:05:10 branch2 8476341e
    │  (empty) description 2
    │ ◌  xtvrqkyv test.user@example.com 2001-02-03 08:05:08 d13ecdbd
    ├─╯  (empty) description 1
    │ @  yqosqzyt test.user@example.com 2001-02-03 08:05:13 5b36783c
    ├─╯  (empty) (no description set)
    ◆  zzzzzzzz root() 00000000
    "#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_conflicting_branches() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config("git.auto-local-branch = true");
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
    test_env.jj_cmd_ok(&workspace_root, &["git", "import"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=description 3"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["git", "fetch"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: xtvrqkyv d13ecdbd (empty) description 1
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2 (conflicted):
      + yostqsxw 8e670e2d (empty) description 3
      + rlzusymt 8476341e (empty) description 2
      @origin (behind by 1 commits): rlzusymt 8476341e (empty) description 2
    "###);

    let bump_branch1 = || {
        test_env.jj_cmd_ok(&workspace_root, &["new", "branch1", "-m=bump"]);
        test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch1"]);
    };

    // Conflicting branch at @
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: Branch branch2 is conflicted
    Hint: Run `jj branch list` to inspect, and use `jj branch set` to fix it up.
    Nothing changed.
    "###);

    // --branch should be blocked by conflicting branch
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch", "branch2"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Branch branch2 is conflicted
    Hint: Run `jj branch list` to inspect, and use `jj branch set` to fix it up.
    "###);

    // --all shouldn't be blocked by conflicting branch
    bump_branch1();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: Branch branch2 is conflicted
    Hint: Run `jj branch list` to inspect, and use `jj branch set` to fix it up.
    Branch changes to push to origin:
      Move forward branch branch1 from d13ecdbda2a2 to 8df52121b022
    "###);

    // --revisions shouldn't be blocked by conflicting branch
    bump_branch1();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-rall()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: Branch branch2 is conflicted
    Hint: Run `jj branch list` to inspect, and use `jj branch set` to fix it up.
    Branch changes to push to origin:
      Move forward branch branch1 from 8df52121b022 to 345e1f64a64d
    "###);
}

#[test]
fn test_git_push_deleted_untracked() {
    let (test_env, workspace_root) = set_up();

    // Absent local branch shouldn't be considered "deleted" compared to
    // non-tracking remote branch.
    test_env.jj_cmd_ok(&workspace_root, &["branch", "delete", "branch1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "untrack", "branch1@origin"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch=branch1"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such branch: branch1
    "###);
}

#[test]
fn test_git_push_tracked_vs_all() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["new", "branch1", "-mmoved branch1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch1"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "branch2", "-mmoved branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "delete", "branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "untrack", "branch1@origin"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch3"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: vruxwmqv 0b16bb02 (empty) moved branch1
    branch1@origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2 (deleted)
      @origin: rlzusymt 8476341e (empty) description 2
    branch3: znkkpsqq 998d6a78 (empty) moved branch2
    "###);

    // At this point, only branch2 is still tracked. `jj git push --tracked` would
    // try to push it and no other branches.
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--tracked", "--dry-run"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Delete branch branch2 from 8476341eb395
    Dry-run requested, not pushing.
    "###);

    // Untrack the last remaining tracked branch.
    test_env.jj_cmd_ok(&workspace_root, &["branch", "untrack", "branch2@origin"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &workspace_root), @r###"
    branch1: vruxwmqv 0b16bb02 (empty) moved branch1
    branch1@origin: xtvrqkyv d13ecdbd (empty) description 1
    branch2@origin: rlzusymt 8476341e (empty) description 2
    branch3: znkkpsqq 998d6a78 (empty) moved branch2
    "###);

    // Now, no branches are tracked. --tracked does not push anything
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--tracked"]);
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // All branches are still untracked.
    // - --all tries to push branch1, but fails because a branch with the same
    // name exist on the remote.
    // - --all succeeds in pushing branch3, since there is no branch of the same
    // name on the remote.
    // - It does not try to push branch2.
    //
    // TODO: Not trying to push branch2 could be considered correct, or perhaps
    // we want to consider this as a deletion of the branch that failed because
    // the branch was untracked. In the latter case, an error message should be
    // printed. Some considerations:
    // - Whatever we do should be consistent with what `jj branch list` does; it
    //   currently does *not* list branches like branch2 as "about to be deleted",
    //   as can be seen above.
    // - We could consider showing some hint on `jj branch untrack branch2@origin`
    //   instead of showing an error here.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Non-tracking remote branch branch1@origin exists
    Hint: Run `jj branch track branch1@origin` to import the remote branch.
    Branch changes to push to origin:
      Add branch branch3 to 998d6a7853d9
    "###);
}

#[test]
fn test_git_push_moved_forward_untracked() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "branch1", "-mmoved branch1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "untrack", "branch1@origin"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Non-tracking remote branch branch1@origin exists
    Hint: Run `jj branch track branch1@origin` to import the remote branch.
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_moved_sideways_untracked() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-mmoved branch1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["branch", "set", "--allow-backwards", "branch1"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["branch", "untrack", "branch1@origin"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Non-tracking remote branch branch1@origin exists
    Hint: Run `jj branch track branch1@origin` to import the remote branch.
    Nothing changed.
    "###);
}

#[test]
// TODO: This test fails with libgit2 v1.8.1 on Windows.
#[cfg(not(target_os = "windows"))]
fn test_git_push_to_remote_named_git() {
    let (test_env, workspace_root) = set_up();
    let git_repo = {
        let mut git_repo_path = workspace_root.clone();
        git_repo_path.extend([".jj", "repo", "store", "git"]);
        git2::Repository::open(&git_repo_path).unwrap()
    };
    git_repo.remote_rename("origin", "git").unwrap();

    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--all", "--remote=git"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to git:
      Add branch branch1 to d13ecdbda2a2
      Add branch branch2 to 8476341eb395
    Error: Git remote named 'git' is reserved for local Git repository
    "###);
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    // --quiet to suppress deleted branches hint
    test_env.jj_cmd_success(repo_path, &["branch", "list", "--all-remotes", "--quiet"])
}
