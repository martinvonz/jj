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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_branch_multiple_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    warning: Creating multiple branches: foo, bar
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 230dd059e1b0
    ◉   000000000000
    "###);

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "set", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    warning: Updating multiple branches: foo, bar
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 8bb159bc30a9
    ◉   230dd059e1b0
    ◉   000000000000
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "foo", "bar", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Deleted 2 branches.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   8bb159bc30a9
    ◉   230dd059e1b0
    ◉   000000000000
    "###);
}

#[test]
fn test_branch_at_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["branch", "create", "fred", "-r=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    Failed to export some branches:
      fred
    "###);
}

#[test]
fn test_branch_empty_name() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["branch", "create", ""]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '<NAMES>...' but none was supplied

    For more information, try '--help'.
    "###);
}

#[test]
fn test_branch_move() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "set", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such branch: foo
    Hint: Use `jj branch create` to create it.
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    insta::assert_snapshot!(stderr, @"");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "create", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Branch already exists: foo
    Hint: Use `jj branch set` to update it.
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "set", "foo"]);
    insta::assert_snapshot!(stderr, @"");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "set", "-r@-", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to move branch backwards or sideways.
    Hint: Use --allow-backwards to allow it.
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["branch", "set", "-r@-", "--allow-backwards", "foo"],
    );
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_branch_forget_glob() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo-1"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "bar-2"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo-3"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo-4"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 230dd059e1b0
    ◉   000000000000
    "###);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "--glob", "foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    --glob has been deprecated. Please prefix the pattern with `glob:` instead.
    Forgot 2 branches.
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Forgot 2 branches.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-4 230dd059e1b0
    ◉   000000000000
    "###);

    // Forgetting a branch via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["branch", "forget", "foo-4", "--glob", "foo-*", "glob:foo-*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    --glob has been deprecated. Please prefix the pattern with `glob:` instead.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 230dd059e1b0
    ◉   000000000000
    "###);

    // Malformed glob
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["branch", "forget", "glob:foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'glob:foo-[1-3' for '[NAMES]...': Pattern syntax error near position 4: invalid range pattern

    For more information, try '--help'.
    "###);

    // We get an error if none of the globs match anything
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["branch", "forget", "glob:bar*", "glob:baz*", "--glob=boom*"],
    );
    insta::assert_snapshot!(stderr, @r###"
    --glob has been deprecated. Please prefix the pattern with `glob:` instead.
    Error: No matching branches for patterns: baz*, boom*
    "###);
}

