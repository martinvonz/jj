// Copyright 2020 Google LLC
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

use jujutsu::testutils;

#[test]
fn test_init_git_internal() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output = testutils::CommandRunner::new(temp_dir.path()).run(vec!["init", "repo", "--git"]);
    assert_eq!(output.status, 0);

    let workspace_root = temp_dir.path().join("repo");
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
    assert_eq!(
        output.stdout_string(),
        format!(
            "Initialized repo in \"{}\"\n",
            workspace_root.to_str().unwrap()
        )
    );
}

#[test]
fn test_init_git_external() {
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_path = temp_dir.path().join("git-repo");
    git2::Repository::init(git_repo_path.clone()).unwrap();

    let output = testutils::CommandRunner::new(temp_dir.path()).run(vec![
        "init",
        "repo",
        "--git-repo",
        git_repo_path.to_str().unwrap(),
    ]);
    assert_eq!(output.status, 0);

    let workspace_root = temp_dir.path().join("repo");
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
        .ends_with("/git-repo"));
    assert_eq!(
        output.stdout_string(),
        format!("Initialized repo in \"{}\"\n", workspace_root.display())
    );
}

#[test]
fn test_init_local() {
    let temp_dir = tempfile::tempdir().unwrap();

    let output = testutils::CommandRunner::new(temp_dir.path()).run(vec!["init", "repo"]);
    assert_eq!(output.status, 0);

    let workspace_root = temp_dir.path().join("repo");
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
    assert_eq!(
        output.stdout_string(),
        format!(
            "Initialized repo in \"{}\"\n",
            workspace_root.to_str().unwrap()
        )
    );
}
