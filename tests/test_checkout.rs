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

use std::path::Path;

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_checkout() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        br#"[ui]
    enable-open-commits = true
    "#,
    );

    test_env.jj_cmd_success(&repo_path, &["close", "-m", "closed"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "open"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "open"]);

    // Check out current commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["checkout", "@"]);
    insta::assert_snapshot!(stdout, @r###"
    Already on that commit
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 169fa76981bcf302d1a96952bdf32a8da79ab084 open
    o b4c967d9c9a9e8b523b0a9b52879b3337a3e67a9 closed
    o 0000000000000000000000000000000000000000 (no description set)
    "###);

    // When checking out a closed commit, a new commit is created on top of it
    test_env.jj_cmd_success(&repo_path, &["checkout", "@-"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 5a38be51f15b107b7c7e89c06c0ab626f1457128 (no description set)
    | o 169fa76981bcf302d1a96952bdf32a8da79ab084 open
    |/  
    o b4c967d9c9a9e8b523b0a9b52879b3337a3e67a9 closed
    o 0000000000000000000000000000000000000000 (no description set)
    "###);

    // When checking out an open commit, the specified commit is edited directly
    test_env.jj_cmd_success(&repo_path, &["checkout", "open"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 169fa76981bcf302d1a96952bdf32a8da79ab084 open
    o b4c967d9c9a9e8b523b0a9b52879b3337a3e67a9 closed
    o 0000000000000000000000000000000000000000 (no description set)
    "###);

    // With ui.enable-open-commits=false, checking out an open commit also results
    // in a commit on top
    test_env.add_config(
        br#"[ui]
    enable-open-commits = false
    "#,
    );
    test_env.jj_cmd_success(&repo_path, &["checkout", "open"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 37b7bc83cf288eef68564044a9ac0ec6c5df34f0 (no description set)
    o 169fa76981bcf302d1a96952bdf32a8da79ab084 open
    o b4c967d9c9a9e8b523b0a9b52879b3337a3e67a9 closed
    o 0000000000000000000000000000000000000000 (no description set)
    "###);

    // Can provide a description
    test_env.jj_cmd_success(&repo_path, &["checkout", "@-", "-m", "my message"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 14a7f0fd8f8a8235efdf4b20635567ebcf5c9776 my message
    o 169fa76981bcf302d1a96952bdf32a8da79ab084 open
    o b4c967d9c9a9e8b523b0a9b52879b3337a3e67a9 closed
    o 0000000000000000000000000000000000000000 (no description set)
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    test_env.jj_cmd_success(cwd, &["log", "-T", r#"commit_id " " description"#])
}
