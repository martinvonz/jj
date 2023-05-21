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

use std::path::Path;

use itertools::Itertools as _;

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
    ◉  message 2
    │ @  message 1
    ├─╯
    ◉
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
    insta::assert_snapshot!(stdout, @r###"
    @  cde29280d4a9 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 123ed18e4c4c0d77428df41112bc02ffc83fb935
    │  args: jj describe -m initial
    ◉  7c212e0863fd test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj describe -m initial
    ◉  a99a3fd5c51e test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  56b94dfc38e7 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
    let op_id_hex = stdout[3..15].to_string();

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "rewritten"]);
    test_env.jj_cmd_success(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child"],
    );

    // We should be informed about the concurrent modification
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    Concurrent modification detected, resolving automatically.
    Rebased 1 descendant commits onto commits rewritten by other operation
    ◉  3f06323826b4a293a9ee6d24cc0e07ad2961b5d5 new child
    @  d91437157468ec86bbbc9e6a14a60d3e8d1790ac rewritten
    ◉  0000000000000000000000000000000000000000
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
    let op_id_hex = stdout[3..15].to_string();

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
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    Concurrent modification detected, resolving automatically.
    @  4eb0610031b7cd148ff9f729a673a3f815033170 new child1
    │ ◉  4b20e61d23ee7d7c4d5e61e11e97c26e716f9c30 new child2
    ├─╯
    ◉  52c893bf3cd201e215b23e084e8a871244ca14d5 initial
    ◉  0000000000000000000000000000000000000000
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
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log", "-Tdescription"]);
    insta::assert_snapshot!(stdout, @r###"
    @  snapshot working copy
    ◉    resolve concurrent operations
    ├─╮
    ◉ │  new empty commit
    │ ◉  new empty commit
    ├─╯
    ◉  describe commit cf911c223d3e24e001fc8264d6dbf0610804fc40
    ◉  snapshot working copy
    ◉  add workspace 'default'
    ◉  initialize repo
    "###);
}

#[test]
fn test_concurrent_snapshot_wc_reloadable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let op_heads_dir = repo_path
        .join(".jj")
        .join("repo")
        .join("op_heads")
        .join("heads");

    std::fs::write(repo_path.join("base"), "").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "initial"]);

    // Create new commit and checkout it.
    std::fs::write(repo_path.join("child1"), "").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "new child1"]);

    let template = r#"id ++ "\n" ++ description ++ "\n" ++ tags"#;
    let op_log_stdout = test_env.jj_cmd_success(&repo_path, &["op", "log", "-T", template]);
    insta::assert_snapshot!(op_log_stdout, @r###"
    @  bacc8f507ccede29c282bb1459b71ffd233a9e29f4ec11b027422923592c4ef949e4465fd101c99ee6cfd39af26f29cd5910ef3b16985538e50fd21523bcf3e1
    │  commit 323b414dd255b51375d7f4392b7b2641ffe4289f
    │  args: jj commit -m 'new child1'
    ◉  6aa8b9099660e021813be72b4230692f782699448947cc76ffe5245d23108184dd60bb795fba350eb39abc2b0643169623cf59bb0c04130102388f8d87791070
    │  snapshot working copy
    │  args: jj commit -m 'new child1'
    ◉  7a31786821de03db03d5b9b7ec97454d10b90eee33844a86ebd282124c8ab43babcf79091cd1c47796ed8cc6ff23febabb35a10e0aad1f96428f958eb834b30d
    │  commit 3d918700494a9895696e955b85fa05eb0d314cc6
    │  args: jj commit -m initial
    ◉  36440b76f049c4999505fa1dd3b21bfdb1a1f93c64e04f0a3797e704145780df959afcd3babd59c536544c6e7a6b883594abe7b38e8c4be5fe6d66a3cd8f4f9d
    │  snapshot working copy
    │  args: jj commit -m initial
    ◉  a99a3fd5c51e8f7ccb9ae2f9fb749612a23f0a7cf25d8c644f36c35c077449ce3c66f49d098a5a704ca5e47089a7f019563a5b8cbc7d451619e0f90c82241ceb
    │  add workspace 'default'
    ◉  56b94dfc38e7d54340377f566e96ab97dc6163ea7841daf49fb2e1d1ceb27e26274db1245835a1a421fb9d06e6e0fe1e4f4aa1b0258c6e86df676ad9111d0dab
       initialize repo
    "###);
    let op_log_lines = op_log_stdout.lines().collect_vec();
    let current_op_id = op_log_lines[0].split_once("  ").unwrap().1;
    let previous_op_id = op_log_lines[6].split_once("  ").unwrap().1;

    // Another process started from the "initial" operation, but snapshots after
    // the "child1" checkout has been completed.
    std::fs::rename(
        op_heads_dir.join(current_op_id),
        op_heads_dir.join(previous_op_id),
    )
    .unwrap();
    std::fs::write(repo_path.join("child2"), "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "new child2"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 4011424ea0a2 new child2
    Parent commit      : e08863ee7a0d new child1
    "###);

    // Since the repo can be reloaded before snapshotting, "child2" should be
    // a child of "child1", not of "initial".
    let template = r#"commit_id ++ " " ++ description"#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template, "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @  4011424ea0a210a914f869ea3c47d76931598d1d new child2
    │  A child2
    ◉  e08863ee7a0df688755d3d3126498afdf4f580ad new child1
    │  A child1
    ◉  79989e62f8331e69a803058b57bacc264405cb65 initial
    │  A base
    ◉  0000000000000000000000000000000000000000
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
