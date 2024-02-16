use std::path::Path;

use itertools::Itertools;

use crate::common::TestEnvironment;

fn get_log_output_with_branches(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id.short() ++ " br:{" ++ local_branches ++ "} dsc: " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn enable_advance_branches_for_patterns(test_env: &TestEnvironment, cwd: &Path, patterns: &[&str]) {
    #[rustfmt::skip]
    let pattern_string: String = patterns.iter().map(|x| format!("\"{}\"", x)).join(",");
    test_env.jj_cmd_success(
        cwd,
        &[
            "config",
            "set",
            "--repo",
            "experimental-advance-branches.enabled-branches",
            &format!("[{}]", pattern_string),
        ],
    );
}

fn set_advance_branches(test_env: &TestEnvironment, cwd: &Path, value: bool) {
    if value {
        enable_advance_branches_for_patterns(test_env, cwd, &["glob:*"]);
    } else {
        enable_advance_branches_for_patterns(test_env, cwd, &[""]);
    }
}

// Check that enabling and disabling advance-branches works as expected.
#[test]
fn test_advance_branches_enabled() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // First, test with advance-branches enabled. Start by creating a branch on the
    // root commit.
    set_advance_branches(&test_env, &workspace_path, true);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "test_branch"],
    );

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  230dd059e1b0 br:{} dsc:
    ◉  000000000000 br:{test_branch} dsc:
    "###);

    // Run jj commit, which will advance the branch pointing to @-.
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  24bb7f9da598 br:{} dsc:
    ◉  95f2456c4bbd br:{test_branch} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);

    // Now disable advance branches and commit again. The branch shouldn't move.
    set_advance_branches(&test_env, &workspace_path, false);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=second"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  b29edd893970 br:{} dsc:
    ◉  ebf7d96fb6ad br:{} dsc: second
    ◉  95f2456c4bbd br:{test_branch} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);
}

// Check that only a branch pointing to @- advances. Branches pointing to @ are
// not advanced.
#[test]
fn test_advance_branches_at_minus() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, &workspace_path, true);
    test_env.jj_cmd_ok(&workspace_path, &["branch", "create", "test_branch"]);

    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  230dd059e1b0 br:{test_branch} dsc:
    ◉  000000000000 br:{} dsc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  24bb7f9da598 br:{} dsc:
    ◉  95f2456c4bbd br:{test_branch} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);

    // Create a second branch pointing to @. On the next commit, only the first
    // branch, which points to @-, will advance.
    test_env.jj_cmd_ok(&workspace_path, &["branch", "create", "test_branch2"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=second"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  b29edd893970 br:{} dsc:
    ◉  ebf7d96fb6ad br:{test_branch test_branch2} dsc: second
    ◉  95f2456c4bbd br:{} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);
}

// Test that per-branch overrides invert the behavior of
// experimental-advance-branches.enabled.
#[test]
fn test_advance_branches_overrides() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // advance-branches is disabled by default.
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "test_branch"],
    );

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  230dd059e1b0 br:{} dsc:
    ◉  000000000000 br:{test_branch} dsc:
    "###);

    // Commit will not advance the branch since advance-branches is disabled.
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  7e3a6f5e0f15 br:{} dsc:
    ◉  307e33f70413 br:{} dsc: first
    ◉  000000000000 br:{test_branch} dsc:
    "###);

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
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  7e3a6f5e0f15 br:{} dsc:
    ◉  307e33f70413 br:{test_branch} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=second"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  8c1bd3e7de60 br:{} dsc:
    ◉  468d1ab20fb3 br:{test_branch} dsc: second
    ◉  307e33f70413 br:{} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);

    // Now disable advance branches for "test_branch" and "second_branch", which
    // we will use later. Disabling always takes precedence over enabling.
    test_env.add_config(
        r#"[experimental-advance-branches]
    enabled-branches = ["test_branch", "second_branch"]
    disabled-branches = ["test_branch"]
    "#,
    );
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=third"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  5888a83948dd br:{} dsc:
    ◉  50e9c28e6d85 br:{} dsc: third
    ◉  468d1ab20fb3 br:{test_branch} dsc: second
    ◉  307e33f70413 br:{} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);

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
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  5888a83948dd br:{} dsc:
    ◉  50e9c28e6d85 br:{second_branch test_branch} dsc: third
    ◉  468d1ab20fb3 br:{} dsc: second
    ◉  307e33f70413 br:{} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=fourth"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  666d42aedca7 br:{} dsc:
    ◉  f23aa63eeb99 br:{second_branch} dsc: fourth
    ◉  50e9c28e6d85 br:{test_branch} dsc: third
    ◉  468d1ab20fb3 br:{} dsc: second
    ◉  307e33f70413 br:{} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);
}

// If multiple eligible branches point to @-, all of them will be advanced.
#[test]
fn test_advance_branches_multiple_branches() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, &workspace_path, true);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "first_branch"],
    );
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "second_branch"],
    );
    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  230dd059e1b0 br:{} dsc:
    ◉  000000000000 br:{first_branch second_branch} dsc:
    "###);

    // Both branches are eligible and both will advance.
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  f307e5d9f90b br:{} dsc:
    ◉  0fca5c9228e6 br:{first_branch second_branch} dsc: first
    ◉  000000000000 br:{} dsc:
    "###);
}
