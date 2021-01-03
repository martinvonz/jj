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

use jujube::testutils;

#[test]
fn test_init_git_internal() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output = testutils::CommandRunner::new(temp_dir.path()).run(vec!["init", "repo", "--git"]);
    assert_eq!(output.status, 0);

    let repo_path = temp_dir.path().join("repo");
    assert!(repo_path.is_dir());
    assert!(repo_path.join(".jj").is_dir());
    assert!(repo_path.join(".jj").join("git").is_dir());
    let store_file_contents = std::fs::read_to_string(repo_path.join(".jj").join("store")).unwrap();
    assert_eq!(store_file_contents, "git: git");
    assert_eq!(
        output.stdout_string(),
        format!("Initialized repo in \"{}\"\n", repo_path.to_str().unwrap())
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
        "--git-store",
        git_repo_path.to_str().unwrap(),
    ]);
    assert_eq!(output.status, 0);

    let repo_path = temp_dir.path().join("repo");
    assert!(repo_path.is_dir());
    assert!(repo_path.join(".jj").is_dir());
    let store_file_contents = std::fs::read_to_string(repo_path.join(".jj").join("store")).unwrap();
    assert!(store_file_contents.starts_with("git: "));
    assert!(store_file_contents.ends_with("/git-repo"));
    assert_eq!(
        output.stdout_string(),
        format!("Initialized repo in \"{}\"\n", repo_path.to_str().unwrap())
    );
}

#[test]
fn test_init_local() {
    let temp_dir = tempfile::tempdir().unwrap();

    let output = testutils::CommandRunner::new(temp_dir.path()).run(vec!["init", "repo"]);
    assert_eq!(output.status, 0);

    let repo_path = temp_dir.path().join("repo");
    assert!(repo_path.is_dir());
    assert!(repo_path.join(".jj").is_dir());
    let store_dir = repo_path.join(".jj").join("store");
    assert!(store_dir.is_dir());
    assert!(store_dir.join("commits").is_dir());
    assert!(store_dir.join("trees").is_dir());
    assert!(store_dir.join("files").is_dir());
    assert!(store_dir.join("symlinks").is_dir());
    assert!(store_dir.join("conflicts").is_dir());
    assert_eq!(
        output.stdout_string(),
        format!("Initialized repo in \"{}\"\n", repo_path.to_str().unwrap())
    );
}
