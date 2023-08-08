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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_git_clone() {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("source");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();

    // Clone an empty repo
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["git", "clone", "source", "empty"]);
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/empty"
    Nothing changed.
    "###);

    // Set-up a non-empty repo
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
            Some("refs/heads/main"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();
    git_repo.set_head("refs/heads/main").unwrap();

    // Clone with relative source path
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["git", "clone", "source", "clone"]);
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/clone"
    Working copy now at: uuqppmxq 1f0b881a (empty) (no description set)
    Parent commit      : mzyxwzks 9f01a0e0 main | message
    Added 1 files, modified 0 files, removed 0 files
    "###);
    assert!(test_env.env_root().join("clone").join("file").exists());

    // Subsequent fetch should just work even if the source path was relative
    let stdout = test_env.jj_cmd_success(&test_env.env_root().join("clone"), &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);

    // Failed clone should clean up the destination directory
    std::fs::create_dir(test_env.env_root().join("bad")).unwrap();
    let assert = test_env
        .jj_cmd(test_env.env_root(), &["git", "clone", "bad", "failed"])
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&common::get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&common::get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Error: could not find repository from '$TEST_ENV/bad'; class=Repository (6)
    "###);
    assert!(!test_env.env_root().join("failed").exists());

    // Failed clone shouldn't remove the existing destination directory
    std::fs::create_dir(test_env.env_root().join("failed")).unwrap();
    let assert = test_env
        .jj_cmd(test_env.env_root(), &["git", "clone", "bad", "failed"])
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&common::get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&common::get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Error: could not find repository from '$TEST_ENV/bad'; class=Repository (6)
    "###);
    assert!(test_env.env_root().join("failed").exists());
    assert!(!test_env.env_root().join("failed").join(".jj").exists());

    // Failed clone (if attempted) shouldn't remove the existing workspace
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["git", "clone", "bad", "clone"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);
    assert!(test_env.env_root().join("clone").join(".jj").exists());

    // Try cloning into an existing workspace
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["git", "clone", "source", "clone"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);

    // Try cloning into an existing file
    std::fs::write(test_env.env_root().join("file"), "contents").unwrap();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["git", "clone", "source", "file"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);

    // Try cloning into non-empty, non-workspace directory
    std::fs::remove_dir_all(test_env.env_root().join("clone").join(".jj")).unwrap();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["git", "clone", "source", "clone"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);
}

#[test]
fn test_git_clone_colocate() {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("source");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();

    // Clone an empty repo
    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["git", "clone", "source", "empty", "--colocate"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/empty"
    Nothing changed.
    "###);

    // Set-up a non-empty repo
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
            Some("refs/heads/main"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();
    git_repo.set_head("refs/heads/main").unwrap();

    // Clone with relative source path
    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["git", "clone", "source", "clone", "--colocate"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/clone"
    Working copy now at: uuqppmxq 1f0b881a (empty) (no description set)
    Parent commit      : mzyxwzks 9f01a0e0 main | message
    Added 1 files, modified 0 files, removed 0 files
    "###);
    assert!(test_env.env_root().join("clone").join("file").exists());
    assert!(test_env.env_root().join("clone").join(".git").exists());

    eprintln!(
        "{:?}",
        git_repo.head().expect("Repo head should be set").name()
    );

    let jj_git_repo = git2::Repository::open(test_env.env_root().join("clone"))
        .expect("Could not open clone repo");
    assert_eq!(
        jj_git_repo
            .head()
            .expect("Clone Repo HEAD should be set.")
            .symbolic_target(),
        git_repo
            .head()
            .expect("Repo HEAD should be set.")
            .symbolic_target()
    );

    // Subsequent fetch should just work even if the source path was relative
    let stdout = test_env.jj_cmd_success(&test_env.env_root().join("clone"), &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);

    // Failed clone should clean up the destination directory
    std::fs::create_dir(test_env.env_root().join("bad")).unwrap();
    let assert = test_env
        .jj_cmd(
            test_env.env_root(),
            &["git", "clone", "--colocate", "bad", "failed"],
        )
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&common::get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&common::get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Error: could not find repository from '$TEST_ENV/bad'; class=Repository (6)
    "###);
    assert!(!test_env.env_root().join("failed").exists());

    // Failed clone shouldn't remove the existing destination directory
    std::fs::create_dir(test_env.env_root().join("failed")).unwrap();
    let assert = test_env
        .jj_cmd(
            test_env.env_root(),
            &["git", "clone", "--colocate", "bad", "failed"],
        )
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&common::get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&common::get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Error: could not find repository from '$TEST_ENV/bad'; class=Repository (6)
    "###);
    assert!(test_env.env_root().join("failed").exists());
    assert!(!test_env.env_root().join("failed").join(".git").exists());
    assert!(!test_env.env_root().join("failed").join(".jj").exists());

    // Failed clone (if attempted) shouldn't remove the existing workspace
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["git", "clone", "--colocate", "bad", "clone"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);
    assert!(test_env.env_root().join("clone").join(".git").exists());
    assert!(test_env.env_root().join("clone").join(".jj").exists());

    // Try cloning into an existing workspace
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["git", "clone", "source", "clone", "--colocate"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);

    // Try cloning into an existing file
    std::fs::write(test_env.env_root().join("file"), "contents").unwrap();
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["git", "clone", "source", "file", "--colocate"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);

    // Try cloning into non-empty, non-workspace directory
    std::fs::remove_dir_all(test_env.env_root().join("clone").join(".jj")).unwrap();
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["git", "clone", "source", "clone", "--colocate"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Destination path exists and is not an empty directory
    "###);
}
