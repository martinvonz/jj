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

use crate::common::TestEnvironment;

/// Test adding a second workspace
#[test]
fn test_workspaces_add_second_workspace() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "initial"]);

    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz 8183d0fc (empty) (no description set)
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &main_path,
        &["workspace", "add", "--name", "second", "../secondary"],
    );
    insta::assert_snapshot!(stdout.replace('\\', "/"), @"");
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Created workspace in "../secondary"
    Working copy now at: rzvqmyuk 5ed2222c (empty) (no description set)
    Parent commit      : qpvuntsm 751b12b7 initial
    Added 1 files, modified 0 files, removed 0 files
    "###);

    // Can see the working-copy commit in each workspace in the log output. The "@"
    // node in the graph indicates the current workspace's working-copy commit.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  5ed2222c28e2 second@
    │ @  8183d0fcaa4c default@
    ├─╯
    ○  751b12b7b981
    ◆  000000000000
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &secondary_path), @r###"
    @  5ed2222c28e2 second@
    │ ○  8183d0fcaa4c default@
    ├─╯
    ○  751b12b7b981
    ◆  000000000000
    "###);

    // Both workspaces show up when we list them
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz 8183d0fc (empty) (no description set)
    second: rzvqmyuk 5ed2222c (empty) (no description set)
    "###);
}

