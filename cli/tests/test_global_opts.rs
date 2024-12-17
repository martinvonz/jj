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

use std::ffi::OsString;

use indoc::indoc;

use crate::common::get_stderr_string;
use crate::common::strip_last_line;
use crate::common::TestEnvironment;

#[test]
fn test_non_utf8_arg() {
    let test_env = TestEnvironment::default();
    #[cfg(unix)]
    let invalid_utf = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![0x66, 0x6f, 0x80, 0x6f])
    };
    #[cfg(windows)]
    let invalid_utf = {
        use std::os::windows::prelude::*;
        OsString::from_wide(&[0x0066, 0x006f, 0xD800, 0x006f])
    };
    let assert = test_env
        .jj_cmd(test_env.env_root(), &[])
        .args(&[invalid_utf])
        .assert()
        .code(2);
    let stderr = get_stderr_string(&assert);
    insta::assert_snapshot!(stderr, @r###"
    Error: Non-utf8 argument
    "###);
}

#[test]
fn test_version() {
    let test_env = TestEnvironment::default();

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["--version"]);
    let sanitized = stdout.replace(|c: char| c.is_ascii_hexdigit(), "?");
    let expected = [
        "jj ?.??.?\n",
        "jj ?.??.?-????????????????????????????????????????\n",
    ];
    assert!(
        expected.contains(&sanitized.as_str()),
        "`jj version` output: {stdout:?}.\nSanitized: {sanitized:?}\nExpected one of: {expected:?}"
    );
}

#[test]
fn test_no_subcommand() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Outside of a repo.
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &[]);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Use `jj -h` for a list of available commands.
    Run `jj config set --user ui.default-command log` to disable this message.
    Error: There is no jj repo in "."
    "###);

    test_env.add_config(r#"ui.default-command="log""#);
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &[]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    "###);

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["--help"]);
    insta::assert_snapshot!(stdout.lines().next().unwrap(), @"Jujutsu (An experimental VCS)");

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["-R", "repo"]);
    assert_eq!(stdout, test_env.jj_cmd_success(&repo_path, &["log"]));

    // Inside of a repo.
    let stdout = test_env.jj_cmd_success(&repo_path, &[]);
    assert_eq!(stdout, test_env.jj_cmd_success(&repo_path, &["log"]));

    // Command argument that looks like a command name.
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "help"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "log"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "show"]);
    // TODO: test_env.jj_cmd_ok(&repo_path, &["-r", "help"])
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["-r", "log"]), @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 08:05:07 help log show 230dd059
    │  (empty) (no description set)
    ~
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["-r", "show"]), @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 08:05:07 help log show 230dd059
    │  (empty) (no description set)
    ~
    "###);

    // Multiple default command strings work.
    test_env.add_config(r#"ui.default-command=["commit", "-m", "foo"]"#);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file.txt"), "file").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &[]);
    assert_eq!(stdout, "");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kxryzmor 89c70edf (empty) (no description set)
    Parent commit      : lylxulpl 51bd3589 foo
    "###);
}

#[test]
fn test_ignore_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);

    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "initial").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @  b15ef4cdd277d2c63cce6d67c1916f53a36141f7
    ◆  0000000000000000000000000000000000000000
    "###);

    // Modify the file. With --ignore-working-copy, we still get the same commit
    // ID.
    std::fs::write(repo_path.join("file"), "modified").unwrap();
    let stdout_again = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "commit_id", "--ignore-working-copy"],
    );
    assert_eq!(stdout_again, stdout);

    // But without --ignore-working-copy, we get a new commit ID.
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @  4d2c49a8f8e2f1ba61f48ba79e5f4a5faa6512cf
    ◆  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_repo_arg_with_init() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["init", "-R=.", "repo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    "###);
}

#[test]
fn test_repo_arg_with_git_clone() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["git", "clone", "-R=.", "remote"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    "###);
}

