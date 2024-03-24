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

use std::fs;

use itertools::Itertools as _;

use crate::common::TestEnvironment;

#[test]
fn test_alias_basic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"aliases.b = ["log", "-r", "@", "-T", "branches"]"#);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "my-branch"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["b"]);
    insta::assert_snapshot!(stdout, @r###"
    @  my-branch
    │
    ~
    "###);
}

#[test]
fn test_alias_legacy_section() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Can define aliases in [alias] section
    test_env.add_config(r#"alias.b = ["log", "-r", "@", "-T", "branches"]"#);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "my-branch"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["b"]);
    insta::assert_snapshot!(stdout, @r###"
    @  my-branch
    │
    ~
    "###);

    // The same alias (name) in both [alias] and [aliases] sections is an error
    test_env.add_config(r#"aliases.b = ["branch", "list"]"#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias "b" is defined in both [aliases] and [alias]
    Hint: [aliases] is the preferred section for aliases. Please remove the alias from [alias].
    "###);
}

#[test]
fn test_alias_bad_name() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["foo."]);
    insta::assert_snapshot!(stderr, @r###"
    error: unrecognized subcommand 'foo.'

    Usage: jj [OPTIONS] <COMMAND>

    For more information, try '--help'.
    "###);
}

#[test]
fn test_alias_calls_empty_command() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r#"
    aliases.empty = []
    aliases.empty_command_with_opts = ["--no-pager"]
    "#,
    );
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["empty"]);
    insta::assert_snapshot!(stderr.lines().take(3).join("\n"), @r###"
    Jujutsu (An experimental VCS)

    Usage: jj [OPTIONS] <COMMAND>
    "###);
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["empty", "--no-pager"]);
    insta::assert_snapshot!(stderr.lines().next().unwrap_or_default(), @r###"
    error: 'jj' requires a subcommand but one was not provided
    "###);
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["empty_command_with_opts"]);
    insta::assert_snapshot!(stderr.lines().next().unwrap_or_default(), @r###"
    error: 'jj' requires a subcommand but one was not provided
    "###);
}

#[test]
fn test_alias_calls_unknown_command() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"aliases.foo = ["nonexistent"]"#);
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["foo"]);
    insta::assert_snapshot!(stderr, @r###"
    error: unrecognized subcommand 'nonexistent'

      tip: a similar subcommand exists: 'next'

    Usage: jj [OPTIONS] <COMMAND>

    For more information, try '--help'.
    "###);
}

#[test]
fn test_alias_calls_command_with_invalid_option() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"aliases.foo = ["log", "--nonexistent"]"#);
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["foo"]);
    insta::assert_snapshot!(stderr, @r###"
    error: unexpected argument '--nonexistent' found

      tip: to pass '--nonexistent' as a value, use '-- --nonexistent'

    Usage: jj log [OPTIONS] [PATHS]...

    For more information, try '--help'.
    "###);
}

#[test]
fn test_alias_calls_help() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(r#"aliases.h = ["--help"]"#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["h"]);
    insta::assert_snapshot!(stdout.lines().take(5).join("\n"), @r###"
    Jujutsu (An experimental VCS)

    To get started, see the tutorial at https://github.com/martinvonz/jj/blob/main/docs/tutorial.md.

    Usage: jj [OPTIONS] <COMMAND>
    "###);
}

#[test]
fn test_alias_cannot_override_builtin() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"aliases.log = ["rebase"]"#);
    // Alias should be ignored
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "root()"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  zzzzzzzz root() 00000000
    "###);
}

