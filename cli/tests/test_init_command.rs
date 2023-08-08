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

use std::path::Path;

use test_case::test_case;

use crate::common::TestEnvironment;

pub mod common;

fn init_git_repo(git_repo_path: &Path, bare: bool) {
    init_git_repo_with_opts(git_repo_path, git2::RepositoryInitOptions::new().bare(bare));
}

fn init_git_repo_with_opts(git_repo_path: &Path, opts: &git2::RepositoryInitOptions) {
    let git_repo = git2::Repository::init_opts(git_repo_path, opts).unwrap();
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

#[test_case(false; "full")]
#[test_case(true; "bare")]
fn test_init_git_external(bare: bool) {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("git-repo");
    init_git_repo(&git_repo_path, bare);

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "init",
            "repo",
            "--git-repo",
            git_repo_path.to_str().unwrap(),
        ],
    );
    insta::allow_duplicates! {
        insta::assert_snapshot!(stdout, @r###"
        Working copy now at: sqpuoqvx f6950fc1 (empty) (no description set)
        Parent commit      : mwrttmos 8d698d4a my-branch | My commit message
        Added 1 files, modified 0 files, removed 0 files
        Initialized repo in "repo"
        "###);
    }

    let workspace_root = test_env.env_root().join("repo");
    let jj_path = workspace_root.join(".jj");
    let repo_path = jj_path.join("repo");
    let store_path = repo_path.join("store");
    assert!(workspace_root.is_dir());
    assert!(jj_path.is_dir());
    assert!(jj_path.join("working_copy").is_dir());
    assert!(repo_path.is_dir());
    assert!(store_path.is_dir());
    let unix_git_target_file_contents = std::fs::read_to_string(store_path.join("git_target"))
        .unwrap()
        .replace('\\', "/");
    if bare {
        assert!(unix_git_target_file_contents.ends_with("/git-repo"));
    } else {
        assert!(unix_git_target_file_contents.ends_with("/git-repo/.git"));
    }

    // Check that the Git repo's HEAD got checked out
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "@-"]);
    insta::allow_duplicates! {
        insta::assert_snapshot!(stdout, @r###"
        ◉  mwrttmos git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a
        │  My commit message
        ~
        "###);
    }
}

#[test]
fn test_init_git_external_non_existent_directory() {
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
fn test_init_git_external_non_existent_git_directory() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let stderr =
        test_env.jj_cmd_failure(test_env.env_root(), &["init", "repo", "--git-repo", "repo"]);

    insta::with_settings!({filters => vec![
        (r"(Failed to open git repository:)(?s).*", "Failed to open git repository:"),
    ]}, {
        insta::assert_snapshot!(&stderr, @"Error: Failed to access the repository: Failed to open git repository:");
    });
    let jj_path = workspace_root.join(".jj");
    assert!(!jj_path.exists());
}

#[test]
fn test_init_git_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo(&workspace_root, false);
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
    ◉  mwrttmos git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a
    │  My commit message
    ~
    "###);

    // Check that the Git repo's HEAD moves
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  sqpuoqvx test.user@example.com 2001-02-03 04:05:07.000 +07:00 HEAD@git f61b77cd
    │  (no description set)
    ~
    "###);
}

#[test]
fn test_init_git_colocated_gitlink() {
    let test_env = TestEnvironment::default();
    // <workspace_root>/.git -> <git_repo_path>
    let git_repo_path = test_env.env_root().join("git-repo");
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo_with_opts(
        &git_repo_path,
        git2::RepositoryInitOptions::new().workdir_path(&workspace_root),
    );
    assert!(workspace_root.join(".git").is_file());
    let stdout = test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "."
    "###);

    // Check that the Git repo's HEAD got checked out
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  mwrttmos git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a
    │  My commit message
    ~
    "###);

    // Check that the Git repo's HEAD moves
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  sqpuoqvx test.user@example.com 2001-02-03 04:05:07.000 +07:00 HEAD@git f61b77cd
    │  (no description set)
    ~
    "###);
}

#[cfg(unix)]
#[test]
fn test_init_git_colocated_symlink_directory() {
    let test_env = TestEnvironment::default();
    // <workspace_root>/.git -> <git_repo_path>
    let git_repo_path = test_env.env_root().join("git-repo");
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo(&git_repo_path, false);
    std::fs::create_dir(&workspace_root).unwrap();
    std::os::unix::fs::symlink(git_repo_path.join(".git"), workspace_root.join(".git")).unwrap();
    let stdout = test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "."
    "###);

    // Check that the Git repo's HEAD got checked out
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  mwrttmos git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a
    │  My commit message
    ~
    "###);

    // Check that the Git repo's HEAD moves
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  sqpuoqvx test.user@example.com 2001-02-03 04:05:07.000 +07:00 HEAD@git f61b77cd
    │  (no description set)
    ~
    "###);
}

#[cfg(unix)]
#[test]
fn test_init_git_colocated_symlink_gitlink() {
    let test_env = TestEnvironment::default();
    // <workspace_root>/.git -> <git_workdir_path>/.git -> <git_repo_path>
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_workdir_path = test_env.env_root().join("git-workdir");
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo_with_opts(
        &git_repo_path,
        git2::RepositoryInitOptions::new().workdir_path(&git_workdir_path),
    );
    assert!(git_workdir_path.join(".git").is_file());
    std::fs::create_dir(&workspace_root).unwrap();
    std::os::unix::fs::symlink(git_workdir_path.join(".git"), workspace_root.join(".git")).unwrap();
    let stdout = test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "."
    "###);

    // Check that the Git repo's HEAD got checked out
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  mwrttmos git.user@example.com 1970-01-01 01:02:03.000 +01:00 my-branch HEAD@git 8d698d4a
    │  My commit message
    ~
    "###);

    // Check that the Git repo's HEAD moves
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  sqpuoqvx test.user@example.com 2001-02-03 04:05:07.000 +07:00 HEAD@git f61b77cd
    │  (no description set)
    ~
    "###);
}

#[test]
fn test_init_git_external_but_git_dir_exists() {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("git-repo");
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&git_repo_path).unwrap();
    init_git_repo(&workspace_root, false);
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["init", "--git-repo", git_repo_path.to_str().unwrap()],
    );
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "."
    "###);

    // The local ".git" repository is unrelated, so no commits should be imported
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  zzzzzzzz 1970-01-01 00:00:00.000 +00:00 00000000
       (empty) (no description set)
    "###);

    // Check that Git HEAD is not set because this isn't a colocated repo
    test_env.jj_cmd_success(&workspace_root, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  qpvuntsm test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059
    │  (empty) (no description set)
    ~
    "###);
}

#[test]
fn test_init_git_internal_but_could_be_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    init_git_repo(&workspace_root, false);

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["init", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    Initialized repo in "."
    "###);
    insta::assert_snapshot!(stderr, @r###"
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
