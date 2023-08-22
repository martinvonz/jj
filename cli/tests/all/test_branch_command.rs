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

#[test]
fn test_branch_multiple_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "set", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    warning: Updating multiple branches (2).
    "###);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 230dd059e1b0
    ◉   000000000000
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "delete", "foo", "bar", "foo"]);
    insta::assert_snapshot!(stdout, @r###"
    Deleted 2 branches.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   230dd059e1b0
    ◉   000000000000
    "###);
}

#[test]
fn test_branch_forbidden_at_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "create", "fred", "-r=root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rewrite the root commit
    "###);
}

#[test]
fn test_branch_empty_name() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["branch", "create", ""]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '<NAMES>...' but none was supplied

    For more information, try '--help'.
    "###);
}

#[test]
fn test_branch_forget_glob() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-1"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "bar-2"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-3"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-4"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 230dd059e1b0
    ◉   000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "forget", "--glob", "foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @r###"
    Forgot 2 branches.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-4 230dd059e1b0
    ◉   000000000000
    "###);

    // Forgetting a branch via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "branch", "forget", "foo-4", "--glob", "foo-*", "--glob", "foo-*",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 230dd059e1b0
    ◉   000000000000
    "###);

    // Malformed glob
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "forget", "--glob", "foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to compile glob: Pattern syntax error near position 4: invalid range pattern
    "###);

    // We get an error if none of the globs match anything
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &[
            "branch",
            "forget",
            "--glob=bar*",
            "--glob=baz*",
            "--glob=boom*",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: The provided globs 'baz*', 'boom*' did not match any branches
    "###);
}

#[test]
fn test_branch_delete_glob() {
    // Set up a git repo with a branch and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    test_env.jj_cmd_success(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    test_env.jj_cmd_success(&repo_path, &["describe", "-m=commit"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-1"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "bar-2"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-3"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo-4"]);
    // Push to create remote-tracking branches
    test_env.jj_cmd_success(&repo_path, &["git", "push", "--all"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 6fbf398c2d59
    │
    ~
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "delete", "--glob", "foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @r###"
    Deleted 2 branches.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1@origin foo-3@origin foo-4 6fbf398c2d59
    │
    ~
    "###);

    // We get an error if none of the globs match live branches. Unlike `jj branch
    // forget`, it's not allowed to delete already deleted branches.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "delete", "--glob=foo-[1-3]"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The provided glob 'foo-[1-3]' did not match any branches
    "###);

    // Deleting a branch via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "branch", "delete", "foo-4", "--glob", "foo-*", "--glob", "foo-*",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1@origin foo-3@origin foo-4@origin 6fbf398c2d59
    │
    ~
    "###);

    // The deleted branches are still there
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    bar-2: qpvuntsm 6fbf398c (empty) commit
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
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "delete", "--glob", "foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to compile glob: Pattern syntax error near position 4: invalid range pattern
    "###);
}

#[test]
fn test_branch_forget_export() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "foo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    foo: rlvkpnrz 65b6b74e (empty) (no description set)
    "###);

    // Exporting the branch to git creates a local-git tracking branch
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "forget", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    // Forgetting a branch does not delete its local-git tracking branch. The
    // git-tracking branch is kept.
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    foo (forgotten)
      @git: rlvkpnrz 65b6b74e (empty) (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r=foo", "--no-graph"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "foo" doesn't exist
    Hint: Did you mean "foo@git"?
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r=foo@git", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    rlvkpnrz test.user@example.com 2001-02-03 04:05:08.000 +07:00 foo@git 65b6b74e
    (empty) (no description set)
    "###);

    // The presence of the @git branch means that a `jj git import` is a no-op...
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "import"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    // ... and a `jj git export` will delete the branch from git and will delete the
    // git-tracking branch. In a colocated repo, this will happen automatically
    // immediately after a `jj branch forget`. This is demonstrated in
    // `test_git_colocated_branch_forget` in test_git_colocated.rs
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @"");

    // Note that if `jj branch forget` *did* delete foo@git, a subsequent `jj
    // git export` would be a no-op and a `jj git import` would resurrect
    // the branch. In a normal repo, that might be OK. In a colocated repo,
    // this would automatically happen before the next command, making `jj
    // branch forget` useless.
}

#[test]
fn test_branch_forget_fetched_branch() {
    // Much of this test is borrowed from `test_git_fetch_remote_only_branch` in
    // test_git_fetch.rs

    // Set up a git repo with a branch and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_success(
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
    test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 1: with export-import
    // Forget the branch
    test_env.jj_cmd_success(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // At this point `jj git export && jj git import` does *not* recreate the
    // branch. This behavior is important in colocated repos, as otherwise a
    // forgotten branch would be immediately resurrected.
    //
    // Technically, this is because `jj branch forget` preserved
    // the ref in jj view's `git_refs` tracking the local git repo's remote-tracking
    // branch.
    // TODO: Show that jj git push is also a no-op
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "export"]), @"");
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // We can fetch feature1 again.
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 2: No export/import (otherwise the same as test 1)
    test_env.jj_cmd_success(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
    // Fetch works even without the export-import
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
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
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(stdout, @"");

    // Fetching a moved branch does not create a conflict
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: ooosovrs 38aefb17 (empty) another message
    "###);
}

