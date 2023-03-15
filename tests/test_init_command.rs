// Copyright 2020 The Jujutsu Authors
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

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

pub mod common;

fn init_git_repo(git_repo_path: &PathBuf) {
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    let git_blob_oid = git_repo.blob(b"some content").unwrap();
    let mut git_tree_builder = git_repo.treebuilder(None).unwrap();
    git_tree_builder
        .insert("some-file", git_blob_oid, 0o100644)
        .unwrap();
    let git_tree_id = git_tree_builder.write().unwrap();
    let git_tree = git_repo.find_tree(git_tree_id).unwrap();
    let git_signature = git2::Signature::new(
        "Git User",
        "git.user@example.com",
        &git2::Time::new(123, 60),
    )
    .unwrap();
    git_repo
        .commit(
            Some("refs/heads/my-branch"),
            &git_signature,
            &git_signature,
            "My commit message",
            &git_tree,
            &[],
        )
        .unwrap();
    git_repo.set_head("refs/heads/my-branch").unwrap();
}

#[test]
fn test_init_git_internal() {
    let test_env = TestEnvironment::default();
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "repo"
    "###);

    let workspace_root = test_env.env_root().join("repo");
    let jj_path = workspace_root.join(".jj");
    let repo_path = jj_path.join("repo");
    let store_path = repo_path.join("store");
    assert!(workspace_root.is_dir());
    assert!(jj_path.is_dir());
    assert!(jj_path.join("working_copy").is_dir());
    assert!(repo_path.is_dir());
    assert!(store_path.is_dir());
    assert!(store_path.join("git").is_dir());
    assert!(store_path.join("git_target").is_file());
    let git_target_file_contents = std::fs::read_to_string(store_path.join("git_target")).unwrap();
    assert_eq!(git_target_file_contents, "git");
}

#[test]
fn test_init_git_external() {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("git-repo");
    init_git_repo(&git_repo_path);

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "init",
            "repo",
            "--git-repo",
            git_repo_path.to_str().unwrap(),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: f6950fc115ae (no description set)
    Added 1 files, modified 0 files, removed 0 files
    Initialized repo in "repo"
    "###);

    let workspace_root = test_env.env_root().join("repo");
    let jj_path = workspace_root.join(".jj");
    let repo_path = jj_path.join("repo");
    let store_path = repo_path.join("store");
    assert!(workspace_root.is_dir());
    assert!(jj_path.is_dir());
    assert!(jj_path.join("working_copy").is_dir());
    assert!(repo_path.is_dir());
    assert!(store_path.is_dir());
    let git_target_file_contents = std::fs::read_to_string(store_path.join("git_target")).unwrap();
    assert!(git_target_file_contents
        .replace('\\', "/")
        .ends_with("/git-repo/.git"));

    // Check that the Git repo's HEAD got checked out
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  mwrttmoslwzp git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a8ee1
    │  My commit message
    ~
    "###);
}

#[test]
fn test_init_git_external_bad_path() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["init", "repo", "--git-repo", "non-existent"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: $TEST_ENV/non-existent doesn't exist
    "###);
}

#[test]
fn test_init_git_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo(&workspace_root);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "."
    "###);

    let jj_path = workspace_root.join(".jj");
    let repo_path = jj_path.join("repo");
    let store_path = repo_path.join("store");
    assert!(workspace_root.is_dir());
    assert!(jj_path.is_dir());
    assert!(jj_path.join("working_copy").is_dir());
    assert!(repo_path.is_dir());
    assert!(store_path.is_dir());
    let git_target_file_contents = std::fs::read_to_string(store_path.join("git_target")).unwrap();
    assert!(git_target_file_contents
        .replace('\\', "/")
        .ends_with("../../../.git"));

    // Check that the Git repo's HEAD got checked out
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  mwrttmoslwzp git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a8ee1
    │  My commit message
    ~
    "###);
}

#[test]
fn test_init_git_internal_but_could_be_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo(&workspace_root);

    let assert = test_env
        .jj_cmd(&workspace_root, &["init", "--git"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    Initialized repo in "."
    "###);
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Empty repo created.
    Hint: To create a repo backed by the existing Git repo, run `jj init --git-repo=.` instead.
    "###);
}

#[test]
fn test_init_git_bad_wc_path() {
    let test_env = TestEnvironment::default();
    std::fs::write(test_env.env_root().join("existing-file"), b"").unwrap();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["init", "--git", "existing-file"]);
    assert!(stderr.contains("Failed to create workspace"));
}

#[test]
fn test_init_local_disallowed() {
    let test_env = TestEnvironment::default();
    let stdout = test_env.jj_cmd_failure(test_env.env_root(), &["init", "repo"]);
    insta::assert_snapshot!(stdout, @r###"
    Error: The native backend is disallowed by default.
    Hint: Did you mean to pass `--git`?
    Set `ui.allow-init-native` to allow initializing a repo with the native backend.
    "###);
}

#[test]
fn test_init_local() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"ui.allow-init-native = true"#);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["init", "repo"]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "repo"
    "###);

    let workspace_root = test_env.env_root().join("repo");
    let jj_path = workspace_root.join(".jj");
    let repo_path = jj_path.join("repo");
    let store_path = repo_path.join("store");
    assert!(workspace_root.is_dir());
    assert!(jj_path.is_dir());
    assert!(jj_path.join("working_copy").is_dir());
    assert!(repo_path.is_dir());
    assert!(store_path.is_dir());
    assert!(store_path.join("commits").is_dir());
    assert!(store_path.join("trees").is_dir());
    assert!(store_path.join("files").is_dir());
    assert!(store_path.join("symlinks").is_dir());
    assert!(store_path.join("conflicts").is_dir());
}
