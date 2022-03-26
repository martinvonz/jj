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
fn test_new_with_message() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "a new commit"]);

    let assert = test_env
        .jj_cmd(&repo_path, &["log", "-T", "commit_id \" \" description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    @ 588665964c5aaf306d9617095c8a9c4447cdb30c a new commit
    o f0f3ab56bfa927e3a65c2ac9a513693d438e271b add a file
    o 0000000000000000000000000000000000000000 
    "###);
}
