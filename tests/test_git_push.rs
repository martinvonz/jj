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

use crate::common::{get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_git_push() {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init(&git_repo_path).unwrap();

    test_env.jj_cmd_success(
        test_env.env_root(),
        &["git", "clone", "git-repo", "jj-repo"],
    );
    let workspace_root = test_env.env_root().join("jj-repo");

    // No branches to push yet
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"Nothing changed.
");

    // When pushing everything, won't push an open commit even if there's a branch
    // on it
    test_env.jj_cmd_success(&workspace_root, &["branch", "my-branch"]);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Skipping branch 'my-branch' since it points to an open commit.
    Nothing changed.
    "###);

    // When pushing a specific branch, won't push it if it points to an open commit
    let assert = test_env
        .jj_cmd(&workspace_root, &["git", "push", "--branch", "my-branch"])
        .assert()
        .failure();
    insta::assert_snapshot!(get_stdout_string(&assert), @"Error: Won't push open commit
");

    // Try pushing a conflict
    std::fs::write(workspace_root.join("file"), "first").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "first"]);
    std::fs::write(workspace_root.join("file"), "second").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "second"]);
    std::fs::write(workspace_root.join("file"), "third").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["rebase", "-d", "@--"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "third"]);
    let assert = test_env
        .jj_cmd(&workspace_root, &["git", "push"])
        .assert()
        .failure();
    insta::assert_snapshot!(get_stdout_string(&assert), @"Error: Won't push commit 28b5642cb786 since it has conflicts
");
}
