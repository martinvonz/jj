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

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_branch_multiple_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let assert = test_env
        .jj_cmd(&repo_path, &["branch", "set", "foo", "bar"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @"warning: Updating multiple branches (2).
");

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 230dd059e1b0
    ◉   000000000000
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "delete", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
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
    insta::assert_snapshot!(stdout, @"");
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
    foo: 65b6b74e0897 (no description set)
    "###);

    // Exporting the branch to git creates a local-git tracking branch
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "forget", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    // Forgetting a branch does not delete its local-git tracking branch. This is
    // the opposite of what happens to remote-tracking branches.
    // TODO: Consider allowing forgetting local-git tracking branches as an option
    let stdout = test_env.jj_cmd_success(&repo_path, &["branch", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    foo (deleted)
      @git: 65b6b74e0897 (no description set)
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r=foo", "--no-graph"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "foo" doesn't exist
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r=foo@git", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    rlvkpnrzqnoo test.user@example.com 2001-02-03 04:05:08.000 +07:00 65b6b74e0897
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
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
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
    feature1: 9f01a0e04879 message
    "###);

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
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "export"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

    // Short-term TODO: Fix this BUG. It should be possible to fetch `feature1`
    // again.
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");

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
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "forget", "feature1"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such branch: feature1
    "###);

    // BUG: fetching a moved branch creates a move-deletion conflict
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1 (conflicted):
      - 9f01a0e04879 message
      + 38aefb173976 another message
    "###);
}

// TODO: Test `jj branch list` with a remote named `git`

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"branches ++ " " ++ commit_id.short()"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list"])
}
