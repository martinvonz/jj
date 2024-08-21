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
use std::path::PathBuf;

use crate::common::TestEnvironment;

#[test]
fn test_move() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("file3"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    std::fs::write(repo_path.join("file3"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["edit", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "d"]);
    std::fs::write(repo_path.join("file3"), "d\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "e"]);
    std::fs::write(repo_path.join("file2"), "e\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "f"]);
    std::fs::write(repo_path.join("file2"), "f\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  a847ab4967fe f
    ○  c2f9de87325d e
    ○  e0dac715116f d
    │ ○  59597b34a0d8 c
    │ ○  12d6103dc0c8 b
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
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
    Working copy now at: kmkuslsw a45950b1 f | (no description set)
    Parent commit      : znkkpsqq c2f9de87 e | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  a45950b1b7ff f
    ○  c2f9de87325d e
    ○  e0dac715116f d
    │ ○  12d6103dc0c8 b c
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);
    // The change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // File `file2`, which was not changed in source, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
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
    Working copy now at: kmkuslsw 5e5727af f | (no description set)
    Parent commit      : znkkpsqq ed9c4164 e | (no description set)
    "###);
    // The change has been removed from the source (the change pointed to by 'd'
    // became empty and was abandoned)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  5e5727af3d75 f
    ○  ed9c41643a77 e
    │ ○  59597b34a0d8 c
    │ ○  12d6103dc0c8 b
    ├─╯
    ○  b7b767179c44 a d
    ◆  000000000000
    "###);
    // The change from the source has been applied (the file contents were already
    // "f", as is typically the case when moving changes from an ancestor)
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
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
    Working copy now at: kmkuslsw e21f6bb0 f | (no description set)
    Parent commit      : vruxwmqv 3cf0fa77 d e | (no description set)
    "###);
    // The change has been removed from the source (the change pointed to by 'e'
    // became empty and was abandoned)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e21f6bb01bae f
    ○  3cf0fa772663 d e
    │ ○  59597b34a0d8 c
    │ ○  12d6103dc0c8 b
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);
    // The change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2", "-r", "d"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_move_partial() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Create history like this:
    //   C
    //   |
    // D B
    // |/
    // A
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("file3"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    std::fs::write(repo_path.join("file3"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["edit", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "d"]);
    std::fs::write(repo_path.join("file3"), "d\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e0dac715116f d
    │ ○  087591be5a01 c
    │ ○  12d6103dc0c8 b
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);

    let edit_script = test_env.set_up_fake_diff_editor();

    // If we don't make any changes in the diff-editor, the whole change is moved
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "-i", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Working copy now at: vruxwmqv 987bcfb2 d | (no description set)
    Parent commit      : qpvuntsm b7b76717 a | (no description set)
    Added 0 files, modified 2 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  987bcfb2eb62 d
    │ ○  12d6103dc0c8 b c
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);
    // The changes from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // File `file3`, which was not changed in source, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file3"]);
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
    Working copy now at: vruxwmqv 576244e8 d | (no description set)
    Parent commit      : qpvuntsm b7b76717 a | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  576244e87883 d
    │ ○  6f486f2f4539 c
    │ ○  12d6103dc0c8 b
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);
    // The selected change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // The unselected change from the source has not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    // File `file3`, which was changed in source's parent, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file3"]);
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
    Working copy now at: vruxwmqv 5b407c24 d | (no description set)
    Parent commit      : qpvuntsm b7b76717 a | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  5b407c249fa7 d
    │ ○  724d64da1487 c
    │ ○  12d6103dc0c8 b
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);
    // The selected change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // The unselected change from the source has not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    // File `file3`, which was changed in source's parent, is unchanged
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file3"]);
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
    ○  d2a587ae205d c
    ○  a53394306362 b
    │ @  e0dac715116f d
    ├─╯
    ○  b7b767179c44 a
    ◆  000000000000
    "###);
    // The selected change from the source has been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    // The unselected change from the source has not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);

    // If we specify only a non-existent file, then nothing changes.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["move", "--from", "c", "nonexistent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Nothing changed.
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ bookmarks"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

#[test]
fn test_move_description() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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

#[test]
fn test_move_description_editor_avoids_unc() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "@-", "-m", "destination"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "source"]);

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["move", "--to", "@-"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    // While `assert!(!edited_path.starts_with("//?/"))` could work here in most
    // cases, it fails when it is not safe to strip the prefix, such as paths
    // over 260 chars.
    assert_eq!(edited_path, dunce::simplified(&edited_path));
}

fn get_description(test_env: &TestEnvironment, repo_path: &Path, rev: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "--no-graph", "-T", "description", "-r", rev],
    )
}
