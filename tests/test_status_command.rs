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

use common::TestEnvironment;

pub mod common;

#[test]
fn test_status_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "base").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "-m=left"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "left"]);
    test_env.jj_cmd_success(&repo_path, &["new", "@-", "-m=right"]);
    std::fs::write(repo_path.join("file"), "right").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "left", "@"]);

    // The output should mention each parent, and the diff should be empty (compared
    // to the auto-merged parents)
    let stdout = test_env.jj_cmd_success(&repo_path, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    The working copy is clean
    Working copy : mzvwutvl c965365c (empty) (no description set)
    Parent commit: rlvkpnrz 9ae48ddb (empty) left
    Parent commit: zsuskuln 29b991e9 right
    "###);
}
