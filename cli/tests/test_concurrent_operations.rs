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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "message 1"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "-m", "message 2", "--at-op", "@-"],
    );

    // "op log" doesn't merge the concurrent operations
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  48f4a48f3f70 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'message 2' --at-op @-
    │ ◉  e31015019d90 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    ├─╯  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │    args: jj describe -m 'message 1'
    ◉  b51416386f26 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  9a7d829846af test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "initial"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  66d1dd775c54 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 4e8f9d2be039994f589b4e57ac5e9488703e604d
    │  args: jj describe -m initial
    ◉  130d67859810 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj describe -m initial
    ◉  b51416386f26 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  9a7d829846af test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
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
    ◉  db141860e12c2d5591c56fde4fc99caf71cec418 new child
    @  07c3641e495cce57ea4ca789123b52f421c57aa2 rewritten
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  4eadcf3df11f46ef3d825c776496221cc8303053 new child1
    │ ◉  68119f1643b7e3c301c5f7c2b6c9bf4ccba87379 new child2
    ├─╯
    ◉  2ff7ae858a3a11837fdf9d1a76be295ef53f1bb3 initial
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file b/file
    index 12f00e90b6..2e0996000b 100644
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
    ◉  describe commit 506f4ec3c2c62befa15fabc34ca9d4e6d7bef254
    ◉  snapshot working copy
    ◉  add workspace 'default'
    ◉  initialize repo
    ◉
    "###);
}

#[test]
fn test_concurrent_snapshot_wc_reloadable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  9f11958bcf79340028eeabf9b0381cd8d2ae2258d0097b8ce8bd24fe7138eca08d9eb113bb4722ebacd9b7a6fa017e3888f72907be7487f275823c8d21359eed
    │  commit 554d22b2c43c1c47e279430197363e8daabe2fd6
    │  args: jj commit -m 'new child1'
    ◉  f5460e8f43a04fbc61553d12fa5ba8d3b12e4fdcfda1999db6b67cc8e1e473b7e62cc0536196a53b84f34e18c1c6d608f427bb64bd5f834f845a9859e39cb320
    │  snapshot working copy
    │  args: jj commit -m 'new child1'
    ◉  49359b6597ead3fbb66802a6bbd8761c0ad4646a2b089090d6fd72fb6e2568aa99c4a92f9f1f252a83cce56ec84961c36e85f731f19fc5a4c24d6a3f7282b774
    │  commit de71e09289762a65f80bb1c3dae2a949df6bcde7
    │  args: jj commit -m initial
    ◉  86dbba2b96a4a801abef7f77f8fdf338b6e36f81ea4a531aacf06acbd06f4037731fffef42503c2225fdb206488971c1601ca8b2b4a83a3fe2dce64ee4db085e
    │  snapshot working copy
    │  args: jj commit -m initial
    ◉  b51416386f2685fd5493f2b20e8eec3c24a1776d9e1a7cb5ed7e30d2d9c88c0c1e1fe71b0b7358cba60de42533d1228ed9878f2f89817d892c803395ccf9fe92
    │  add workspace 'default'
    ◉  9a7d829846af88a2f7a1e348fb46ff58729e49632bc9c6a052aec8501563cb0d10f4a4e6010ffde529f84a2b9b5b3a4c211a889106a41f6c076dfdacc79f6af7
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
    Working copy now at: kkmpptxz 1795621b new child2
    Parent commit      : rlvkpnrz 86f54245 new child1
    "###);

    // Since the repo can be reloaded before snapshotting, "child2" should be
    // a child of "child1", not of "initial".
    let template = r#"commit_id ++ " " ++ description"#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template, "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @  1795621b54f4ebb435978b65d66bc0f90d8f20b6 new child2
    │  A child2
    ◉  86f54245e13f850f8275b5541e56da996b6a47b7 new child1
    │  A child1
    ◉  84f07f6bca2ffeddac84a8b09f60c6b81112375c initial
    │  A base
    ◉  0000000000000000000000000000000000000000
    "###);
}

fn get_log_output_with_stderr(test_env: &TestEnvironment, cwd: &Path) -> (String, String) {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_ok(cwd, &["log", "-T", template])
}
