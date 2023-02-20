// Copyright 2023 The Jujutsu Authors
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

/// Add a remote containing a branch with the same name
fn add_git_remote(test_env: &TestEnvironment, repo_path: &Path, remote: &str) {
    let git_repo_path = test_env.env_root().join(remote);
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
    git_repo
        .commit(
            Some(&format!("refs/heads/{remote}")),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();
    test_env.jj_cmd_success(
        repo_path,
        &["git", "remote", "add", remote, &format!("../{remote}")],
    );
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list"])
}

#[test]
fn test_git_fetch_default_remote() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");

    test_env.jj_cmd_success(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    origin: 9f01a0e04879 message
    "###);
}

#[test]
fn test_git_fetch_single_remote() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    test_env.jj_cmd_success(&repo_path, &["git", "fetch", "--remote", "rem1"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    rem1: 9f01a0e04879 message
    "###);
}

#[test]
fn test_git_fetch_single_remote_from_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    test_env.add_config(r#"git.fetch = "rem1""#);

    test_env.jj_cmd_success(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    rem1: 9f01a0e04879 message
    "###);
}

#[test]
fn test_git_fetch_multiple_remotes() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    add_git_remote(&test_env, &repo_path, "rem2");

    test_env.jj_cmd_success(
        &repo_path,
        &["git", "fetch", "--remote", "rem1", "--remote", "rem2"],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    rem1: 9f01a0e04879 message
    rem2: 9f01a0e04879 message
    "###);
}

#[test]
fn test_git_fetch_multiple_remotes_from_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    add_git_remote(&test_env, &repo_path, "rem2");
    test_env.add_config(r#"git.fetch = ["rem1", "rem2"]"#);

    test_env.jj_cmd_success(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    rem1: 9f01a0e04879 message
    rem2: 9f01a0e04879 message
    "###);
}

#[test]
fn test_git_fetch_nonexistent_remote() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    let stderr = &test_env.jj_cmd_failure(
        &repo_path,
        &["git", "fetch", "--remote", "rem1", "--remote", "rem2"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: No git remote named 'rem2'
    "###);
    // No remote should have been fetched as part of the failing transaction
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
}

#[test]
fn test_git_fetch_nonexistent_remote_from_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    test_env.add_config(r#"git.fetch = ["rem1", "rem2"]"#);

    let stderr = &test_env.jj_cmd_failure(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No git remote named 'rem2'
    "###);
    // No remote should have been fetched as part of the failing transaction
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
}

#[test]
fn test_git_fetch_prune_before_updating_tips() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");
    test_env.jj_cmd_success(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    origin: 9f01a0e04879 message
    "###);

    // Remove origin branch in git repo and create origin/subname
    let git_repo = git2::Repository::open(test_env.env_root().join("origin")).unwrap();
    git_repo
        .find_branch("origin", git2::BranchType::Local)
        .unwrap()
        .rename("origin/subname", false)
        .unwrap();

    test_env.jj_cmd_success(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    origin/subname: 9f01a0e04879 message
    "###);
}
