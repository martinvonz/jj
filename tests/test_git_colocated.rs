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
fn test_git_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_success(&workspace_root, &["init", "--git-repo", "."]);

    // Create a commit from jj and check that it's reflected in git
    std::fs::write(workspace_root.join("new-file"), "contents").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "add a file"]);
    test_env.jj_cmd_success(&workspace_root, &["git", "import"]);
    let stdout =
        test_env.jj_cmd_success(&workspace_root, &["log", "-T", "commit_id \" \" branches"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 2588800a4ee68926773f1e9c44dcc50ada923650 
    o 172b1cbfe88c97cbd1b1c8a98a48e729a4540e85 master
    o 0000000000000000000000000000000000000000 
    "###);
    assert_eq!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        "172b1cbfe88c97cbd1b1c8a98a48e729a4540e85".to_string()
    );
}
