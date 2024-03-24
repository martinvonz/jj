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

#[test]
fn test_move() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Create history like this:
    // F
    // |
    // E C
    // | |
    // D B
    // |/
    // A
    //
    // When moving changes between e.g. C and F, we should not get unrelated changes
    // from B and D.
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("file3"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file3"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["edit", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);
    std::fs::write(repo_path.join("file3"), "d\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "e"]);
    std::fs::write(repo_path.join("file2"), "e\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "f"]);
    std::fs::write(repo_path.join("file2"), "f\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  0d7353584003 f
    ◉  e9515f21068c e
    ◉  bdd835cae844 d
    │ ◉  caa4d0b23201 c
    │ ◉  55171e33db26 b
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);

    // Errors out without arguments
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["move"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      <--from <FROM>|--to <TO>>

    Usage: jj move <--from <FROM>|--to <TO>> [PATHS]...

    For more information, try '--help'.
    "###);
    // Errors out if source and destination are the same
    let stderr = test_env.jj_cmd_failure(&repo_path, &["move", "--to", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Error: Source and destination cannot be the same.
    "###);

    // Can move from sibling, which results in the source being abandoned
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: kmkuslsw 1c03e3d3 f | (no description set)
    Parent commit      : znkkpsqq e9515f21 e | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  1c03e3d3c63f f
    ◉  e9515f21068c e
    ◉  bdd835cae844 d
    │ ◉  55171e33db26 b c
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);
    // The change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // File `file2`, which was not changed in source, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    f
    "###);

    // Can move from ancestor
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "--from", "@--"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: kmkuslsw c8d83075 f | (no description set)
    Parent commit      : znkkpsqq 2c50bfc5 e | (no description set)
    "###);
    // The change has been removed from the source (the change pointed to by 'd'
    // became empty and was abandoned)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c8d83075e8c2 f
    ◉  2c50bfc59c68 e
    │ ◉  caa4d0b23201 c
    │ ◉  55171e33db26 b
    ├─╯
    ◉  3db0a2f5b535 a d
    ◉  000000000000
    "###);
    // The change from the source has been applied (the file contents were already
    // "f", as is typically the case when moving changes from an ancestor)
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    f
    "###);

    // Can move from descendant
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "--from", "e", "--to", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Rebased 1 descendant commits
    Working copy now at: kmkuslsw 2b723b1d f | (no description set)
    Parent commit      : vruxwmqv 4293930d d e | (no description set)
    "###);
    // The change has been removed from the source (the change pointed to by 'e'
    // became empty and was abandoned)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2b723b1d6033 f
    ◉  4293930d6333 d e
    │ ◉  caa4d0b23201 c
    │ ◉  55171e33db26 b
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);
    // The change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "d"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_move_partial() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Create history like this:
    //   C
    //   |
    // D B
    // |/
    // A
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("file3"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file3"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["edit", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);
    std::fs::write(repo_path.join("file3"), "d\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bdd835cae844 d
    │ ◉  5028db694b6b c
    │ ◉  55171e33db26 b
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);

    let edit_script = test_env.set_up_fake_diff_editor();

    // If we don't make any changes in the diff-editor, the whole change is moved
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "-i", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: vruxwmqv 71b69e43 d | (no description set)
    Parent commit      : qpvuntsm 3db0a2f5 a | (no description set)
    Added 0 files, modified 2 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  71b69e433fbc d
    │ ◉  55171e33db26 b c
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);
    // The changes from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // File `file3`, which was not changed in source, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file3"]);
    insta::assert_snapshot!(stdout, @r###"
    d
    "###);

    // Can move only part of the change in interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "reset file2").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "-i", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: vruxwmqv 63f1a6e9 d | (no description set)
    Parent commit      : qpvuntsm 3db0a2f5 a | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  63f1a6e96edb d
    │ ◉  d027c6e3e6bc c
    │ ◉  55171e33db26 b
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);
    // The selected change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // The unselected change from the source has not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    // File `file3`, which was changed in source's parent, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file3"]);
    insta::assert_snapshot!(stdout, @r###"
    d
    "###);

    // Can move only part of the change from a sibling in non-interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Clear the script so we know it won't be used
    std::fs::write(&edit_script, "").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "--from", "c", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: vruxwmqv 17c2e663 d | (no description set)
    Parent commit      : qpvuntsm 3db0a2f5 a | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  17c2e6632cc5 d
    │ ◉  6a3ae047a03e c
    │ ◉  55171e33db26 b
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);
    // The selected change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // The unselected change from the source has not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    // File `file3`, which was changed in source's parent, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file3"]);
    insta::assert_snapshot!(stdout, @r###"
    d
    "###);

    // Can move only part of the change from a descendant in non-interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Clear the script so we know it won't be used
    std::fs::write(&edit_script, "").unwrap();
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["move", "--from", "c", "--to", "b", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Rebased 1 descendant commits
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  21253406d416 c
    ◉  e1cf08aae711 b
    │ @  bdd835cae844 d
    ├─╯
    ◉  3db0a2f5b535 a
    ◉  000000000000
    "###);
    // The selected change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // The unselected change from the source has not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);

    // If we specify only a non-existent file, then the move still succeeds and
    // creates unchanged commits.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "--from", "c", "nonexistent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: vruxwmqv b670567d d | (no description set)
    Parent commit      : qpvuntsm 3db0a2f5 a | (no description set)
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ branches"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

#[test]
fn test_move_description() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(&edit_script, r#"fail"#).unwrap();

    // If both descriptions are empty, the resulting description is empty
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["move", "--to", "@-"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @"");

    // If the destination's description is empty and the source's description is
    // non-empty, the resulting description is from the source
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "source"]);
    test_env.jj_cmd_ok(&repo_path, &["move", "--to", "@-"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    source
    "###);

    // If the destination's description is non-empty and the source's description is
    // empty, the resulting description is from the destination
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "@-", "-m", "destination"]);
    test_env.jj_cmd_ok(&repo_path, &["move", "--to", "@-"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    destination
    "###);

    // If both descriptions were non-empty, we get asked for a combined description
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "source"]);
    std::fs::write(&edit_script, "dump editor0").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["move", "--to", "@-"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    destination

    source
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    JJ: Enter a description for the combined commit.
    JJ: Description from the destination commit:
    destination

    JJ: Description from source commit:
    source

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);

    // If the source's *content* doesn't become empty, then the source remains and
    // both descriptions are unchanged
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["move", "--to", "@-", "file1"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    destination
    "###);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @r###"
    source
    "###);
}

fn get_description(test_env: &TestEnvironment, repo_path: &Path, rev: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "--no-graph", "-T", "description", "-r", rev],
    )
}
