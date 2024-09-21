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

    // "--at-op=@" disables op heads merging, and prints head operation ids.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["op", "log", "--at-op=@"]);
    insta::assert_snapshot!(stderr, @r#"
    Error: The "@" expression resolved to more than one operation
    Hint: Try specifying one of the operations by ID: 0162305507cc, d74dff64472e
    "#);

    // "op log --at-op" should work without merging the head operations
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log", "--at-op=d74dff64472e"]);
    insta::assert_snapshot!(stdout, @r#"
    @  d74dff64472e test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'message 2' --at-op @-
    ○  eac759b9ab75 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ○  000000000000 root()
    "#);

    // We should be informed about the concurrent modification
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r#"
    @  message 1
    │ ○  message 2
    ├─╯
    ◆
    "#);
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
    insta::assert_snapshot!(stdout, @r#"
    @  c62ace5c0522 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 4e8f9d2be039994f589b4e57ac5e9488703e604d
    │  args: jj describe -m initial
    ○  82d32fc68fc3 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj describe -m initial
    ○  eac759b9ab75 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ○  000000000000 root()
    "#);
    let op_id_hex = stdout[3..15].to_string();

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "rewritten"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "--at-op", &op_id_hex, "-m", "new child"],
    );

    // We should be informed about the concurrent modification
    let (stdout, stderr) = get_log_output_with_stderr(&test_env, &repo_path);
    insta::assert_snapshot!(stdout, @r###"
    ○  db141860e12c2d5591c56fde4fc99caf71cec418 new child
    @  07c3641e495cce57ea4ca789123b52f421c57aa2 rewritten
    ◆  0000000000000000000000000000000000000000
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
    │ ○  68119f1643b7e3c301c5f7c2b6c9bf4ccba87379 new child2
    ├─╯
    ○  2ff7ae858a3a11837fdf9d1a76be295ef53f1bb3 initial
    ◆  0000000000000000000000000000000000000000
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
    insta::assert_snapshot!(stdout, @r#"
    @  snapshot working copy
    ○    reconcile divergent operations
    ├─╮
    ○ │  new empty commit
    │ ○  new empty commit
    ├─╯
    ○  describe commit 506f4ec3c2c62befa15fabc34ca9d4e6d7bef254
    ○  snapshot working copy
    ○  add workspace 'default'
    ○
    "#);
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
    insta::assert_snapshot!(op_log_stdout, @r#"
    @  ec6bf266624bbaed55833a34ae62fa95c0e9efa651b94eb28846972da645845052dcdc8580332a5628849f23f48b9e99fc728dc3fb13106df8d0666d746f8b85
    │  commit 554d22b2c43c1c47e279430197363e8daabe2fd6
    │  args: jj commit -m 'new child1'
    ○  23858df860b789e8176a73c0eb21804e3f1848f26d68b70d234c004d08980c41499b6669042bca20fbc2543c437222a084c7cd473e91c7a9a095a02bf38544ab
    │  snapshot working copy
    │  args: jj commit -m 'new child1'
    ○  e1db5fa988fc66e5cc0491b00c53fb93e25e730341c850cb42e1e0db0c76d2b4065005787563301b1d292c104f381918897f7deabeb92d2532f42ce75d3fe588
    │  commit de71e09289762a65f80bb1c3dae2a949df6bcde7
    │  args: jj commit -m initial
    ○  7de878155a459b7751097222132c935f9dcbb8f69a72b0f3a9036345a963010a553dc7c92964220128679ead72b087ca3aaf4ab9e20a221d1ffa4f9e92a32193
    │  snapshot working copy
    │  args: jj commit -m initial
    ○  eac759b9ab75793fd3da96e60939fb48f2cd2b2a9c1f13ffe723cf620f3005b8d3e7e923634a07ea39513e4f2f360c87b9ad5d331cf90d7a844864b83b72eba1
    │  add workspace 'default'
    ○  00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000

    "#);
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
    ○  86f54245e13f850f8275b5541e56da996b6a47b7 new child1
    │  A child1
    ○  84f07f6bca2ffeddac84a8b09f60c6b81112375c initial
    │  A base
    ◆  0000000000000000000000000000000000000000
    "###);
}

fn get_log_output_with_stderr(test_env: &TestEnvironment, cwd: &Path) -> (String, String) {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_ok(cwd, &["log", "-T", template])
}
