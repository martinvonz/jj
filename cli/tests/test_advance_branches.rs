// Copyright 2024 The Jujutsu Authors
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

use test_case::test_case;

use crate::common::TestEnvironment;

fn get_log_output_with_branches(test_env: &TestEnvironment, cwd: &Path) -> String {
    // Don't include commit IDs since they will be different depending on
    // whether the test runs with `jj commit` or `jj describe` + `jj new`.
    let template = r#""branches{" ++ local_branches ++ "} desc: " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn set_advance_branches(test_env: &TestEnvironment, enabled: bool) {
    if enabled {
        test_env.add_config(
            r#"[experimental-advance-branches]
        enabled-branches = ["glob:*"]
        "#,
        );
    } else {
        test_env.add_config(
            r#"[experimental-advance-branches]
        enabled-branches = []
        "#,
        );
    }
}

// Runs a command in the specified test environment and workspace path that
// describes the current commit with `commit_message` and creates a new commit
// on top of it.
type CommitFn = fn(env: &TestEnvironment, workspace_path: &Path, commit_message: &str);

// Implements CommitFn using the `jj commit` command.
fn commit_cmd(env: &TestEnvironment, workspace_path: &Path, commit_message: &str) {
    env.jj_cmd_ok(workspace_path, &["commit", "-m", commit_message]);
}

// Implements CommitFn using the `jj describe` and `jj new`.
fn describe_new_cmd(env: &TestEnvironment, workspace_path: &Path, commit_message: &str) {
    env.jj_cmd_ok(workspace_path, &["describe", "-m", commit_message]);
    env.jj_cmd_ok(workspace_path, &["new"]);
}

// Check that enabling and disabling advance-branches works as expected.
#[test_case(commit_cmd ; "commit")]
#[test_case(describe_new_cmd; "new")]
fn test_advance_branches_enabled(make_commit: CommitFn) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // First, test with advance-branches enabled. Start by creating a branch on the
    // root commit.
    set_advance_branches(&test_env, true);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "test_branch"],
    );

    // Check the initial state of the repo.
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◆  branches{test_branch} desc:
    "###);
    }

    // Run jj commit, which will advance the branch pointing to @-.
    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);
    }

    // Now disable advance branches and commit again. The branch shouldn't move.
    set_advance_branches(&test_env, false);
    make_commit(&test_env, &workspace_path, "second");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{} desc: second
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);
    }
}

// Check that only a branch pointing to @- advances. Branches pointing to @ are
// not advanced.
#[test_case(commit_cmd ; "commit")]
#[test_case(describe_new_cmd; "new")]
fn test_advance_branches_at_minus(make_commit: CommitFn) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, true);
    test_env.jj_cmd_ok(&workspace_path, &["branch", "create", "test_branch"]);

    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{test_branch} desc:
    ◆  branches{} desc:
    "###);
    }

    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);
    }

    // Create a second branch pointing to @. On the next commit, only the first
    // branch, which points to @-, will advance.
    test_env.jj_cmd_ok(&workspace_path, &["branch", "create", "test_branch2"]);
    make_commit(&test_env, &workspace_path, "second");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{test_branch test_branch2} desc: second
    ○  branches{} desc: first
    ◆  branches{} desc:
    "###);
    }
}

