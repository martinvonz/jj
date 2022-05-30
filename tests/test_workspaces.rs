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

use itertools::Itertools;

use crate::common::TestEnvironment;

pub mod common;

/// Test adding a second workspace
#[test]
fn test_workspaces_add_second_workspace() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&main_path, &["close", "-m", "initial"]);

    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: 988d8c1dca7e (no description set)
    "###);

    let stdout = test_env.jj_cmd_success(
        &main_path,
        &["workspace", "add", "--name", "second", "../secondary"],
    );
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    Created workspace in "../secondary"
    Working copy now at: 8ac248e0c8d2 (no description set)
    Added 1 files, modified 0 files, removed 0 files
    "###);

    // Can see the checkout in each workspace in the log output. The "@" node in the
    // graph indicates the current workspace's checkout.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    o 8ac248e0c8d2d1865fe3679296e329c0137b1a31 second@
    | @ 988d8c1dca7e0944210ccc33584a6a42cd2962d4 default@
    |/  
    o 2062e7d6f1f46b4fe1453040d691931e77a88f7c 
    o 0000000000000000000000000000000000000000 
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &secondary_path), @r###"
    @ 8ac248e0c8d2d1865fe3679296e329c0137b1a31 second@
    | o 988d8c1dca7e0944210ccc33584a6a42cd2962d4 default@
    |/  
    o 2062e7d6f1f46b4fe1453040d691931e77a88f7c 
    o 0000000000000000000000000000000000000000 
    "###);

    // Both workspaces show up when we list them
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    default: 988d8c1dca7e (no description set)
    second: 8ac248e0c8d2 (no description set)
    "###);
}

/// Test making changes to the working copy in a workspace as it gets rewritten
/// from another workspace
#[test]
fn test_workspaces_conflicting_edits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");

    std::fs::write(main_path.join("file"), "contents\n").unwrap();
    test_env.jj_cmd_success(&main_path, &["close", "-m", "initial"]);

    test_env.jj_cmd_success(&main_path, &["workspace", "add", "../secondary"]);

    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    o 6bafff1a880f313aebb6d357c79b7aa4befa0af8 secondary@
    | @ c8f1217f93a0bc570a8bbfe055980f27062339ef default@
    |/  
    o 5af56dcc2cc27bb234e5574b5a3ebc5f22081462 
    o 0000000000000000000000000000000000000000 
    "###);

    // Make changes in both working copies
    std::fs::write(main_path.join("file"), "changed in main\n").unwrap();
    std::fs::write(secondary_path.join("file"), "changed in second\n").unwrap();
    // Squash the changes from the main workspace in the initial commit (before
    // running any command in the secondary workspace
    let stdout = test_env.jj_cmd_success(&main_path, &["squash"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: 6d004761e813 (no description set)
    "###);

    // The secondary workspace's checkout was updated
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    o 8d8269a323a01a287236c4fd5f64dc9737febb5b secondary@
    | @ 6d004761e81306cf8b2168a18868fbc84f182556 default@
    |/  
    o 52601f748bf6cb00ad5389922f530f20a7ecffaa 
    o 0000000000000000000000000000000000000000 
    "###);
    let stdout = get_log_output(&test_env, &secondary_path);
    // It was detected that the working copy is now stale
    // TODO: Since there was an uncommitted change in the working copy, it should
    // have been committed first (causing divergence)
    assert!(stdout.starts_with("The working copy is stale"));
    insta::assert_snapshot!(stdout.lines().skip(1).join("\n"), @r###"
    @ 8d8269a323a01a287236c4fd5f64dc9737febb5b secondary@
    | o 6d004761e81306cf8b2168a18868fbc84f182556 default@
    |/  
    o 52601f748bf6cb00ad5389922f530f20a7ecffaa 
    o 0000000000000000000000000000000000000000 
    "###);
}

/// Test forgetting workspaces
#[test]
fn test_workspaces_forget() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "main"]);
    let main_path = test_env.env_root().join("main");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_success(&main_path, &["close", "-m", "initial"]);

    test_env.jj_cmd_success(&main_path, &["workspace", "add", "../secondary"]);
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "forget"]);
    insta::assert_snapshot!(stdout, @"");

    // When listing workspaces, only the secondary workspace shows up
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    secondary: 39a6d6c6f295 (no description set)
    "###);

    // `jj status` tells us that there's no working copy here
    let stdout = test_env.jj_cmd_success(&main_path, &["st"]);
    insta::assert_snapshot!(stdout, @r###"
    No working copy
    "###);

    // The old checkout doesn't get an "@" in the log output
    // TODO: We should abandon the empty working copy commit
    // TODO: It seems useful to still have the "secondary@" marker here even though
    // there's only one workspace. We should show it when the command is not run
    // from that workspace.
    insta::assert_snapshot!(get_log_output(&test_env, &main_path), @r###"
    o 39a6d6c6f29557f886ded65d50063da4321ab2a8 
    | o 988d8c1dca7e0944210ccc33584a6a42cd2962d4 
    |/  
    o 2062e7d6f1f46b4fe1453040d691931e77a88f7c 
    o 0000000000000000000000000000000000000000 
    "###);

    // Revision "@" cannot be used
    let stderr = test_env.jj_cmd_failure(&main_path, &["log", "-r", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "@" doesn't exist
    "###);

    // Try to add back the workspace
    // TODO: We should make this just add it back instead of failing
    let stderr = test_env.jj_cmd_failure(&main_path, &["workspace", "add", "."]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Workspace already exists
    "###);

    // Forget the secondary workspace
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "forget", "secondary"]);
    insta::assert_snapshot!(stdout, @"");
    // No workspaces left
    let stdout = test_env.jj_cmd_success(&main_path, &["workspace", "list"]);
    insta::assert_snapshot!(stdout, @"");
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    test_env.jj_cmd_success(cwd, &["log", "-T", r#"commit_id " " checkouts"#])
}
