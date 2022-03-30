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

use std::path::PathBuf;

use crate::common::{get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_untrack() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "initial").unwrap();
    std::fs::write(repo_path.join("file1.bak"), "initial").unwrap();
    std::fs::write(repo_path.join("file2.bak"), "initial").unwrap();
    let target_dir = repo_path.join("target");
    std::fs::create_dir(&target_dir).unwrap();
    std::fs::write(target_dir.join("file2"), "initial").unwrap();
    std::fs::write(target_dir.join("file3"), "initial").unwrap();

    // Run a command so all the files get tracked, then add "*.bak" to the ignore
    // patterns
    test_env.jj_cmd_success(&repo_path, &["st"]);
    std::fs::write(repo_path.join(".gitignore"), "*.bak\n").unwrap();
    let files_before = test_env.jj_cmd_success(&repo_path, &["files"]);

    // Errors out when a specified file is not ignored
    let assert = test_env
        .jj_cmd(&repo_path, &["untrack", "file1", "file1.bak"])
        .assert()
        .failure();
    assert.stdout(
        "Error: 'file1' would be added back because it's not ignored. Make sure it's ignored, \
         then try again.\n",
    );
    let files_after = test_env.jj_cmd_success(&repo_path, &["files"]);
    // There should be no changes to the state when there was an error
    assert_eq!(files_after, files_before);

    // Can untrack a single file
    assert!(files_before.contains("file1.bak\n"));
    let stdout = test_env.jj_cmd_success(&repo_path, &["untrack", "file1.bak"]);
    assert_eq!(stdout, "");
    let files_after = test_env.jj_cmd_success(&repo_path, &["files"]);
    // The file is no longer tracked
    assert!(!files_after.contains("file1.bak"));
    // Other files that match the ignore pattern are not untracked
    assert!(files_after.contains("file2.bak"));
    // The files still exist on disk
    assert!(repo_path.join("file1.bak").exists());
    assert!(repo_path.join("file2.bak").exists());

    // Errors out when multiple specified files are not ignored
    let assert = test_env
        .jj_cmd(&repo_path, &["untrack", "target"])
        .assert()
        .failure();
    assert_eq!(
        get_stdout_string(&assert),
        format!(
            "Error: '{}' and 1 other files would be added back because they're not ignored. Make \
             sure they're ignored, then try again.\n",
            PathBuf::from("target").join("file2").display()
        )
    );

    // Can untrack after adding to ignore patterns
    std::fs::write(repo_path.join(".gitignore"), ".bak\ntarget/\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["untrack", "target"]);
    assert_eq!(stdout, "");
    let files_after = test_env.jj_cmd_success(&repo_path, &["files"]);
    assert!(!files_after.contains("target"));
}
