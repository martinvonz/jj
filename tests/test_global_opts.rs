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

use jujutsu::testutils::{get_stdout_string, TestEnvironment};

#[test]
fn test_no_commit_working_copy() {
    let test_env = TestEnvironment::default();
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo"])
        .assert()
        .success();

    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "initial").unwrap();
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id"])
        .assert()
        .success();
    let initial_commit_id_hex = get_stdout_string(&assert);

    // Modify the file. With --no-commit-working-copy, we still get the same commit
    // ID.
    std::fs::write(repo_path.join("file"), "modified").unwrap();
    let assert = test_env
        .jj_cmd(
            &repo_path,
            &["log", "-T", "commit_id", "--no-commit-working-copy"],
        )
        .assert()
        .success();
    let still_initial_commit_id_hex = get_stdout_string(&assert);
    assert_eq!(still_initial_commit_id_hex, initial_commit_id_hex);
    // But without --no-commit-working-copy, we get a new commit ID.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id"])
        .assert()
        .success();
    let modified_commit_id_hex = get_stdout_string(&assert);
    assert_ne!(modified_commit_id_hex, initial_commit_id_hex);
}
