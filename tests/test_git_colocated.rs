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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_git_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);

    // Create a commit from jj and check that it's reflected in git
    std::fs::write(workspace_root.join("new-file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["commit", "-m", "add a file"]);
    let stdout =
        test_env.jj_cmd_success(&workspace_root, &["log", "-T", "commit_id \" \" branches"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 2588800a4ee68926773f1e9c44dcc50ada923650 
    o 172b1cbfe88c97cbd1b1c8a98a48e729a4540e85 master
    o 0000000000000000000000000000000000000000 
    "###);
    assert_eq!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        "172b1cbfe88c97cbd1b1c8a98a48e729a4540e85".to_string()
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
    let stdout =
        test_env.jj_cmd_success(&workspace_root, &["log", "-T", "commit_id \" \" branches"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 840303b127545e55dfa5858a97555acf54a80513 
    o f0f3ab56bfa927e3a65c2ac9a513693d438e271b master
    o 0000000000000000000000000000000000000000 
    "###);
}