#[test]
fn test_alias_recursive() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r#"[aliases]
    foo = ["foo"]
    bar = ["baz"]
    baz = ["bar"]
    "#,
    );
    // Alias should not cause infinite recursion or hang
    let stderr = test_env.jj_cmd_failure(&repo_path, &["foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Recursive alias definition involving "foo"
    "###);
    // Also test with mutual recursion
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bar"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Recursive alias definition involving "bar"
    "###);
}

#[test]
fn test_alias_global_args_before_and_after() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(r#"aliases.l = ["log", "-T", "commit_id", "-r", "all()"]"#);
    // Test the setup
    let stdout = test_env.jj_cmd_success(&repo_path, &["l"]);
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);

    // Can pass global args before
    let stdout = test_env.jj_cmd_success(&repo_path, &["l", "--at-op", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  0000000000000000000000000000000000000000
    "###);
    // Can pass global args after
    let stdout = test_env.jj_cmd_success(&repo_path, &["--at-op", "@-", "l"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  0000000000000000000000000000000000000000
    "###);
    // Test passing global args both before and after
    let stdout = test_env.jj_cmd_success(&repo_path, &["--at-op", "abc123", "l", "--at-op", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  0000000000000000000000000000000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["-R", "../nonexistent", "l", "-R", "."]);
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_alias_global_args_in_definition() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(
        r#"aliases.l = ["log", "-T", "commit_id", "--at-op", "@-", "-r", "all()", "--color=always"]"#,
    );

    // The global argument in the alias is respected
    let stdout = test_env.jj_cmd_success(&repo_path, &["l"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  [38;5;4m0000000000000000000000000000000000000000[39m
    "###);
}

#[test]
fn test_alias_invalid_definition() {
    let test_env = TestEnvironment::default();

    test_env.add_config(
        r#"[aliases]
    non-list = 5
    non-string-list = [[]]
    "#,
    );
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["non-list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias definition for "non-list" must be a string list
    "###);
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["non-string-list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias definition for "non-string-list" must be a string list
    "###);
}

#[test]
fn test_alias_in_repo_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo1", "--git"]);
    let repo1_path = test_env.env_root().join("repo1");
    fs::create_dir(repo1_path.join("sub")).unwrap();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo2", "--git"]);
    let repo2_path = test_env.env_root().join("repo2");
    fs::create_dir(repo2_path.join("sub")).unwrap();

    test_env.add_config(r#"aliases.l = ['log', '-r@', '--no-graph', '-T"user alias\n"']"#);
    fs::write(
        repo1_path.join(".jj/repo/config.toml"),
        r#"aliases.l = ['log', '-r@', '--no-graph', '-T"repo1 alias\n"']"#,
    )
    .unwrap();

    // In repo1 sub directory, aliases can be loaded from the repo1 config.
    let stdout = test_env.jj_cmd_success(&repo1_path.join("sub"), &["l"]);
    insta::assert_snapshot!(stdout, @r###"
    repo1 alias
    "###);

    // In repo2 directory, no repo-local aliases exist.
    let stdout = test_env.jj_cmd_success(&repo2_path, &["l"]);
    insta::assert_snapshot!(stdout, @r###"
    user alias
    "###);

    // Aliases can't be loaded from the -R path due to chicken and egg problem.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo2_path, &["l", "-R", repo1_path.to_str().unwrap()]);
    insta::assert_snapshot!(stdout, @r###"
    user alias
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Command aliases cannot be loaded from -R/--repository path
    "###);

    // Aliases are loaded from the cwd-relative workspace even with -R.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo1_path, &["l", "-R", repo2_path.to_str().unwrap()]);
    insta::assert_snapshot!(stdout, @r###"
    repo1 alias
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Command aliases cannot be loaded from -R/--repository path
    "###);

    // No warning if the expanded command is identical.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo1_path, &["files", "-R", repo2_path.to_str().unwrap()]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");

    // Config loaded from the cwd-relative workspace shouldn't persist. It's
    // used only for command arguments expansion.
    let stdout = test_env.jj_cmd_success(
        &repo1_path,
        &[
            "config",
            "list",
            "aliases",
            "-R",
            repo2_path.to_str().unwrap(),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    aliases.l=["log", "-r@", "--no-graph", "-T\"user alias\\n\""]
    "###);
}
