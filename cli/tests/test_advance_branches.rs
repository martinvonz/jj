use std::path::Path;

use itertools::Itertools;

use crate::common::TestEnvironment;

fn get_log_output_with_branches(test_env: &TestEnvironment, cwd: &Path) -> String {
    // Don't include commit IDs since they will be different depending on
    // whether the test runs with `jj commit` or `jj describe` + `jj new`.
    let template = r#""branches{" ++ local_branches ++ "} desc: " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn set_advance_branches(test_env: &TestEnvironment, cwd: &Path, value: bool) -> String {
    test_env.jj_cmd_success(
        cwd,
        &[
            "config",
            "set",
            "--repo",
            "experimental-advance-branches.enabled",
            &format!("{}", value),
        ],
    )
}

fn set_advance_branches_overrides(
    test_env: &TestEnvironment,
    cwd: &Path,
    overrides: &[&str],
) -> String {
    let override_string: String = overrides.iter().map(|x| format!("\"{}\"", x)).join(",");
    test_env.jj_cmd_success(
        cwd,
        &[
            "config",
            "set",
            "--repo",
            "experimental-advance-branches.overrides",
            &format!("[{}]", override_string),
        ],
    )
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

macro_rules! parameterized_tests{
    ($($test_name:ident: $case_fn:ident($commit_fn:ident),)*) => {
    $(
        #[test]
        fn $test_name() {
            $case_fn($commit_fn);
        }
    )*
    }
}

parameterized_tests! {
    test_commit_advance_branches_enabled: case_advance_branches_enabled(commit_cmd),
    test_commit_advance_branches_at_minus: case_advance_branches_at_minus(commit_cmd),
    test_commit_advance_branches_overrides: case_advance_branches_overrides(commit_cmd),
    test_commit_advance_branches_multiple_branches:
        case_advance_branches_multiple_branches(commit_cmd),
    test_new_advance_branches_enabled: case_advance_branches_enabled(describe_new_cmd),
    test_new_advance_branches_at_minus: case_advance_branches_at_minus(describe_new_cmd),
    test_new_advance_branches_overrides: case_advance_branches_overrides(describe_new_cmd),
    test_new_advance_branches_multiple_branches:
        case_advance_branches_multiple_branches(describe_new_cmd),
}

// Check that enabling and disabling advance-branches works as expected.
fn case_advance_branches_enabled(make_commit: CommitFn) {
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
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch} desc:
    "###);
    }

    // Run jj commit, which will advance the branch pointing to @-.
    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);
    }

    // Now disable advance branches and commit again. The branch shouldn't move.
    set_advance_branches(&test_env, &workspace_path, false);
    make_commit(&test_env, &workspace_path, "second");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{} desc: second
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);
    }
}

// Check that only a branch pointing to @- advances. Branches pointing to @ are
// not advanced.
fn case_advance_branches_at_minus(make_commit: CommitFn) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    set_advance_branches(&test_env, &workspace_path, true);
    test_env.jj_cmd_ok(&workspace_path, &["branch", "create", "test_branch"]);

    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{test_branch} desc:
    ◉  branches{} desc:
    "###);
    }

    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);
    }

    // Create a second branch pointing to @. On the next commit, only the first
    // branch, which points to @-, will advance.
    test_env.jj_cmd_ok(&workspace_path, &["branch", "create", "test_branch2"]);
    make_commit(&test_env, &workspace_path, "second");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch test_branch2} desc: second
    ◉  branches{} desc: first
    ◉  branches{} desc:
    "###);
    }
}

// Test that per-branch overrides invert the behavior of
// experimental-advance-branches.enabled.
fn case_advance_branches_overrides(make_commit: CommitFn) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // Disable advance branches.
    set_advance_branches(&test_env, &workspace_path, false);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "create", "-r", "@-", "test_branch"],
    );

    // Check the initial state of the repo.
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch} desc:
    "###);
    }

    // Commit will not advance the branch since advance-branches is disabled.
    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{} desc: first
    ◉  branches{test_branch} desc:
    "###);
    }

    // Now add an override, move the branch, and commit again.
    set_advance_branches_overrides(&test_env, &workspace_path, &["test_branch"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["branch", "set", "test_branch", "-r", "@-"],
    );
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);
    }
    make_commit(&test_env, &workspace_path, "second");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{test_branch} desc: second
    ◉  branches{} desc: first
    ◉  branches{} desc:
    "###);
    }

    // Now enable advance-branches, which will cause the override to disable it
    // for test_branch. The branch will not move.
    set_advance_branches(&test_env, &workspace_path, true);
    make_commit(&test_env, &workspace_path, "third");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{} desc: third
    ◉  branches{test_branch} desc: second
    ◉  branches{} desc: first
    ◉  branches{} desc:
    "###);
    }

    // If we create a new branch at @- and move test_branch there as well. When
    // we commit, the new branch will advance. There won't be ambiguity about
    // which branch to advance because there is an override for test_branch.
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
    ◉  branches{second_branch test_branch} desc: third
    ◉  branches{} desc: second
    ◉  branches{} desc: first
    ◉  branches{} desc:
    "###);
    }
    make_commit(&test_env, &workspace_path, "fourth");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{second_branch} desc: fourth
    ◉  branches{test_branch} desc: third
    ◉  branches{} desc: second
    ◉  branches{} desc: first
    ◉  branches{} desc:
    "###);
    }
}

// If multiple eligible branches point to @-, all of them will be advanced.
fn case_advance_branches_multiple_branches(make_commit: CommitFn) {
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

    insta::allow_duplicates! {
    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{first_branch second_branch} desc:
    "###);
    }

    // Both branches are eligible and both will advance.
    make_commit(&test_env, &workspace_path, "first");
    insta::allow_duplicates! {
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{first_branch second_branch} desc: first
    ◉  branches{} desc:
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

    // First, test with advance-branches enabled. Start by creating a branch on the
    // root commit.
    set_advance_branches(&test_env, &workspace_path, true);

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{} desc:
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
    ◉  branches{} desc: third
    ◉  branches{} desc: second
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["new", "-r", "@--"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    │ ◉  branches{} desc: third
    ├─╯
    ◉  branches{test_branch} desc: second
    ◉  branches{} desc: first
    ◉  branches{} desc:
    "###);
}

// If the `--before` flag is passed to `jj new`, branches are not advanced.
#[test]
fn test_new_advance_branches_before() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // First, test with advance-branches enabled. Start by creating a branch on the
    // root commit.
    set_advance_branches(&test_env, &workspace_path, true);

    // Check the initial state of the repo.
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{} desc:
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
    ◉  branches{} desc: third
    ◉  branches{} desc: second
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["new", "--before", "-r", "@-"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    ◉  branches{} desc: third
    @  branches{} desc:
    ◉  branches{} desc: second
    ◉  branches{test_branch} desc: first
    ◉  branches{} desc:
    "###);
}

// If the `--after` flag is passed to `jj new`, branches are not advanced.
#[test]
fn test_new_advance_branches_after() {
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
    @  branches{} desc:
    ◉  branches{test_branch} desc:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "--after"]);
    insta::assert_snapshot!(get_log_output_with_branches(&test_env, &workspace_path), @r###"
    @  branches{} desc:
    ◉  branches{} desc: first
    ◉  branches{test_branch} desc:
    "###);
}
