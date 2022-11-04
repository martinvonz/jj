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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_templater_branches() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");
    // TODO: This initial export shouldn't be needed
    test_env.jj_cmd_success(&origin_path, &["git", "export"]);

    // Created some branches on the remote
    test_env.jj_cmd_success(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch1"]);
    test_env.jj_cmd_success(&origin_path, &["new", "root", "-m=description 2"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch2"]);
    test_env.jj_cmd_success(&origin_path, &["new", "root", "-m=description 3"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch3"]);
    test_env.jj_cmd_success(&origin_path, &["git", "export"]);
    test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "git",
            "clone",
            origin_git_repo_path.to_str().unwrap(),
            "local",
        ],
    );
    let workspace_root = test_env.env_root().join("local");

    // Rewrite branch1, move branch2 forward, create conflict in branch3, add
    // new-branch
    test_env.jj_cmd_success(
        &workspace_root,
        &["describe", "branch1", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_success(&workspace_root, &["new", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "new-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["describe", "branch3", "-m=local"]);
    test_env.jj_cmd_success(&origin_path, &["describe", "branch3", "-m=origin"]);
    test_env.jj_cmd_success(&origin_path, &["git", "export"]);
    test_env.jj_cmd_success(&workspace_root, &["git", "fetch"]);

    let output = test_env.jj_cmd_success(
        &workspace_root,
        &["log", "-T", r#"commit_id.short() " " branches"#],
    );
    insta::assert_snapshot!(output, @r###"
    o 212985c08a44 branch3?
    | @ cbf02da4e154 branch2* new-branch
    | | o c794a4eab3b9 branch1*
    | |/  
    |/|   
    | o 8cd8e5dc9595 branch2@origin
    |/  
    o 000000000000 
    "###);
}
