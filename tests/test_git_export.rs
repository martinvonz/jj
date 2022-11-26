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

use crate::common::{get_stderr_string, TestEnvironment};

pub mod common;

#[test]
fn test_git_export_conflicting_git_refs() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // TODO: Make it an error to try to create a branch with an empty name
    test_env.jj_cmd_success(&repo_path, &["branch", "create", ""]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "main/sub"]);
    let assert = test_env
        .jj_cmd(&repo_path, &["git", "export"])
        .assert()
        .success()
        .stdout("");
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Failed to export some branches:
      
      main/sub
    "###);
}