#[test]
fn test_branch_forget_deleted_or_nonexistent_branch() {
    // Much of this test is borrowed from `test_git_fetch_remote_only_branch` in
    // test_git_fetch.rs

    // ======== Beginning of test setup ========
    // Set up a git repo with a branch and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_success(
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
    test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "delete", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1 (deleted)
      @origin: mzyxwzks 9f01a0e0 message
      (this branch will be *deleted permanently* on the remote on the
       next `jj git push`. Use `jj branch forget` to prevent this)
    "###);

    // ============ End of test setup ============

    // We can forget a deleted branch
    test_env.jj_cmd_success(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // Can't forget a non-existent branch
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "forget", "i_do_not_exist"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such branch: i_do_not_exist
    "###);
}

// TODO: Test `jj branch list` with a remote named `git`

#[test]
fn test_branch_list_filtered_by_revset() {
    let test_env = TestEnvironment::default();

    // Initialize remote refs
    test_env.jj_cmd_success(test_env.env_root(), &["init", "remote", "--git"]);
    let remote_path = test_env.env_root().join("remote");
    for branch in ["remote-keep", "remote-delete", "remote-rewrite"] {
        test_env.jj_cmd_success(&remote_path, &["new", "root", "-m", branch]);
        test_env.jj_cmd_success(&remote_path, &["branch", "set", branch]);
    }
    test_env.jj_cmd_success(&remote_path, &["new"]);
    test_env.jj_cmd_success(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = remote_path;
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_success(
        test_env.env_root(),
        &["git", "clone", remote_git_path.to_str().unwrap(), "local"],
    );
    let local_path = test_env.env_root().join("local");
    test_env.jj_cmd_success(&local_path, &["new", "root", "-m", "local-keep"]);
    test_env.jj_cmd_success(&local_path, &["branch", "set", "local-keep"]);

    // Mutate refs in local repository
    test_env.jj_cmd_success(&local_path, &["branch", "delete", "remote-delete"]);
    test_env.jj_cmd_success(&local_path, &["describe", "-mrewritten", "remote-rewrite"]);

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
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn 3e9a5af6 (empty) remote-rewrite
    "###);

    let query = |revset| test_env.jj_cmd_success(&local_path, &["branch", "list", "-r", revset]);
    let query_error =
        |revset| test_env.jj_cmd_failure(&local_path, &["branch", "list", "-r", revset]);

    // "all()" doesn't include deleted branches since they have no local targets.
    // So "all()" is identical to "branches()".
    insta::assert_snapshot!(query("all()"), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn 3e9a5af6 (empty) remote-rewrite
    "###);

    // Exclude remote-only branches. "remote-rewrite@origin" is included since
    // local "remote-rewrite" target matches.
    insta::assert_snapshot!(query("branches()"), @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn 3e9a5af6 (empty) remote-rewrite
    "###);

    // Select branches by name.
    insta::assert_snapshot!(query("branches(remote-rewrite)"), @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn 3e9a5af6 (empty) remote-rewrite
    "###);

    // Can't select deleted branch.
    insta::assert_snapshot!(query("branches(remote-delete)"), @r###"
    "###);
    insta::assert_snapshot!(query_error("remote-delete"), @r###"
    Error: Revision "remote-delete" doesn't exist
    Hint: Did you mean "remote-delete@origin", "remote-keep", "remote-rewrite", "remote-rewrite@origin"?
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"branches ++ " " ++ commit_id.short()"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list"])
}
