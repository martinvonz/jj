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

use std::path::{self, Path, PathBuf};

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

fn set_up_non_empty_git_repo(git_repo: &git2::Repository) {
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
}

#[test]
fn test_git_clone() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-branch = true");
    let git_repo_path = test_env.env_root().join("source");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();

    // Clone an empty repo
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "empty"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/empty"
    Nothing changed.
    "###);

    set_up_non_empty_git_repo(&git_repo);

    // Clone with relative source path
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "clone"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/clone"
    branch: main@origin [new] tracked
    Working copy now at: uuqppmxq 1f0b881a (empty) (no description set)
    Parent commit      : mzyxwzks 9f01a0e0 main | message
    Added 1 files, modified 0 files, removed 0 files
    "###);
    assert!(test_env.env_root().join("clone").join("file").exists());

    // Subsequent fetch should just work even if the source path was relative
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&test_env.env_root().join("clone"), &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // Failed clone should clean up the destination directory
    std::fs::create_dir(test_env.env_root().join("bad")).unwrap();
    let assert = test_env
        .jj_cmd(test_env.env_root(), &["git", "clone", "bad", "failed"])
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    Error: could not find repository at '$TEST_ENV/bad'; class=Repository (6)
    "###);
    assert!(!test_env.env_root().join("failed").exists());

    // Failed clone shouldn't remove the existing destination directory
    std::fs::create_dir(test_env.env_root().join("failed")).unwrap();
    let assert = test_env
        .jj_cmd(test_env.env_root(), &["git", "clone", "bad", "failed"])
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    Error: could not find repository at '$TEST_ENV/bad'; class=Repository (6)
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
    test_env.add_config("git.auto-local-branch = true");
    let git_repo_path = test_env.env_root().join("source");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();

    // Clone an empty repo
    let (stdout, stderr) = test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", "source", "empty", "--colocate"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/empty"
    Nothing changed.
    "###);

    // git_target path should be relative to the store
    let store_path = test_env
        .env_root()
        .join(PathBuf::from_iter(["empty", ".jj", "repo", "store"]));
    let git_target_file_contents = std::fs::read_to_string(store_path.join("git_target")).unwrap();
    insta::assert_snapshot!(
        git_target_file_contents.replace(path::MAIN_SEPARATOR, "/"),
        @"../../../.git");

    set_up_non_empty_git_repo(&git_repo);

    // Clone with relative source path
    let (stdout, stderr) = test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", "source", "clone", "--colocate"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/clone"
    branch: main@origin [new] tracked
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
    // ".jj" directory should be ignored at Git side.
    #[allow(clippy::format_collect)]
    let git_statuses: String = jj_git_repo
        .statuses(None)
        .unwrap()
        .iter()
        .map(|entry| format!("{:?} {}\n", entry.status(), entry.path().unwrap()))
        .collect();
    insta::assert_snapshot!(git_statuses, @r###"
    Status(IGNORED) .jj/.gitignore
    Status(IGNORED) .jj/repo/
    Status(IGNORED) .jj/working_copy/
    "###);

    // The old default branch "master" shouldn't exist.
    insta::assert_snapshot!(
        get_branch_output(&test_env, &test_env.env_root().join("clone")), @r###"
    main: mzyxwzks 9f01a0e0 message
      @git: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // Subsequent fetch should just work even if the source path was relative
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&test_env.env_root().join("clone"), &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
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
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    Error: could not find repository at '$TEST_ENV/bad'; class=Repository (6)
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
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&get_stderr_string(&assert));
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/failed"
    Error: could not find repository at '$TEST_ENV/bad'; class=Repository (6)
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

#[test]
fn test_git_clone_remote_default_branch() {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("source");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    set_up_non_empty_git_repo(&git_repo);
    // Create non-default branch in remote
    let oid = git_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .target()
        .unwrap();
    git_repo
        .reference("refs/heads/feature1", oid, false, "")
        .unwrap();

    // All fetched branches will be imported if auto-local-branch is on
    test_env.add_config("git.auto-local-branch = true");
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "clone1"]);
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/clone1"
    branch: feature1@origin [new] tracked
    branch: main@origin     [new] tracked
    Working copy now at: sqpuoqvx cad212e1 (empty) (no description set)
    Parent commit      : mzyxwzks 9f01a0e0 feature1 main | message
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(
        get_branch_output(&test_env, &test_env.env_root().join("clone1")), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    main: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // Only the default branch will be imported if auto-local-branch is off
    test_env.add_config("git.auto-local-branch = false");
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "clone2"]);
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/clone2"
    branch: feature1@origin [new] untracked
    branch: main@origin     [new] untracked
    Working copy now at: pmmvwywv fa729b1e (empty) (no description set)
    Parent commit      : mzyxwzks 9f01a0e0 feature1@origin main | message
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(
        get_branch_output(&test_env, &test_env.env_root().join("clone2")), @r###"
    feature1@origin: mzyxwzks 9f01a0e0 message
    main: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list", "--all"])
}