#[test]
fn test_branch_delete_glob() {
    // Set up a git repo with a branch and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=commit"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo-1"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "bar-2"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo-3"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo-4"]);
    // Push to create remote-tracking branches
    test_env.jj_cmd_ok(&repo_path, &["git", "push", "--all"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 6fbf398c2d59
    ◉   000000000000
    "###);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "--glob", "foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    --glob has been deprecated. Please prefix the pattern with `glob:` instead.
    Deleted 2 branches.
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Deleted 2 branches.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1@origin foo-3@origin foo-4 6fbf398c2d59
    ◉   000000000000
    "###);

    // We get an error if none of the globs match live branches. Unlike `jj branch
    // forget`, it's not allowed to delete already deleted branches.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "delete", "--glob=foo-[1-3]"]);
    insta::assert_snapshot!(stderr, @r###"
    --glob has been deprecated. Please prefix the pattern with `glob:` instead.
    Error: No matching branches for patterns: foo-[1-3]
    "###);

    // Deleting a branch via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["branch", "delete", "foo-4", "--glob", "foo-*", "glob:foo-*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    --glob has been deprecated. Please prefix the pattern with `glob:` instead.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1@origin foo-3@origin foo-4@origin 6fbf398c2d59
    ◉   000000000000
    "###);

    // The deleted branches are still there
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    bar-2: qpvuntsm 6fbf398c (empty) commit
      @origin: qpvuntsm 6fbf398c (empty) commit
    foo-1 (deleted)
      @origin: qpvuntsm 6fbf398c (empty) commit
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    foo-3 (deleted)
      @origin: qpvuntsm 6fbf398c (empty) commit
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    foo-4 (deleted)
      @origin: qpvuntsm 6fbf398c (empty) commit
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    "###);

    // Malformed glob
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["branch", "delete", "glob:foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'glob:foo-[1-3' for '[NAMES]...': Pattern syntax error near position 4: invalid range pattern

    For more information, try '--help'.
    "###);

    // Unknown pattern kind
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["branch", "forget", "whatever:branch"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'whatever:branch' for '[NAMES]...': Invalid string pattern kind "whatever"

    For more information, try '--help'.
    "###);
}

#[test]
fn test_branch_delete_export() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);

    test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "foo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    foo (deleted)
      @git: rlvkpnrz 65b6b74e (empty) (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
}

#[test]
fn test_branch_forget_export() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    foo: rlvkpnrz 65b6b74e (empty) (no description set)
    "###);

    // Exporting the branch to git creates a local-git tracking branch
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    // Forgetting a branch deletes local and remote-tracking branches including
    // the corresponding git-tracking branch.
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r=foo", "--no-graph"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "foo" doesn't exist
    "###);

    // `jj git export` will delete the branch from git. In a colocated repo,
    // this will happen automatically immediately after a `jj branch forget`.
    // This is demonstrated in `test_git_colocated_branch_forget` in
    // test_git_colocated.rs
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @"");
}

#[test]
fn test_branch_forget_fetched_branch() {
    // Much of this test is borrowed from `test_git_fetch_remote_only_branch` in
    // test_git_fetch.rs

    // Set up a git repo with a branch and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    // Create a commit and a branch in the git repo
    let first_git_repo_commit = git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();

    // Fetch normally
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 1: with export-import
    // Forget the branch
    test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // At this point `jj git export && jj git import` does *not* recreate the
    // branch. This behavior is important in colocated repos, as otherwise a
    // forgotten branch would be immediately resurrected.
    //
    // Technically, this is because `jj branch forget` preserved
    // the ref in jj view's `git_refs` tracking the local git repo's remote-tracking
    // branch.
    // TODO: Show that jj git push is also a no-op
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "import"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // We can fetch feature1 again.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 2: No export/import (otherwise the same as test 1)
    test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
    // Fetch works even without the export-import
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 3: fetch branch that was moved & forgotten

    // Move the branch in the git repo.
    git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "another message",
            &tree,
            &[&git_repo.find_commit(first_git_repo_commit).unwrap()],
        )
        .unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");

    // Fetching a moved branch does not create a conflict
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: ooosovrs 38aefb17 (empty) another message
      @origin: ooosovrs 38aefb17 (empty) another message
    "###);
}

#[test]
fn test_branch_forget_deleted_or_nonexistent_branch() {
    // Much of this test is borrowed from `test_git_fetch_remote_only_branch` in
    // test_git_fetch.rs

    // ======== Beginning of test setup ========
    // Set up a git repo with a branch and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    // Create a commit and a branch in the git repo
    git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();

    // Fetch and then delete the branch
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1 (deleted)
      @origin: mzyxwzks 9f01a0e0 message
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    "###);

    // ============ End of test setup ============

    // We can forget a deleted branch
    test_env.jj_cmd_ok(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // Can't forget a non-existent branch
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "forget", "i_do_not_exist"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such branch: i_do_not_exist
    "###);
}

#[test]
fn test_branch_track_untrack() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up remote
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    let create_remote_commit = |message: &str, data: &[u8], ref_names: &[&str]| {
        let signature =
            git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0))
                .unwrap();
        let mut tree_builder = git_repo.treebuilder(None).unwrap();
        let file_oid = git_repo.blob(data).unwrap();
        tree_builder
            .insert("file", file_oid, git2::FileMode::Blob.into())
            .unwrap();
        let tree_oid = tree_builder.write().unwrap();
        let tree = git_repo.find_tree(tree_oid).unwrap();
        // Create commit and branches in the remote
        let git_commit_oid = git_repo
            .commit(None, &signature, &signature, message, &tree, &[])
            .unwrap();
        for name in ref_names {
            git_repo.reference(name, git_commit_oid, true, "").unwrap();
        }
    };

    // Fetch new commit without auto tracking. No local branches should be
    // created.
    create_remote_commit(
        "commit 1",
        b"content 1",
        &[
            "refs/heads/main",
            "refs/heads/feature1",
            "refs/heads/feature2",
        ],
    );
    test_env.add_config("git.auto-local-branch = false");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1@origin: sptzoqmo 7b33f629 commit 1
    feature2@origin: sptzoqmo 7b33f629 commit 1
    main@origin: sptzoqmo 7b33f629 commit 1
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  feature1@origin feature2@origin main@origin 7b33f6295eda
    │ @   230dd059e1b0
    ├─╯
    ◉   000000000000
    "###);

    // Track new branch. Local branch should be created.
    test_env.jj_cmd_ok(
        &repo_path,
        &["branch", "track", "feature1@origin", "main@origin"],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    feature2@origin: sptzoqmo 7b33f629 commit 1
    main: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    "###);

    // Track existing branch. Local branch should result in conflict.
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "feature2"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "track", "feature2@origin"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    feature2 (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + sptzoqmo 7b33f629 commit 1
      @origin (behind by 1 commits): sptzoqmo 7b33f629 commit 1
    main: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    "###);

    // Untrack existing and locally-deleted branches. Branch targets should be
    // unchanged
    test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "feature2"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["branch", "untrack", "feature1@origin", "feature2@origin"],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
    feature1@origin: sptzoqmo 7b33f629 commit 1
    feature2@origin: sptzoqmo 7b33f629 commit 1
    main: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  feature1 feature1@origin feature2@origin main 7b33f6295eda
    │ @   230dd059e1b0
    ├─╯
    ◉   000000000000
    "###);

    // Fetch new commit. Only tracking branch "main" should be merged.
    create_remote_commit(
        "commit 2",
        b"content 2",
        &[
            "refs/heads/main",
            "refs/heads/feature1",
            "refs/heads/feature2",
        ],
    );
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
    feature1@origin: mmqqkyyt 40dabdaf commit 2
    feature2@origin: mmqqkyyt 40dabdaf commit 2
    main: mmqqkyyt 40dabdaf commit 2
      @origin: mmqqkyyt 40dabdaf commit 2
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  feature1@origin feature2@origin main 40dabdaf4abe
    │ ◉  feature1 7b33f6295eda
    ├─╯
    │ @   230dd059e1b0
    ├─╯
    ◉   000000000000
    "###);

    // Fetch new commit with auto tracking. Tracking branch "main" and new
    // branch "feature3" should be merged.
    create_remote_commit(
        "commit 3",
        b"content 3",
        &[
            "refs/heads/main",
            "refs/heads/feature1",
            "refs/heads/feature2",
            "refs/heads/feature3",
        ],
    );
    test_env.add_config("git.auto-local-branch = true");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 1 commits that are no longer reachable.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
    feature1@origin: wwnpyzpo 3f0f86fa commit 3
    feature2@origin: wwnpyzpo 3f0f86fa commit 3
    feature3: wwnpyzpo 3f0f86fa commit 3
      @origin: wwnpyzpo 3f0f86fa commit 3
    main: wwnpyzpo 3f0f86fa commit 3
      @origin: wwnpyzpo 3f0f86fa commit 3
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  feature1@origin feature2@origin feature3 main 3f0f86fa0e57
    │ ◉  feature1 7b33f6295eda
    ├─╯
    │ @   230dd059e1b0
    ├─╯
    ◉   000000000000
    "###);
}

#[test]
fn test_branch_track_untrack_patterns() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up remote
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    // Create remote commit
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    // Create commit and branches in the remote
    let git_commit_oid = git_repo
        .commit(None, &signature, &signature, "commit", &tree, &[])
        .unwrap();
    for name in ["refs/heads/feature1", "refs/heads/feature2"] {
        git_repo.reference(name, git_commit_oid, true, "").unwrap();
    }

    // Fetch new commit without auto tracking
    test_env.add_config("git.auto-local-branch = false");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @"");

    // Track local branch
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_cli_error(&repo_path, &["branch", "track", "main"]), @r###"
    error: invalid value 'main' for '<NAMES>...': remote branch must be specified in branch@remote form

    For more information, try '--help'.
    "###);

    // Track/untrack unknown branch
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["branch", "track", "main@origin"]), @r###"
    Error: No such remote branch: main@origin
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["branch", "untrack", "main@origin"]), @r###"
    Error: No such remote branch: main@origin
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["branch", "track", "glob:maine@*"]), @r###"
    Error: No matching remote branches for patterns: maine@*
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(
            &repo_path,
            &["branch", "untrack", "main@origin", "glob:main@o*"],
        ), @r###"
    Error: No matching remote branches for patterns: main@origin, main@o*
    "###);

    // Track already tracked branch
    test_env.jj_cmd_ok(&repo_path, &["branch", "track", "feature1@origin"]);
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "track", "feature1@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Remote branch already tracked: feature1@origin
    Nothing changed.
    "###);

    // Untrack non-tracking branch
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "untrack", "feature2@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Remote branch not tracked yet: feature2@origin
    Nothing changed.
    "###);

    // Untrack Git-tracking branch
    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "untrack", "main@git"]);
    insta::assert_snapshot!(stderr, @r###"
    Git-tracking branch cannot be untracked: main@git
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: omvolwpu 1336caed commit
      @git: omvolwpu 1336caed commit
      @origin: omvolwpu 1336caed commit
    feature2@origin: omvolwpu 1336caed commit
    main: qpvuntsm 230dd059 (empty) (no description set)
      @git: qpvuntsm 230dd059 (empty) (no description set)
    "###);

    // Untrack by pattern
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "untrack", "glob:*@*"]);
    insta::assert_snapshot!(stderr, @r###"
    Git-tracking branch cannot be untracked: feature1@git
    Remote branch not tracked yet: feature2@origin
    Git-tracking branch cannot be untracked: main@git
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: omvolwpu 1336caed commit
      @git: omvolwpu 1336caed commit
    feature1@origin: omvolwpu 1336caed commit
    feature2@origin: omvolwpu 1336caed commit
    main: qpvuntsm 230dd059 (empty) (no description set)
      @git: qpvuntsm 230dd059 (empty) (no description set)
    "###);

    // Track by pattern
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "track", "glob:feature?@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Started tracking 2 remote branches.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: omvolwpu 1336caed commit
      @git: omvolwpu 1336caed commit
      @origin: omvolwpu 1336caed commit
    feature2: omvolwpu 1336caed commit
      @origin: omvolwpu 1336caed commit
    main: qpvuntsm 230dd059 (empty) (no description set)
      @git: qpvuntsm 230dd059 (empty) (no description set)
    "###);
}

