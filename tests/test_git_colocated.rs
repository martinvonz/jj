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

use std::path::Path;

use git2::Oid;

use crate::common::{get_stderr_string, TestEnvironment};

pub mod common;

#[test]
fn test_git_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();

    // Create an initial commit in Git
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    git_repo
        .index()
        .unwrap()
        .add_path(Path::new("file"))
        .unwrap();
    let tree1_oid = git_repo.index().unwrap().write_tree().unwrap();
    let tree1 = git_repo.find_tree(tree1_oid).unwrap();
    let signature = git2::Signature::new(
        "Someone",
        "someone@example.com",
        &git2::Time::new(1234567890, 60),
    )
    .unwrap();
    git_repo
        .commit(
            Some("refs/heads/master"),
            &signature,
            &signature,
            "initial",
            &tree1,
            &[],
        )
        .unwrap();
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    // Import the repo
    test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @ 3e9369cd54227eb88455e1834dbc08aad6a16ac4 
    o e61b6729ff4292870702f2f72b2a60165679ef37 master
    o 0000000000000000000000000000000000000000 
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    // Modify the working copy. The working-copy commit should changed, but the Git
    // HEAD commit should not
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @ b26951a9c6f5c270e4d039880208952fd5faae5e 
    o e61b6729ff4292870702f2f72b2a60165679ef37 master
    o 0000000000000000000000000000000000000000 
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    // Create a new change from jj and check that it's reflected in Git
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @ 9dbb23ff2ff5e66c43880f1042369d704f7a321e 
    o b26951a9c6f5c270e4d039880208952fd5faae5e 
    o e61b6729ff4292870702f2f72b2a60165679ef37 master
    o 0000000000000000000000000000000000000000 
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"b26951a9c6f5c270e4d039880208952fd5faae5e"
    );
}

#[test]
fn test_git_colocated_rebase_on_import() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);

    // Make some changes in jj and check that they're reflected in git
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["commit", "-m", "add a file"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "master"]);
    test_env.jj_cmd_success(&workspace_root, &["commit", "-m", "modify a file"]);
    // TODO: We shouldn't need this command here to trigger an import of the
    // refs/heads/master we just exported
    test_env.jj_cmd_success(&workspace_root, &["st"]);

    // Move `master` and HEAD backwards, which should result in commit2 getting
    // hidden, and a new working-copy commit at the new position.
    let commit2_oid = git_repo
        .find_branch("master", git2::BranchType::Local)
        .unwrap()
        .get()
        .target()
        .unwrap();
    let commit2 = git_repo.find_commit(commit2_oid).unwrap();
    let commit1 = commit2.parents().next().unwrap();
    git_repo.branch("master", &commit1, true).unwrap();
    git_repo.set_head("refs/heads/master").unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @ 840303b127545e55dfa5858a97555acf54a80513 
    o f0f3ab56bfa927e3a65c2ac9a513693d438e271b master
    o 0000000000000000000000000000000000000000 
    "###);
}

#[test]
fn test_git_colocated_branches() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    test_env.jj_cmd_success(&workspace_root, &["new", "-m", "foo"]);
    test_env.jj_cmd_success(&workspace_root, &["new", "@-", "-m", "bar"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @ 086821b6c35f5fdf07da884b859a14dcf85b5e36 
    | o 6c0e140886d181602ae7a8e1ac41bc3094842370 
    |/  
    o 230dd059e1b059aefc0da06a2e5a7dbf22362f22 master
    o 0000000000000000000000000000000000000000 
    "###);

    // Create a branch in jj. It should be exported to Git even though it points to
    // the working- copy commit.
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "master"]);
    insta::assert_snapshot!(
        git_repo.find_reference("refs/heads/master").unwrap().target().unwrap().to_string(),
        @"086821b6c35f5fdf07da884b859a14dcf85b5e36"
    );
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"230dd059e1b059aefc0da06a2e5a7dbf22362f22"
    );

    // Update the branch in Git
    git_repo
        .reference(
            "refs/heads/master",
            Oid::from_str("6c0e140886d181602ae7a8e1ac41bc3094842370").unwrap(),
            true,
            "test",
        )
        .unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    Working copy now at: eb08b363bb5e (no description set)
    @ eb08b363bb5ef8ee549314260488980d7bbe8f63 
    | o 6c0e140886d181602ae7a8e1ac41bc3094842370 master
    |/  
    o 230dd059e1b059aefc0da06a2e5a7dbf22362f22 
    o 0000000000000000000000000000000000000000 
    "###);
}

#[test]
fn test_git_colocated_conflicting_git_refs() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "main"]);
    let assert = test_env
        .jj_cmd(&workspace_root, &["branch", "create", "main/sub"])
        .assert()
        .success()
        .stdout("");
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Failed to export some branches:
      main/sub
    "###);
}

fn get_log_output(test_env: &TestEnvironment, workspace_root: &Path) -> String {
    test_env.jj_cmd_success(workspace_root, &["log", "-T", "commit_id \" \" branches"])
}
