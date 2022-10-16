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

use regex::Regex;

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_concurrent_operation_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "message 1"]);
    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "message 2", "--at-op", "@-"],
    );

    // We should be informed about the concurrent modification
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    o message 2
    | @ message 1
    |/  
    o (no description set)
    "###);
}

#[test]
fn test_concurrent_operations_auto_rebase() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let op_id_hex = stdout[2..14].to_string();

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "rewritten"]);
    test_env.jj_cmd_success(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child"],
    );

    // We should be informed about the concurrent modification
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id \" \" description"]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    Rebased 1 descendant commits onto commits rewritten by other operation
    o 4eeb7d76372418118a91c34f09e5e3936f0deeb5 new child
    @ 14176aeadc0259b2150fc7374969e74b1552a498 rewritten
    o 0000000000000000000000000000000000000000 (no description set)
    "###);
}

#[test]
fn test_concurrent_operations_wc_modified() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let op_id_hex = stdout[2..14].to_string();

    test_env.jj_cmd_success(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child1"],
    );
    test_env.jj_cmd_success(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child2"],
    );
    std::fs::write(repo_path.join("file"), "modified\n").unwrap();

    // We should be informed about the concurrent modification
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id \" \" description"]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    @ 304d2b4b70536e0bfd7a38394db584ee069a3b1a new child1
    | o ac08e56f9b802269864c5061f2a7305b9258a671 new child2
    |/  
    o 5af56dcc2cc27bb234e5574b5a3ebc5f22081462 initial
    o 0000000000000000000000000000000000000000 (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file b/file
    index 12f00e90b6...2e0996000b 100644
    --- a/file
    +++ b/file
    @@ -1,1 +1,1 @@
    -contents
    +modified
    "###);

    // The working copy should be committed after merging the operations
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(redact_op_log(&stdout), @r###"
    @ 
    | commit working copy
    o   
    |\  resolve concurrent operations
    | | 
    o | 
    | | new empty commit
    | | 
    | o 
    |/  new empty commit
    |   
    o 
    | describe commit cf911c223d3e24e001fc8264d6dbf0610804fc40
    | 
    o 
    | commit working copy
    o 
    | 
    o 
      initialize repo
    "###);
}

fn redact_op_log(stdout: &str) -> String {
    let mut lines = vec![];
    // Filter out the operation id etc, and the CLI arguments
    let unwanted = Regex::new(r" ([0-9a-f]+|args:) .*").unwrap();
    for line in stdout.lines() {
        lines.push(unwanted.replace(line, " ").to_string());
    }
    lines.join("\n")
}
