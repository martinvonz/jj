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

#[test]
fn test_concurrent_operation_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "message 1"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "-m", "message 2", "--at-op", "@-"],
    );

    // "op log" doesn't merge the concurrent operations
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  1e346ac76e7c test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'message 2' --at-op @-
    │ ◉  1fb59888922c test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    ├─╯  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │    args: jj describe -m 'message 1'
    ◉  6ac4339ad699 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  1b0049c19762 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  initialize repo
    ◉  000000000000 root()
    "###);

    // We should be informed about the concurrent modification
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  message 2
    │ @  message 1
    ├─╯
    ◉
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    "###);
}

#[test]
fn test_concurrent_operations_auto_rebase() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "initial"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  faefa12d4ced test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 123ed18e4c4c0d77428df41112bc02ffc83fb935
    │  args: jj describe -m initial
    ◉  75021f3fc19d test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj describe -m initial
    ◉  6ac4339ad699 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  1b0049c19762 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  initialize repo
    ◉  000000000000 root()
    "###);
    let op_id_hex = stdout[3..15].to_string();

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "rewritten"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child"],
    );

    // We should be informed about the concurrent modification
    let (stdout, stderr) = get_log_output_with_stderr(&test_env, &repo_path);
    insta::assert_snapshot!(stdout, @r###"
    ◉  3f06323826b4a293a9ee6d24cc0e07ad2961b5d5 new child
    @  d91437157468ec86bbbc9e6a14a60d3e8d1790ac rewritten
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    Rebased 1 descendant commits onto commits rewritten by other operation
    "###);
}

#[test]
fn test_concurrent_operations_wc_modified() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "initial"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let op_id_hex = stdout[3..15].to_string();

    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child1"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child2"],
    );
    std::fs::write(repo_path.join("file"), "modified\n").unwrap();

    // We should be informed about the concurrent modification
    let (stdout, stderr) = get_log_output_with_stderr(&test_env, &repo_path);
    insta::assert_snapshot!(stdout, @r###"
    @  4eb0610031b7cd148ff9f729a673a3f815033170 new child1
    │ ◉  4b20e61d23ee7d7c4d5e61e11e97c26e716f9c30 new child2
    ├─╯
    ◉  52c893bf3cd201e215b23e084e8a871244ca14d5 initial
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
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
    ◉
    "###);
}

#[test]
fn test_concurrent_snapshot_wc_reloadable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let op_heads_dir = repo_path
        .join(".jj")
        .join("repo")
        .join("op_heads")
        .join("heads");

    std::fs::write(repo_path.join("base"), "").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "initial"]);

    // Create new commit and checkout it.
    std::fs::write(repo_path.join("child1"), "").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "new child1"]);

    let template = r#"id ++ "\n" ++ description ++ "\n" ++ tags"#;
    let op_log_stdout = test_env.jj_cmd_success(&repo_path, &["op", "log", "-T", template]);
    insta::assert_snapshot!(op_log_stdout, @r###"
    @  bc38b5d20b2843d546bf5d40d8e1897be131b2ede73b4de031d3b2c6ac394e454a24f52196687accc8d27eb99aaadd8e1d63a8d30a2cb22efae0e3232492bc86
    │  commit 323b414dd255b51375d7f4392b7b2641ffe4289f
    │  args: jj commit -m 'new child1'
    ◉  af69a63bdc3c88f12e0b1fbc673bce01eb46087b8c6629efd57cb9e68d72973d928f738f2e51504959dc5f531f68a15321f6c9560bca9862f5e850a872ce0c30
    │  snapshot working copy
    │  args: jj commit -m 'new child1'
    ◉  2af019579b0b19d9a147df4779e587b92061fb5cd18fe9ba6d8a33cbeac205ced27c7f411df78278626993725ace7738da9352addb6699898e252c75f57f6f93
    │  commit 3d918700494a9895696e955b85fa05eb0d314cc6
    │  args: jj commit -m initial
    ◉  c07152a7412fac7a7b5570176a1f8799e878c9ec75c7f69633943e6d59a555b7b778804c5b930ce3528e787c5e221c3874b470abb1ff97a2663a4df21f9e09d2
    │  snapshot working copy
    │  args: jj commit -m initial
    ◉  6ac4339ad6999058dd1806653ec37fc0091c1cc17419c750fddc5e8c1a6a77829e6dd70b3408403fb2c0b9839cf6bfd1c270f980674f7f89d4d78dc54082a8ef
    │  add workspace 'default'
    ◉  1b0049c19762e43499f2499a45afc9f72b3004d75a2863d41d8867cfafb9bbc8e16aa447107e460d58a5c1462429f032d806f7487836c66c6f351df45746c218
    │  initialize repo
    ◉  00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000

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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "new child2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz 4011424e new child2
    Parent commit      : rlvkpnrz e08863ee new child1
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

fn get_log_output_with_stderr(test_env: &TestEnvironment, cwd: &Path) -> (String, String) {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_ok(cwd, &["log", "-T", template])
}
