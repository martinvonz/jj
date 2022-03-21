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

use jujutsu::testutils::TestEnvironment;

#[test]
fn test_concurrent_operations() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let op_id_hex = stdout[2..14].to_string();

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "message 1"]);
    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "message 2", "--at-op", &op_id_hex],
    );

    // We should be informed about the concurrent modification
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    o message 2
    | @ message 1
    |/  
    o 
    "###);
}