#[test]
fn test_resolve_workspace_directory() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let subdir = repo_path.join("dir").join("subdir");
    std::fs::create_dir_all(&subdir).unwrap();

    // Ancestor of cwd
    let stdout = test_env.jj_cmd_success(&subdir, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    The working copy is clean
    Working copy : qpvuntsm 230dd059 (empty) (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Explicit subdirectory path
    let stderr = test_env.jj_cmd_failure(&subdir, &["status", "-R", "."]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    "###);

    // Valid explicit path
    let stdout = test_env.jj_cmd_success(&subdir, &["status", "-R", "../.."]);
    insta::assert_snapshot!(stdout, @r###"
    The working copy is clean
    Working copy : qpvuntsm 230dd059 (empty) (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // "../../..".ancestors() contains "../..", but it should never be looked up.
    let stderr = test_env.jj_cmd_failure(&subdir, &["status", "-R", "../../.."]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "../../.."
    "###);
}

#[test]
fn test_no_workspace_directory() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    std::fs::create_dir(&repo_path).unwrap();

    let stderr = test_env.jj_cmd_failure(&repo_path, &["status"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    "###);

    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["status", "-R", "repo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "repo"
    "###);

    std::fs::create_dir(repo_path.join(".git")).unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["status"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    Hint: It looks like this is a git repo. You can create a jj repo backed by it by running this:
    jj git init --colocate
    "###);
}

#[test]
fn test_bad_path() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let subdir = repo_path.join("dir");
    std::fs::create_dir_all(&subdir).unwrap();

    // cwd == workspace_root
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "show", "../out"]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Error: Failed to parse fileset: Invalid file pattern
    Caused by:
    1:  --> 1:1
      |
    1 | ../out
      | ^----^
      |
      = Invalid file pattern
    2: Path "../out" is not in the repo "."
    3: Invalid component ".." in repo-relative path "../out"
    "###);

    // cwd != workspace_root, can't be parsed as repo-relative path
    let stderr = test_env.jj_cmd_failure(&subdir, &["file", "show", "../.."]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Error: Failed to parse fileset: Invalid file pattern
    Caused by:
    1:  --> 1:1
      |
    1 | ../..
      | ^---^
      |
      = Invalid file pattern
    2: Path "../.." is not in the repo "../"
    3: Invalid component ".." in repo-relative path "../"
    "###);

    // cwd != workspace_root, can be parsed as repo-relative path
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["file", "show", "-Rrepo", "out"]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Error: Failed to parse fileset: Invalid file pattern
    Caused by:
    1:  --> 1:1
      |
    1 | out
      | ^-^
      |
      = Invalid file pattern
    2: Path "out" is not in the repo "repo"
    3: Invalid component ".." in repo-relative path "../out"
    Hint: Consider using root:"out" to specify repo-relative path
    "###);

    test_env.add_config("ui.allow-filesets = false");

    // If fileset/pattern syntax is disabled, no hint should be generated
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["file", "show", "-Rrepo", "out"]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Error: Path "out" is not in the repo "repo"
    Caused by: Invalid component ".." in repo-relative path "../out"
    "###);
}

#[test]
fn test_invalid_filesets_looking_like_filepaths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "show", "abc~"]);
    insta::assert_snapshot!(stderr, @r#"
    Error: Failed to parse fileset: Syntax error
    Caused by:  --> 1:5
      |
    1 | abc~
      |     ^---
      |
      = expected `~` or <primary>
    Hint: See https://jj-vcs.github.io/jj/latest/filesets/ for filesets syntax, or for how to match file paths.
    "#);

    test_env.add_config(r#"ui.allow-filesets=false"#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "show", "abc~"]);
    insta::assert_snapshot!(stderr, @r#"
    Error: No such path: abc~
    "#);
}

#[test]
fn test_broken_repo_structure() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let store_path = repo_path.join(".jj").join("repo").join("store");
    let store_type_path = store_path.join("type");

    // Test the error message when the git repository can't be located.
    std::fs::remove_file(store_path.join("git_target")).unwrap();
    let stderr = test_env.jj_cmd_internal_error(&repo_path, &["log"]);
    insta::assert_snapshot!(strip_last_line(&stderr), @r###"
    Internal error: The repository appears broken or inaccessible
    Caused by:
    1: Cannot access $TEST_ENV/repo/.jj/repo/store/git_target
    "###);

    // Test the error message when the commit backend is of unknown type.
    std::fs::write(&store_type_path, "unknown").unwrap();
    let stderr = test_env.jj_cmd_internal_error(&repo_path, &["log"]);
    insta::assert_snapshot!(stderr, @r###"
    Internal error: This version of the jj binary doesn't support this type of repo
    Caused by: Unsupported commit backend type 'unknown'
    "###);

    // Test the error message when the file indicating the commit backend type
    // cannot be read.
    std::fs::remove_file(&store_type_path).unwrap();
    std::fs::create_dir(&store_type_path).unwrap();
    let stderr = test_env.jj_cmd_internal_error(&repo_path, &["log"]);
    insta::assert_snapshot!(strip_last_line(&stderr), @r###"
    Internal error: The repository appears broken or inaccessible
    Caused by:
    1: Failed to read commit backend type
    2: Cannot access $TEST_ENV/repo/.jj/repo/store/type
    "###);

    // Test when the .jj directory is empty. The error message is identical to
    // the previous one, but writing the default type file would also fail.
    std::fs::remove_dir_all(repo_path.join(".jj")).unwrap();
    std::fs::create_dir(repo_path.join(".jj")).unwrap();
    let stderr = test_env.jj_cmd_internal_error(&repo_path, &["log"]);
    insta::assert_snapshot!(strip_last_line(&stderr), @r###"
    Internal error: The repository appears broken or inaccessible
    Caused by:
    1: Failed to read commit backend type
    2: Cannot access $TEST_ENV/repo/.jj/repo/store/type
    "###);
}

#[test]
fn test_color_config() {
    let mut test_env = TestEnvironment::default();

    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Test that --color=always is respected.
    let stdout = test_env.jj_cmd_success(&repo_path, &["--color=always", "log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;2m@[0m  [38;5;4m230dd059e1b059aefc0da06a2e5a7dbf22362f22[39m
    [1m[38;5;14m◆[0m  [38;5;4m0000000000000000000000000000000000000000[39m
    "###);

    // Test that color is used if it's requested in the config file
    test_env.add_config(r#"ui.color="always""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;2m@[0m  [38;5;4m230dd059e1b059aefc0da06a2e5a7dbf22362f22[39m
    [1m[38;5;14m◆[0m  [38;5;4m0000000000000000000000000000000000000000[39m
    "###);

    // Test that --color=never overrides the config.
    let stdout = test_env.jj_cmd_success(&repo_path, &["--color=never", "log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◆  0000000000000000000000000000000000000000
    "###);

    // Test that --color=auto overrides the config.
    let stdout = test_env.jj_cmd_success(&repo_path, &["--color=auto", "log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◆  0000000000000000000000000000000000000000
    "###);

    // Test that --config 'ui.color=never' overrides the config.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["--config=ui.color=never", "log", "-T", "commit_id"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◆  0000000000000000000000000000000000000000
    "###);

    // --color overrides --config 'ui.color=...'.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "--color",
            "never",
            "--config=ui.color=always",
            "log",
            "-T",
            "commit_id",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◆  0000000000000000000000000000000000000000
    "###);

    // Test that NO_COLOR does NOT override the request for color in the config file
    test_env.add_env_var("NO_COLOR", "1");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;2m@[0m  [38;5;4m230dd059e1b059aefc0da06a2e5a7dbf22362f22[39m
    [1m[38;5;14m◆[0m  [38;5;4m0000000000000000000000000000000000000000[39m
    "###);

    // Test that per-repo config overrides the user config.
    std::fs::write(
        repo_path.join(".jj/repo/config.toml"),
        r#"ui.color = "never""#,
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id"]);
    insta::assert_snapshot!(stdout, @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◆  0000000000000000000000000000000000000000
    "###);

    // Invalid --color
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["log", "--color=foo"]);
    insta::assert_snapshot!(stderr, @r"
    error: invalid value 'foo' for '--color <WHEN>': unknown variant `foo`, expected one of `always`, `never`, `debug`, `auto`

    For more information, try '--help'.
    ");
    // Invalid ui.color
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "--config=ui.color=true"]);
    insta::assert_snapshot!(stderr, @r"
    Config error: Invalid type or value for ui.color
    Caused by: wanted string or table

    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");
}

#[test]
fn test_color_ui_messages() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config("ui.color = 'always'");

    // hint and error
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["-R."]);
    insta::assert_snapshot!(stderr, @r###"
    [1m[38;5;6mHint: [0m[39mUse `jj -h` for a list of available commands.[39m
    [39mRun `jj config set --user ui.default-command log` to disable this message.[39m
    [1m[38;5;1mError: [39mThere is no jj repo in "."[0m
    "###);

    // error source
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", ".."]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    [1m[38;5;1mError: [39mFailed to parse fileset: Invalid file pattern[0m
    [1m[39mCaused by:[0m
    [1m[39m1: [0m[39m --> 1:1[39m
    [39m  |[39m
    [39m1 | ..[39m
    [39m  | ^^[39m
    [39m  |[39m
    [39m  = Invalid file pattern[39m
    [1m[39m2: [0m[39mPath ".." is not in the repo "."[39m
    [1m[39m3: [0m[39mInvalid component ".." in repo-relative path "../"[39m
    "###);

    // warning
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    [1m[38;5;3mWarning: [39mThe argument "@" is being interpreted as a path. To specify a revset, pass -r "@" instead.[0m
    "###);

    // error inlined in template output
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-r@|@--",
            "--config=templates.log_node=commit_id",
            "-Tdescription",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    [38;5;4m167f90e7600a50f85c4f909b53eaf546faa82879[39m
    [1m[39m<[38;5;1mError: [39mNo Commit available>[0m  [38;5;8m(elided revisions)[39m
    [38;5;4m0000000000000000000000000000000000000000[39m
    "###);

    // formatted hint
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", ".."]);
    insta::assert_snapshot!(stderr, @r###"
    [1m[38;5;1mError: [39mRevset ".." resolved to more than one revision[0m
    [1m[38;5;6mHint: [0m[39mThe revset ".." resolved to these revisions:[39m
    [39m  [1m[38;5;5mm[0m[38;5;8mzvwutvl[39m [1m[38;5;4m1[0m[38;5;8m67f90e7[39m [38;5;2m(empty)[39m [38;5;2m(no description set)[39m[39m
    [39m  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m [38;5;2m(empty)[39m [38;5;2m(no description set)[39m[39m
    [1m[38;5;6mHint: [0m[39mPrefix the expression with 'all:' to allow any number of revisions (i.e. 'all:..').[39m
    "###);

    // debugging colors
    let (stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["st", "--color", "debug"]);
    insta::assert_snapshot!(stdout, @r###"
    The working copy is clean
    Working copy : [1m[38;5;13m<<working_copy change_id shortest prefix::m>>[38;5;8m<<working_copy change_id shortest rest::zvwutvl>>[39m<<working_copy:: >>[38;5;12m<<working_copy commit_id shortest prefix::1>>[38;5;8m<<working_copy commit_id shortest rest::67f90e7>>[39m<<working_copy:: >>[38;5;10m<<working_copy empty::(empty)>>[39m<<working_copy:: >>[38;5;10m<<working_copy empty description placeholder::(no description set)>>[0m
    Parent commit: [1m[38;5;5m<<change_id shortest prefix::q>>[0m[38;5;8m<<change_id shortest rest::pvuntsm>>[39m [1m[38;5;4m<<commit_id shortest prefix::2>>[0m[38;5;8m<<commit_id shortest rest::30dd059>>[39m [38;5;2m<<empty::(empty)>>[39m [38;5;2m<<empty description placeholder::(no description set)>>[39m
    "###);
}

#[test]
fn test_quiet() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Can skip message about new working copy with `--quiet`
    std::fs::write(repo_path.join("file1"), "contents").unwrap();
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["--quiet", "describe", "-m=new description"]);
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_early_args() {
    // Test that help output parses early args
    let test_env = TestEnvironment::default();

    // The default is no color.
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["help"]);
    insta::assert_snapshot!(stdout.lines().find(|l| l.contains("Commands:")).unwrap(), @"Commands:");

    // Check that output is colorized.
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["--color=always", "help"]);
    insta::assert_snapshot!(stdout.lines().find(|l| l.contains("Commands:")).unwrap(), @"[1m[4mCommands:[0m");

    // Check that early args are accepted after the help command
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["help", "--color=always"]);
    insta::assert_snapshot!(stdout.lines().find(|l| l.contains("Commands:")).unwrap(), @"[1m[4mCommands:[0m");

    // Check that early args are accepted after -h/--help
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["-h", "--color=always"]);
    insta::assert_snapshot!(
        stdout.lines().find(|l| l.contains("Usage:")).unwrap(),
        @"[1m[4mUsage:[0m [1mjj[0m [OPTIONS] <COMMAND>");
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["log", "--help", "--color=always"]);
    insta::assert_snapshot!(
        stdout.lines().find(|l| l.contains("Usage:")).unwrap(),
        @"[1m[4mUsage:[0m [1mjj log[0m [OPTIONS] [FILESETS]...");

    // Early args are parsed with clap's ignore_errors(), but there is a known
    // bug that causes defaults to be unpopulated. Test that the early args are
    // tolerant of this bug and don't cause a crash.
    test_env.jj_cmd_success(test_env.env_root(), &["--no-pager", "help"]);
    test_env.jj_cmd_success(test_env.env_root(), &["--config=ui.color=always", "help"]);
}

#[test]
fn test_config_args() {
    let test_env = TestEnvironment::default();
    let list_config = |args: &[&str]| {
        // Suppress deprecation warning of --config-toml
        let (stdout, _stderr) = test_env.jj_cmd_ok(
            test_env.env_root(),
            &[&["config", "list", "--include-overridden", "test"], args].concat(),
        );
        stdout
    };

    std::fs::write(
        test_env.env_root().join("file1.toml"),
        indoc! {"
            test.key1 = 'file1'
            test.key2 = 'file1'
        "},
    )
    .unwrap();
    std::fs::write(
        test_env.env_root().join("file2.toml"),
        indoc! {"
            test.key3 = 'file2'
        "},
    )
    .unwrap();

    let stdout = list_config(&["--config=test.key1=arg1"]);
    insta::assert_snapshot!(stdout, @r#"test.key1 = "arg1""#);
    let stdout = list_config(&["--config-toml=test.key1='arg1'"]);
    insta::assert_snapshot!(stdout, @"test.key1 = 'arg1'");
    let stdout = list_config(&["--config-file=file1.toml"]);
    insta::assert_snapshot!(stdout, @r"
    test.key1 = 'file1'
    test.key2 = 'file1'
    ");

    // --config items are inserted to a single layer internally
    let stdout = list_config(&[
        "--config=test.key1='arg1'",
        "--config=test.key2.sub=true",
        "--config=test.key1=arg3",
    ]);
    insta::assert_snapshot!(stdout, @r#"
    test.key1 = "arg3"
    test.key2.sub = true
    "#);

    // --config* arguments are processed in order of appearance
    let stdout = list_config(&[
        "--config=test.key1=arg1",
        "--config-file=file1.toml",
        "--config-toml=test.key2='arg3'",
        "--config-file=file2.toml",
    ]);
    insta::assert_snapshot!(stdout, @r##"
    # test.key1 = "arg1"
    test.key1 = 'file1'
    # test.key2 = 'file1'
    test.key2 = 'arg3'
    test.key3 = 'file2'
    "##);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        test_env.env_root(),
        &["config", "list", "foo", "--config-toml=foo='bar'"],
    );
    insta::assert_snapshot!(stdout, @"foo = 'bar'");
    insta::assert_snapshot!(
        stderr,
        @"Warning: --config-toml is deprecated; use --config or --config-file instead.");

    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["config", "list", "--config=foo"]);
    insta::assert_snapshot!(stderr, @r"
    Config error: --config must be specified as NAME=VALUE
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");

    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["config", "list", "--config-file=unknown.toml"],
    );
    insta::with_settings!({
        filters => [("(?m)^([2-9]): .*", "$1: <redacted>")],
    }, {
        insta::assert_snapshot!(stderr, @r"
        Config error: Failed to read configuration file
        Caused by:
        1: Cannot access unknown.toml
        2: <redacted>
        For help, see https://jj-vcs.github.io/jj/latest/config/.
        ");
    });
}

#[test]
fn test_invalid_config() {
    // Test that we get a reasonable error if the config is invalid (#55)
    let test_env = TestEnvironment::default();

    test_env.add_config("[section]key = value-missing-quotes");
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["init", "repo"]);
    insta::assert_snapshot!(stderr, @r"
    Config error: Configuration cannot be parsed as TOML document
    Caused by: TOML parse error at line 1, column 10
      |
    1 | [section]key = value-missing-quotes
      |          ^
    invalid table header
    expected newline, `#`

    Hint: Check the config file: $TEST_ENV/config/config0002.toml
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");
}

#[test]
fn test_invalid_config_value() {
    // Test that we get a reasonable error if a config value is invalid
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["status", "--config=snapshot.auto-track=[0]"]);
    insta::assert_snapshot!(stderr, @r"
    Config error: Invalid type or value for snapshot.auto-track
    Caused by: invalid type: sequence, expected a string

    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");
}

#[test]
#[cfg_attr(windows, ignore = "dirs::home_dir() can't be overridden by $HOME")] // TODO
fn test_conditional_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.home_dir(), &["git", "init", "repo1"]);
    test_env.jj_cmd_ok(test_env.home_dir(), &["git", "init", "repo2"]);
    test_env.add_config(indoc! {"
        aliases.foo = ['new', 'root()', '-mglobal']
        [[--scope]]
        --when.repositories = ['~']
        aliases.foo = ['new', 'root()', '-mhome']
        [[--scope]]
        --when.repositories = ['~/repo1']
        aliases.foo = ['new', 'root()', '-mrepo1']
    "});

    // Sanity check
    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["config", "list", "--include-overridden", "aliases"],
    );
    insta::assert_snapshot!(stdout, @"aliases.foo = ['new', 'root()', '-mglobal']");
    let stdout = test_env.jj_cmd_success(
        &test_env.home_dir().join("repo1"),
        &["config", "list", "--include-overridden", "aliases"],
    );
    insta::assert_snapshot!(stdout, @r"
    # aliases.foo = ['new', 'root()', '-mglobal']
    # aliases.foo = ['new', 'root()', '-mhome']
    aliases.foo = ['new', 'root()', '-mrepo1']
    ");
    let stdout = test_env.jj_cmd_success(
        &test_env.home_dir().join("repo2"),
        &["config", "list", "--include-overridden", "aliases"],
    );
    insta::assert_snapshot!(stdout, @r"
    # aliases.foo = ['new', 'root()', '-mglobal']
    aliases.foo = ['new', 'root()', '-mhome']
    ");

    // Aliases can be expanded by using the conditional tables
    let (_stdout, stderr) = test_env.jj_cmd_ok(&test_env.home_dir().join("repo1"), &["foo"]);
    insta::assert_snapshot!(stderr, @r"
    Working copy now at: royxmykx 82899b03 (empty) repo1
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    ");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&test_env.home_dir().join("repo2"), &["foo"]);
    insta::assert_snapshot!(stderr, @r"
    Working copy now at: yqosqzyt 3bd315a9 (empty) home
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    ");
}

#[test]
fn test_no_user_configured() {
    // Test that the user is reminded if they haven't configured their name or email
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let assert = test_env
        .jj_cmd(&repo_path, &["describe", "-m", "without name"])
        .env_remove("JJ_USER")
        .assert()
        .success();
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Working copy now at: qpvuntsm 7a7d6016 (empty) without name
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Warning: Name not configured. Until configured, your commits will be created with the empty identity, and can't be pushed to remotes. To configure, run:
      jj config set --user user.name "Some One"
    "###);
    let assert = test_env
        .jj_cmd(&repo_path, &["describe", "-m", "without email"])
        .env_remove("JJ_EMAIL")
        .assert()
        .success();
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Working copy now at: qpvuntsm 906f8b89 (empty) without email
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Warning: Email not configured. Until configured, your commits will be created with the empty identity, and can't be pushed to remotes. To configure, run:
      jj config set --user user.email "someone@example.com"
    "###);
    let assert = test_env
        .jj_cmd(&repo_path, &["describe", "-m", "without name and email"])
        .env_remove("JJ_USER")
        .env_remove("JJ_EMAIL")
        .assert()
        .success();
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Working copy now at: qpvuntsm 57d3a489 (empty) without name and email
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Warning: Name and email not configured. Until configured, your commits will be created with the empty identity, and can't be pushed to remotes. To configure, run:
      jj config set --user user.name "Some One"
      jj config set --user user.email "someone@example.com"
    "###);
}

#[test]
fn test_help() {
    // Test that global options are separated out in the help output
    let test_env = TestEnvironment::default();

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["diffedit", "-h"]);
    insta::assert_snapshot!(stdout, @r###"
    Touch up the content changes in a revision with a diff editor

    Usage: jj diffedit [OPTIONS]

    Options:
      -r, --revision <REVSET>    The revision to touch up
      -f, --from <REVSET>        Show changes from this revision
      -t, --to <REVSET>          Edit changes in this revision
          --tool <NAME>          Specify diff editor to be used
          --restore-descendants  Preserve the content (not the diff) when rebasing descendants
      -h, --help                 Print help (see more with '--help')

    Global Options:
      -R, --repository <REPOSITORY>      Path to repository to operate on
          --ignore-working-copy          Don't snapshot the working copy, and don't update it
          --ignore-immutable             Allow rewriting immutable commits
          --at-operation <AT_OPERATION>  Operation to load the repo at [aliases: at-op]
          --debug                        Enable debug logging
          --color <WHEN>                 When to colorize output (always, never, debug, auto)
          --quiet                        Silence non-primary command output
          --no-pager                     Disable the pager
          --config <NAME=VALUE>          Additional configuration options (can be repeated)
          --config-file <PATH>           Additional configuration files (can be repeated)
    "###);
}

#[test]
fn test_debug_logging_enabled() {
    // Test that the debug flag enabled debug logging
    let test_env = TestEnvironment::default();

    let (_stdout, stderr) = test_env.jj_cmd_ok(test_env.env_root(), &["version", "--debug"]);
    // Split the first log line into a timestamp and the rest.
    // The timestamp is constant sized so this is a robust operation.
    // Example timestamp: 2022-11-20T06:24:05.477703Z
    let (_timestamp, log_line) = stderr
        .lines()
        .next()
        .expect("debug logging on first line")
        .split_at(36);
    // The log format is currently Pretty so we include the terminal markup.
    // Luckily, insta will print this in colour when reviewing.
    insta::assert_snapshot!(log_line, @"[32m INFO[0m [2mjj_cli::cli_util[0m[2m:[0m debug logging enabled");
}