#[test]
fn test_branch_list() {
    let test_env = TestEnvironment::default();

    // Initialize remote refs
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "remote", "--git"]);
    let remote_path = test_env.env_root().join("remote");
    for branch in [
        "remote-sync",
        "remote-unsync",
        "remote-untrack",
        "remote-delete",
    ] {
        test_env.jj_cmd_ok(&remote_path, &["new", "root()", "-m", branch]);
        test_env.jj_cmd_ok(&remote_path, &["branch", "create", branch]);
    }
    test_env.jj_cmd_ok(&remote_path, &["new"]);
    test_env.jj_cmd_ok(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = remote_path;
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", remote_git_path.to_str().unwrap(), "local"],
    );
    let local_path = test_env.env_root().join("local");
    test_env.jj_cmd_ok(&local_path, &["new", "root()", "-m", "local-only"]);
    test_env.jj_cmd_ok(&local_path, &["branch", "create", "local-only"]);

    // Mutate refs in local repository
    test_env.jj_cmd_ok(&local_path, &["branch", "delete", "remote-delete"]);
    test_env.jj_cmd_ok(&local_path, &["branch", "delete", "remote-untrack"]);
    test_env.jj_cmd_ok(&local_path, &["branch", "untrack", "remote-untrack@origin"]);
    test_env.jj_cmd_ok(
        &local_path,
        &["branch", "set", "--allow-backwards", "remote-unsync"],
    );

    // Synchronized tracking remotes and non-tracking remotes aren't listed by
    // default
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&local_path, &["branch", "list"]), @r###"
    local-only: wqnwkozp 4e887f78 (empty) local-only
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: wqnwkozp 4e887f78 (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&local_path, &["branch", "list", "--all"]), @r###"
    local-only: wqnwkozp 4e887f78 (empty) local-only
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
      @origin: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: wqnwkozp 4e887f78 (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    remote-untrack@origin: vmortlor 71a16b05 (empty) remote-untrack
    "###);
}