// Test that per-branch overrides invert the behavior of
// experimental-advance-branches.enabled.
#[test_case(commit_cmd ; "commit")]
#[test_case(describe_new_cmd; "new")]
fn test_advance_branches_overrides(make_commit: CommitFn) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // advance-branches is disabled by default.
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "test_branch"],
    );

    // Check the initial state of the repo.
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◆  branches{test_branch} desc:
    "###);
    }

    // Commit will not advance the branch since advance-branches is disabled.
    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{} desc: first
    ◆  branches{test_branch} desc:
    "###);
    }

    // Now enable advance branches for "test_branch", move the branch, and commit
    // again.
    test_env.add_config(
        r#"[experimental-advance-branches]
    enabled-branches = ["test_branch"]
    "#,
    );
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "set", "test_branch", "-r", "@-"],
    );
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);
    }
    make_commit(&test_env, &workspace_path, "second");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{test_branch} desc: second
    ○  branches{} desc: first
    ◆  branches{} desc:
    "###);
    }

    // Now disable advance branches for "test_branch" and "second_branch", which
    // we will use later. Disabling always takes precedence over enabling.
    test_env.add_config(
        r#"[experimental-advance-branches]
    enabled-branches = ["test_branch", "second_branch"]
    disabled-branches = ["test_branch"]
    "#,
    );
    make_commit(&test_env, &workspace_path, "third");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{} desc: third
    ○  branches{test_branch} desc: second
    ○  branches{} desc: first
    ◆  branches{} desc:
    "###);
    }

    // If we create a new branch at @- and move test_branch there as well. When
    // we commit, only "second_branch" will advance since "test_branch" is disabled.
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "second_branch", "-r", "@-"],
    );
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "set", "test_branch", "-r", "@-"],
    );
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{second_branch test_branch} desc: third
    ○  branches{} desc: second
    ○  branches{} desc: first
    ◆  branches{} desc:
    "###);
    }
    make_commit(&test_env, &workspace_path, "fourth");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{second_branch} desc: fourth
    ○  branches{test_branch} desc: third
    ○  branches{} desc: second
    ○  branches{} desc: first
    ◆  branches{} desc:
    "###);
    }
}

// If multiple eligible branches point to @-, all of them will be advanced.
#[test_case(commit_cmd ; "commit")]
#[test_case(describe_new_cmd; "new")]
fn test_advance_branches_multiple_branches(make_commit: CommitFn) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, true);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "first_branch"],
    );
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "second_branch"],
    );

    insta::allow_duplicates! {
    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◆  branches{first_branch second_branch} desc:
    "###);
    }

    // Both branches are eligible and both will advance.
    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{first_branch second_branch} desc: first
    ◆  branches{} desc:
    "###);
    }
}

// Call `jj new` on an interior commit and see that the branch pointing to its
// parent's parent is advanced.
#[test]
fn test_new_advance_branches_interior() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, true);

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◆  branches{} desc:
    "###);

    // Create a gap in the commits for us to insert our new commit with --before.
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@---", "test_branch"],
    );
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{} desc: third
    ○  branches{} desc: second
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["new", "-r", "@--"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    │ ○  branches{} desc: third
    ├─╯
    ○  branches{test_branch} desc: second
    ○  branches{} desc: first
    ◆  branches{} desc:
    "###);
}

// If the `--before` flag is passed to `jj new`, branches are not advanced.
#[test]
fn test_new_advance_branches_before() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, true);

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◆  branches{} desc:
    "###);

    // Create a gap in the commits for us to insert our new commit with --before.
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@---", "test_branch"],
    );
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{} desc: third
    ○  branches{} desc: second
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["new", "--before", "@-"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    ○  branches{} desc: third
    @  branches{} desc:
    ○  branches{} desc: second
    ○  branches{test_branch} desc: first
    ◆  branches{} desc:
    "###);
}

// If the `--after` flag is passed to `jj new`, branches are not advanced.
#[test]
fn test_new_advance_branches_after() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, true);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "test_branch"],
    );

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◆  branches{test_branch} desc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "--after", "@"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ○  branches{} desc: first
    ◆  branches{test_branch} desc:
    "###);
}

#[test]
fn test_new_advance_branches_merge_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, true);
    test_env.jj_cmd_ok(&workspace_path, &["desc", "-m", "0"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m", "1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(0)", "-m", "2"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "test_branch", "-r", "description(0)"],
    );

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc: 2
    │ ○  branches{} desc: 1
    ├─╯
    ○  branches{test_branch} desc: 0
    ◆  branches{} desc:
    "###);

    // The branch won't advance because `jj  new` had multiple targets.
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(1)", "description(2)"],
    );
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @    branches{} desc:
    ├─╮
    │ ○  branches{} desc: 2
    ○ │  branches{} desc: 1
    ├─╯
    ○  branches{test_branch} desc: 0
    ◆  branches{} desc:
    "###);
}
