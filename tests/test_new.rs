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
fn test_new_with_message() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "a new commit"]);

    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id \" \" description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    @ 88436dbcdbedc2b8a6ebd0687981906d09ccc68f a new commit
    o 51e9c5819117991e4a6dc5a4a744283fc74f0746 add a file
    o 0000000000000000000000000000000000000000 
    "###);
}