#[test]
fn test_branch_list_filtered() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);

    // Initialize remote refs
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "remote", "--git"]);
    let remote_path = test_env.env_root().join("remote");
    for branch in ["remote-keep", "remote-delete", "remote-rewrite"] {
        test_env.jj_cmd_ok(&remote_path, &["new", "root()", "-m", branch]);
        test_env.jj_cmd_ok(&remote_path, &["branch", "create", branch]);
    }
    test_env.jj_cmd_ok(&remote_path, &["new"]);
    test_env.jj_cmd_ok(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = remote_path;
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", remote_git_path.to_str().unwrap(), "local"],
    );
    let local_path = test_env.env_root().join("local");
    test_env.jj_cmd_ok(&local_path, &["new", "root()", "-m", "local-keep"]);
    test_env.jj_cmd_ok(&local_path, &["branch", "create", "local-keep"]);

    // Mutate refs in local repository
    test_env.jj_cmd_ok(&local_path, &["branch", "delete", "remote-delete"]);
    test_env.jj_cmd_ok(&local_path, &["describe", "-mrewritten", "remote-rewrite"]);

    let template = r#"separate(" ", commit_id.short(), branches, if(hidden, "(hidden)"))"#;
    insta::assert_snapshot!(
        test_env.jj_cmd_success(
            &local_path,
            &["log", "-r::(branches() | remote_branches())", "-T", template],
        ),
        @r###"
    ◉  e31634b64294 remote-rewrite*
    │ @  c7b4c09cd77c local-keep
    ├─╯
    │ ◉  3e9a5af6ef15 remote-rewrite@origin (hidden)
    ├─╯
    │ ◉  dad5f298ca57 remote-delete@origin
    ├─╯
    │ ◉  911e912015fb remote-keep
    ├─╯
    ◉  000000000000
    "###);

    // All branches are listed by default.
    insta::assert_snapshot!(test_env.jj_cmd_success(&local_path, &["branch", "list"]), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);

    let query =
        |args: &[&str]| test_env.jj_cmd_success(&local_path, &[&["branch", "list"], args].concat());
    let query_error =
        |args: &[&str]| test_env.jj_cmd_failure(&local_path, &[&["branch", "list"], args].concat());

    // "all()" doesn't include deleted branches since they have no local targets.
    // So "all()" is identical to "branches()".
    insta::assert_snapshot!(query(&["-rall()"]), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);

    // Exclude remote-only branches. "remote-rewrite@origin" is included since
    // local "remote-rewrite" target matches.
    insta::assert_snapshot!(query(&["-rbranches()"]), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);

    // Select branches by name.
    insta::assert_snapshot!(query(&["remote-rewrite"]), @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(query(&["-rbranches(remote-rewrite)"]), @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);

    // Can select deleted branch by name pattern, but not by revset.
    insta::assert_snapshot!(query(&["remote-delete"]), @r###"
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    "###);
    insta::assert_snapshot!(query(&["-rbranches(remote-delete)"]), @r###"
    "###);
    insta::assert_snapshot!(query_error(&["-rremote-delete"]), @r###"
    Error: Revision "remote-delete" doesn't exist
    Hint: Did you mean "remote-delete@origin", "remote-keep", "remote-rewrite", "remote-rewrite@origin"?
    "###);

    // Name patterns are OR-ed.
    insta::assert_snapshot!(query(&["glob:*-keep", "remote-delete"]), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    "###);

    // Unmatched name pattern shouldn't be an error. A warning can be added later.
    insta::assert_snapshot!(query(&["local-keep", "glob:push-*"]), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    "###);

    // Name pattern and revset are OR-ed.
    insta::assert_snapshot!(query(&["local-keep", "-rbranches(remote-rewrite)"]), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"branches ++ " " ++ commit_id.short()"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list", "--all"])
}