/// Test how sparse patterns are inherited
#[test]
fn test_workspaces_sparse_patterns() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "ws1"]);
    let ws1_path = test_env.env_root().join("ws1");
    let ws2_path = test_env.env_root().join("ws2");
    let ws3_path = test_env.env_root().join("ws3");

    test_env.jj_cmd_ok(&ws1_path, &["sparse", "set", "--clear", "--add=foo"]);
    test_env.jj_cmd_ok(&ws1_path, &["workspace", "add", "../ws2"]);
    let stdout = test_env.jj_cmd_success(&ws2_path, &["sparse", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    foo
    "###);
    test_env.jj_cmd_ok(&ws2_path, &["sparse", "set", "--add=bar"]);
    test_env.jj_cmd_ok(&ws2_path, &["workspace", "add", "../ws3"]);
    let stdout = test_env.jj_cmd_success(&ws3_path, &["sparse", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    bar
    foo
    "###);
}

/// Test adding a second workspace while the current workspace is editing a
/// merge
#[test]
fn test_workspaces_add_second_workspace_on_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    test_env.jj_cmd_ok(&main_path, &["describe", "-m=left"]);
    test_env.jj_cmd_ok(&main_path, &["new", "@-", "-m=right"]);
    test_env.jj_cmd_ok(&main_path, &["new", "all:@-+", "-m=merge"]);

    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: zsuskuln 35e47bff (empty) merge
    "###);

    test_env.jj_cmd_ok(
        &main_path,
        &["workspace", "add", "--name", "second", "../secondary"],
    );

    // The new workspace's working-copy commit shares all parents with the old one.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○    7013a493bd09 second@
    ├─╮
    │ │ @  35e47bff781e default@
    ╭─┬─╯
    │ ○  444b77e99d43
    ○ │  1694f2ddf8ec
    ├─╯
    ◆  000000000000
    "###);
}

/// Test adding a workspace, but at a specific revision using '-r'
#[test]
fn test_workspaces_add_workspace_at_revision() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file-1"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "first"]);

    std::fs::write(main_path.join("file-2"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: kkmpptxz dadeedb4 (empty) (no description set)
    "###);

    let (_, stderr) = test_env.jj_cmd_ok(
        &main_path,
        &[
            "workspace",
            "add",
            "--name",
            "second",
            "../secondary",
            "-r",
            "@--",
        ],
    );
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Created workspace in "../secondary"
    Working copy now at: zxsnswpr e374e74a (empty) (no description set)
    Parent commit      : qpvuntsm f6097c2f first
    Added 1 files, modified 0 files, removed 0 files
    "###);

    // Can see the working-copy commit in each workspace in the log output. The "@"
    // node in the graph indicates the current workspace's working-copy commit.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  e374e74aa0c8 second@
    │ @  dadeedb493e8 default@
    │ ○  c420244c6398
    ├─╯
    ○  f6097c2f7cac
    ◆  000000000000
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &secondary_path), @r###"
    @  e374e74aa0c8 second@
    │ ○  dadeedb493e8 default@
    │ ○  c420244c6398
    ├─╯
    ○  f6097c2f7cac
    ◆  000000000000
    "###);
}

/// Test multiple `-r` flags to `workspace add` to create a workspace
/// working-copy commit with multiple parents.
#[test]
fn test_workspaces_add_workspace_multiple_revisions() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    std::fs::write(main_path.join("file-1"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&main_path, &["new", "-r", "root()"]);

    std::fs::write(main_path.join("file-2"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&main_path, &["new", "-r", "root()"]);

    std::fs::write(main_path.join("file-3"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&main_path, &["new", "-r", "root()"]);

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    @  5b36783cd11c
    │ ○  6c843d62ca29
    ├─╯
    │ ○  544cd61f2d26
    ├─╯
    │ ○  f6097c2f7cac
    ├─╯
    ◆  000000000000
    "###);

    let (_, stderr) = test_env.jj_cmd_ok(
        &main_path,
        &[
            "workspace",
            "add",
            "--name=merge",
            "../merged",
            "-r=description(third)",
            "-r=description(second)",
            "-r=description(first)",
        ],
    );
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Created workspace in "../merged"
    Working copy now at: wmwvqwsz f4fa64f4 (empty) (no description set)
    Parent commit      : mzvwutvl 6c843d62 third
    Parent commit      : kkmpptxz 544cd61f second
    Parent commit      : qpvuntsm f6097c2f first
    Added 3 files, modified 0 files, removed 0 files
    "###);

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○      f4fa64f40944 merge@
    ├─┬─╮
    │ │ ○  f6097c2f7cac
    │ ○ │  544cd61f2d26
    │ ├─╯
    ○ │  6c843d62ca29
    ├─╯
    │ @  5b36783cd11c default@
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_workspaces_add_workspace_from_subdir() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let subdir_path = main_path.join("subdir");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::create_dir(&subdir_path).unwrap();
    std::fs::write(subdir_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "initial"]);

    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz e1038e77 (empty) (no description set)
    "###);

    // Create workspace while in sub-directory of current workspace
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&subdir_path, &["workspace", "add", "../../secondary"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @"");
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Created workspace in "../../secondary"
    Working copy now at: rzvqmyuk 7ad84461 (empty) (no description set)
    Parent commit      : qpvuntsm a3a43d9e initial
    Added 1 files, modified 0 files, removed 0 files
    "###);

    // Both workspaces show up when we list them
    let stdout = test_env.jj_cmd_success(&secondary_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz e1038e77 (empty) (no description set)
    secondary: rzvqmyuk 7ad84461 (empty) (no description set)
    "###);
}

/// Test making changes to the working copy in a workspace as it gets rewritten
/// from another workspace
#[test]
fn test_workspaces_conflicting_edits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "contents\n").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);

    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../secondary"]);

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  3224de8ae048 secondary@
    │ @  06b57f44a3ca default@
    ├─╯
    ○  506f4ec3c2c6
    ◆  000000000000
    "###);

    // Make changes in both working copies
    std::fs::write(main_path.join("file"), "changed in main\n").unwrap();
    std::fs::write(secondary_path.join("file"), "changed in second\n").unwrap();
    // Squash the changes from the main workspace into the initial commit (before
    // running any command in the secondary workspace
    let (stdout, stderr) = test_env.jj_cmd_ok(&main_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl a58c9a9b (empty) (no description set)
    Parent commit      : qpvuntsm d4124476 (no description set)
    "###);

    // The secondary workspace's working-copy commit was updated
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    @  a58c9a9b19ce default@
    │ ○  e82cd4ee8faa secondary@
    ├─╯
    ○  d41244767d45
    ◆  000000000000
    "###);
    let stderr = test_env.jj_cmd_failure(&secondary_path, &["st"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The working copy is stale (not updated since operation 0da24da631e3).
    Hint: Run `jj workspace update-stale` to update it.
    See https://github.com/martinvonz/jj/blob/main/docs/working-copy.md#stale-working-copy for more information.
    "###);
    // Same error on second run, and from another command
    let stderr = test_env.jj_cmd_failure(&secondary_path, &["log"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The working copy is stale (not updated since operation 0da24da631e3).
    Hint: Run `jj workspace update-stale` to update it.
    See https://github.com/martinvonz/jj/blob/main/docs/working-copy.md#stale-working-copy for more information.
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&secondary_path, &["workspace", "update-stale"]);
    // It was detected that the working copy is now stale.
    // Since there was an uncommitted change in the working copy, it should
    // have been committed first (causing divergence)
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    Rebased 1 descendant commits onto commits rewritten by other operation
    Working copy now at: pmmvwywv?? e82cd4ee (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &secondary_path),
    @r###"
    ×  a28c85ce128b (divergent)
    │ ○  a58c9a9b19ce default@
    ├─╯
    │ @  e82cd4ee8faa secondary@ (divergent)
    ├─╯
    ○  d41244767d45
    ◆  000000000000
    "###);
    // The stale working copy should have been resolved by the previous command
    let stdout = get_log_output(&test_env, &secondary_path);
    assert!(!stdout.starts_with("The working copy is stale"));
    insta::assert_snapshot!(stdout, @r###"
    ×  a28c85ce128b (divergent)
    │ ○  a58c9a9b19ce default@
    ├─╯
    │ @  e82cd4ee8faa secondary@ (divergent)
    ├─╯
    ○  d41244767d45
    ◆  000000000000
    "###);
}

/// Test a clean working copy that gets rewritten from another workspace
#[test]
fn test_workspaces_updated_by_other() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "contents\n").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);

    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../secondary"]);

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  3224de8ae048 secondary@
    │ @  06b57f44a3ca default@
    ├─╯
    ○  506f4ec3c2c6
    ◆  000000000000
    "###);

    // Rewrite the check-out commit in one workspace.
    std::fs::write(main_path.join("file"), "changed in main\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&main_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl a58c9a9b (empty) (no description set)
    Parent commit      : qpvuntsm d4124476 (no description set)
    "###);

    // The secondary workspace's working-copy commit was updated.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    @  a58c9a9b19ce default@
    │ ○  e82cd4ee8faa secondary@
    ├─╯
    ○  d41244767d45
    ◆  000000000000
    "###);
    let stderr = test_env.jj_cmd_failure(&secondary_path, &["st"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The working copy is stale (not updated since operation 0da24da631e3).
    Hint: Run `jj workspace update-stale` to update it.
    See https://github.com/martinvonz/jj/blob/main/docs/working-copy.md#stale-working-copy for more information.
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&secondary_path, &["workspace", "update-stale"]);
    // It was detected that the working copy is now stale, but clean. So no
    // divergent commit should be created.
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: pmmvwywv e82cd4ee (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &secondary_path),
    @r###"
    ○  a58c9a9b19ce default@
    │ @  e82cd4ee8faa secondary@
    ├─╯
    ○  d41244767d45
    ◆  000000000000
    "###);
}

#[test]
fn test_workspaces_current_op_discarded_by_other() {
    let test_env = TestEnvironment::default();
    // Use the local backend because GitBackend::gc() depends on the git CLI.
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["init", "main", "--config-toml=ui.allow-init-native=true"],
    );
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("modified"), "base\n").unwrap();
    std::fs::write(main_path.join("deleted"), "base\n").unwrap();
    std::fs::write(main_path.join("sparse"), "base\n").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);
    std::fs::write(main_path.join("modified"), "main\n").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);

    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../secondary"]);
    // Make unsnapshotted writes in the secondary working copy
    test_env.jj_cmd_ok(
        &secondary_path,
        &[
            "sparse",
            "set",
            "--clear",
            "--add=modified",
            "--add=deleted",
            "--add=added",
        ],
    );
    std::fs::write(secondary_path.join("modified"), "secondary\n").unwrap();
    std::fs::remove_file(secondary_path.join("deleted")).unwrap();
    std::fs::write(secondary_path.join("added"), "secondary\n").unwrap();

    // Create an op by abandoning the parent commit. Importantly, that commit also
    // changes the target tree in the secondary workspace.
    test_env.jj_cmd_ok(&main_path, &["abandon", "@-"]);

    let stdout = test_env.jj_cmd_success(
        &main_path,
        &[
            "operation",
            "log",
            "--template",
            r#"id.short(10) ++ " " ++ description"#,
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  78e49a9ae1 abandon commit 3540d386892997a2a927078635a2d933e37499fb8691938a2f540c25bccffd9e8a60b2d5a8cb94bb3eeab17e1c56f96aafa2bcb66fa1e4eb96911d093d7a579e
    ○  083b1fe855 create initial working-copy commit in workspace secondary
    ○  aacb3bda7d add workspace 'secondary'
    ○  46bcf7d75e new empty commit
    ○  4d2f5d7cbf snapshot working copy
    ○  2f863a1573 new empty commit
    ○  f01631d976 snapshot working copy
    ○  17dbb2fe40 add workspace 'default'
    ○  cecfee9647 initialize repo
    ○  0000000000
    "###);

    // Abandon ops, including the one the secondary workspace is currently on.
    test_env.jj_cmd_ok(&main_path, &["operation", "abandon", "..@-"]);
    test_env.jj_cmd_ok(&main_path, &["util", "gc", "--expire=now"]);

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    @  cc0b087cb874 default@
    │ ○  376eee1462a7 secondary@
    ├─╯
    ○  7788883a847c
    ◆  000000000000
    "###);

    let stderr = test_env.jj_cmd_failure(&secondary_path, &["st"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Could not read working copy's operation.
    Hint: Run `jj workspace update-stale` to recover.
    See https://github.com/martinvonz/jj/blob/main/docs/working-copy.md#stale-working-copy for more information.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&secondary_path, &["workspace", "update-stale"]);
    insta::assert_snapshot!(stderr, @r###"
    Failed to read working copy's current operation; attempting recovery. Error message from read attempt: Object 083b1fe855c23361b504fc78eb827aaa6099067d5ccba28b4e8cd89ed35d850be68ea557f03aaeaa37a09cc91b185c03c7ac638fff1fd8234b2e2a9cc62dc2cf of type operation not found
    Created and checked out recovery commit 6803354995e6
    "###);
    insta::assert_snapshot!(stdout, @"");

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  a8f7db7868c1 secondary@
    ○  376eee1462a7
    │ @  cc0b087cb874 default@
    ├─╯
    ○  7788883a847c
    ◆  000000000000
    "###);

    // The sparse patterns should remain
    let stdout = test_env.jj_cmd_success(&secondary_path, &["sparse", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    added
    deleted
    modified
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&secondary_path, &["st"]);
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    A added
    D deleted
    M modified
    Working copy : kmkuslsw a8f7db78 (no description set)
    Parent commit: rzvqmyuk 376eee14 (empty) (no description set)
    "###);
    // The modified file should have the same contents it had before (not reset to
    // the base contents)
    insta::assert_snapshot!(std::fs::read_to_string(secondary_path.join("modified")).unwrap(), @r###"
    secondary
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&secondary_path, &["obslog"]);
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(stdout, @r###"
    @  kmkuslsw test.user@example.com 2001-02-03 08:05:18 secondary@ a8f7db78
    │  (no description set)
    ○  kmkuslsw hidden test.user@example.com 2001-02-03 08:05:18 68033549
       (empty) (no description set)
    "###);
}

#[test]
fn test_workspaces_update_stale_noop() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    let (stdout, stderr) = test_env.jj_cmd_ok(&main_path, &["workspace", "update-stale"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing to do (the working copy is not stale).
    "###);

    let stderr = test_env.jj_cmd_failure(
        &main_path,
        &["workspace", "update-stale", "--ignore-working-copy"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: This command must be able to update the working copy.
    Hint: Don't use --ignore-working-copy.
    "###);

    let stdout = test_env.jj_cmd_success(&main_path, &["op", "log", "-Tdescription"]);
    insta::assert_snapshot!(stdout, @r###"
    @  add workspace 'default'
    ○  initialize repo
    ○
    "###);
}

/// Test "update-stale" in a dirty, but not stale working copy.
#[test]
fn test_workspaces_update_stale_snapshot() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "changed in main\n").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);
    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../secondary"]);

    // Record new operation in one workspace.
    test_env.jj_cmd_ok(&main_path, &["new"]);

    // Snapshot the other working copy, which unfortunately results in concurrent
    // operations, but should be resolved cleanly.
    std::fs::write(secondary_path.join("file"), "changed in second\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&secondary_path, &["workspace", "update-stale"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    Nothing to do (the working copy is not stale).
    "###);

    insta::assert_snapshot!(get_log_output(&test_env, &secondary_path), @r###"
    @  e672fd8fefac secondary@
    │ ○  ea37b073f5ab default@
    │ ○  b13c81dedc64
    ├─╯
    ○  e6e9989f1179
    ◆  000000000000
    "###);
}

/// Test forgetting workspaces
#[test]
fn test_workspaces_forget() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);

    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../secondary"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&main_path, &["workspace", "forget"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");

    // When listing workspaces, only the secondary workspace shows up
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    secondary: pmmvwywv 18463f43 (empty) (no description set)
    "###);

    // `jj status` tells us that there's no working copy here
    let (stdout, stderr) = test_env.jj_cmd_ok(&main_path, &["st"]);
    insta::assert_snapshot!(stdout, @r###"
    No working copy
    "###);
    insta::assert_snapshot!(stderr, @"");

    // The old working copy doesn't get an "@" in the log output
    // TODO: It seems useful to still have the "secondary@" marker here even though
    // there's only one workspace. We should show it when the command is not run
    // from that workspace.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  18463f438cc9
    ○  4e8f9d2be039
    ◆  000000000000
    "###);

    // Revision "@" cannot be used
    let stderr = test_env.jj_cmd_failure(&main_path, &["log", "-r", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Workspace "default" doesn't have a working-copy commit
    "###);

    // Try to add back the workspace
    // TODO: We should make this just add it back instead of failing
    let stderr = test_env.jj_cmd_failure(&main_path, &["workspace", "add", "."]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Workspace already exists
    "###);

    // Add a third workspace...
    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../third"]);
    // ... and then forget it, and the secondary workspace too
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&main_path, &["workspace", "forget", "secondary", "third"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    // No workspaces left
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @"");
}

#[test]
fn test_workspaces_forget_multi_transaction() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);

    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../second"]);
    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../third"]);

    // there should be three workspaces
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz 909d51b1 (empty) (no description set)
    second: pmmvwywv 18463f43 (empty) (no description set)
    third: rzvqmyuk cc383fa2 (empty) (no description set)
    "###);

    // delete two at once, in a single tx
    test_env.jj_cmd_ok(&main_path, &["workspace", "forget", "second", "third"]);
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz 909d51b1 (empty) (no description set)
    "###);

    // the op log should have multiple workspaces forgotten in a single tx
    let stdout = test_env.jj_cmd_success(&main_path, &["op", "log", "--limit", "1"]);
    insta::assert_snapshot!(stdout, @r###"
    @  b7ab9f1c16cc test-username@host.example.com 2001-02-03 04:05:12.000 +07:00 - 2001-02-03 04:05:12.000 +07:00
    │  forget workspaces second, third
    │  args: jj workspace forget second third
    "###);

    // now, undo, and that should restore both workspaces
    test_env.jj_cmd_ok(&main_path, &["op", "undo"]);

    // finally, there should be three workspaces at the end
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: rlvkpnrz 909d51b1 (empty) (no description set)
    second: pmmvwywv 18463f43 (empty) (no description set)
    third: rzvqmyuk cc383fa2 (empty) (no description set)
    "###);
}

#[test]
fn test_workspaces_forget_abandon_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    std::fs::write(main_path.join("file"), "contents").unwrap();

    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../second"]);
    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../third"]);
    test_env.jj_cmd_ok(&main_path, &["workspace", "add", "../fourth"]);
    let third_path = test_env.env_root().join("third");
    test_env.jj_cmd_ok(&third_path, &["edit", "second@"]);
    let fourth_path = test_env.env_root().join("fourth");
    test_env.jj_cmd_ok(&fourth_path, &["edit", "second@"]);

    // there should be four workspaces, three of which are at the same empty commit
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: qpvuntsm 4e8f9d2b (no description set)
    fourth: uuqppmxq 57d63245 (empty) (no description set)
    second: uuqppmxq 57d63245 (empty) (no description set)
    third: uuqppmxq 57d63245 (empty) (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  57d63245a308 fourth@ second@ third@
    │ @  4e8f9d2be039 default@
    ├─╯
    ◆  000000000000
    "###);

    // delete the default workspace (should not abandon commit since not empty)
    test_env.jj_cmd_success(&main_path, &["workspace", "forget", "default"]);
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  57d63245a308 fourth@ second@ third@
    │ ○  4e8f9d2be039
    ├─╯
    ◆  000000000000
    "###);

    // delete the second workspace (should not abandon commit since other workspaces
    // still have commit checked out)
    test_env.jj_cmd_success(&main_path, &["workspace", "forget", "second"]);
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  57d63245a308 fourth@ third@
    │ ○  4e8f9d2be039
    ├─╯
    ◆  000000000000
    "###);

    // delete the last 2 workspaces (commit should be abandoned now even though
    // forgotten in same tx)
    test_env.jj_cmd_success(&main_path, &["workspace", "forget", "third", "fourth"]);
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    ○  4e8f9d2be039
    ◆  000000000000
    "###);
}

/// Test context of commit summary template
#[test]
fn test_list_workspaces_template() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    test_env.add_config(
        r#"
        templates.commit_summary = """commit_id.short() ++ " " ++ description.first_line() ++
                                      if(current_working_copy, " (current)")"""
        "#,
    );
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["commit", "-m", "initial"]);
    test_env.jj_cmd_ok(
        &main_path,
        &["workspace", "add", "--name", "second", "../secondary"],
    );

    // "current_working_copy" should point to the workspace we operate on
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: 8183d0fcaa4c  (current)
    second: 0a77a39d7d6f 
    "###);

    let stdout = test_env.jj_cmd_success(&secondary_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: 8183d0fcaa4c 
    second: 0a77a39d7d6f  (current)
    "###);
}

/// Test getting the workspace root from primary and secondary workspaces
#[test]
fn test_workspaces_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    $TEST_ENV/main
    "###);
    let main_subdir_path = main_path.join("subdir");
    std::fs::create_dir(&main_subdir_path).unwrap();
    let stdout = test_env.jj_cmd_success(&main_subdir_path, &["workspace", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    $TEST_ENV/main
    "###);

    test_env.jj_cmd_ok(
        &main_path,
        &["workspace", "add", "--name", "secondary", "../secondary"],
    );
    let stdout = test_env.jj_cmd_success(&secondary_path, &["workspace", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    $TEST_ENV/secondary
    "###);
    let secondary_subdir_path = secondary_path.join("subdir");
    std::fs::create_dir(&secondary_subdir_path).unwrap();
    let stdout = test_env.jj_cmd_success(&secondary_subdir_path, &["workspace", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    $TEST_ENV/secondary
    "###);
}

#[test]
fn test_debug_snapshot() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["debug", "snapshot"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  e1e762d39b39 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj debug snapshot
    ○  b51416386f26 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ○  9a7d829846af test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  initialize repo
    ○  000000000000 root()
    "###);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "initial"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  9ac6e7144e8a test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    │  describe commit 4e8f9d2be039994f589b4e57ac5e9488703e604d
    │  args: jj describe -m initial
    ○  e1e762d39b39 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj debug snapshot
    ○  b51416386f26 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ○  9a7d829846af test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  initialize repo
    ○  000000000000 root()
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"
    separate(" ",
      commit_id.short(),
      working_copies,
      if(divergent, "(divergent)"),
    )
    "#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template, "-r", "all()"])
}
