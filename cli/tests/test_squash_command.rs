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
fn test_squash() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  90fe0a96fc90 c
    ◉  fa5efbdf533c b
    ◉  90aeefd03044 a
    ◉  000000000000
    "###);

    // Squashes the working copy into the parent by default
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv b9280a98 (empty) (no description set)
    Parent commit      : kkmpptxz 6ca29c9d b c | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  b9280a9898cb
    ◉  6ca29c9d2e7c b c
    ◉  90aeefd03044 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Can squash a given commit into its parent
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-r", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl e87cf8eb c | (no description set)
    Parent commit      : qpvuntsm 893c93ae a b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e87cf8ebc7e1 c
    ◉  893c93ae2a87 a b
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Cannot squash a merge commit (because it's unclear which parent it should go
    // into)
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);
    std::fs::write(repo_path.join("file2"), "d\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "c", "d"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "e"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    c7a11b36d333 e
    ├─╮
    │ ◉  5658521e0f8b d
    ◉ │  90fe0a96fc90 c
    ├─╯
    ◉  fa5efbdf533c b
    ◉  90aeefd03044 a
    ◉  000000000000
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot squash merge commits
    "###);

    // Can squash into a merge commit
    test_env.jj_cmd_ok(&repo_path, &["new", "e"]);
    std::fs::write(repo_path.join("file1"), "e\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: xlzxqlsl 959145c1 (empty) (no description set)
    Parent commit      : nmzmmopx 80960125 e | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  959145c11426
    ◉    80960125bb96 e
    ├─╮
    │ ◉  5658521e0f8b d
    ◉ │  90fe0a96fc90 c
    ├─╯
    ◉  fa5efbdf533c b
    ◉  90aeefd03044 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "e"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_squash_partial() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d989314f3df0 c
    ◉  2a2d19a3283f b
    ◉  47a1e795d146 a
    ◉  000000000000
    "###);

    // If we don't make any changes in the diff-editor, the whole change is moved
    // into the parent
    let edit_script = test_env.set_up_fake_diff_editor();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl f03d5ce4 c | (no description set)
    Parent commit      : qpvuntsm c9f931cd a b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  f03d5ce4a973 c
    ◉  c9f931cd78af a b
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // Can squash only some changes in interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "reset file1").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    Working copy now at: mzvwutvl e7a40106 c | (no description set)
    Parent commit      : kkmpptxz 05d95164 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  e7a40106bee6 c
    ◉  05d951646873 b
    ◉  0c5ddc685260 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // Can squash only some changes in non-interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Clear the script so we know it won't be used even without -i
    std::fs::write(&edit_script, "").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-r", "b", "file2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    Working copy now at: mzvwutvl a911fa1d c | (no description set)
    Parent commit      : kkmpptxz fb73ad17 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  a911fa1d0627 c
    ◉  fb73ad17899f b
    ◉  70621f4c7a42 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // If we specify only a non-existent file, then nothing changes.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-r", "b", "nonexistent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // We get a warning if we pass a positional argument that looks like a revset
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: The argument "b" is being interpreted as a path. To specify a revset, pass -r "b" instead.
    Nothing changed.
    "###);
    insta::assert_snapshot!(stdout, @"");
}

#[test]
fn test_squash_from_to() {
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

    // Errors out if source and destination are the same
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash", "--into", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Source and destination cannot be the same
    "###);

    // Can squash from sibling, which results in the source being abandoned
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kmkuslsw 5337fca9 f | (no description set)
    Parent commit      : znkkpsqq e9515f21 e | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  5337fca918e8 f
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

    // Can squash from ancestor
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "--from", "@--"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kmkuslsw 66ff309f f | (no description set)
    Parent commit      : znkkpsqq 16f4e7c4 e | (no description set)
    "###);
    // The change has been removed from the source (the change pointed to by 'd'
    // became empty and was abandoned)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  66ff309f65e8 f
    ◉  16f4e7c4886f e
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

    // Can squash from descendant
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["squash", "--from", "e", "--into", "d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: kmkuslsw b4f8051d f | (no description set)
    Parent commit      : vruxwmqv f74c102f d e | (no description set)
    "###);
    // The change has been removed from the source (the change pointed to by 'e'
    // became empty and was abandoned)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  b4f8051d8466 f
    ◉  f74c102ff29a d e
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
fn test_squash_from_to_partial() {
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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-i", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
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

    // Can squash only part of the change in interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "reset file2").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-i", "--from", "c"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
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

    // Can squash only part of the change from a sibling in non-interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Clear the script so we know it won't be used
    std::fs::write(&edit_script, "").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "--from", "c", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
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

    // Can squash only part of the change from a descendant in non-interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // Clear the script so we know it won't be used
    std::fs::write(&edit_script, "").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["squash", "--from", "c", "--into", "b", "file1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
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

    // If we specify only a non-existent file, then nothing changes.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["squash", "--from", "c", "nonexistent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_squash_from_multiple() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Create history like this:
    //   F
    //   |
    //   E
    //  /|\
    // B C D
    //  \|/
    //   A
    let file = repo_path.join("file");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    std::fs::write(&file, "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    std::fs::write(&file, "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    std::fs::write(&file, "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);
    std::fs::write(&file, "d\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "all:visible_heads()"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "e"]);
    std::fs::write(&file, "e\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "f"]);
    std::fs::write(&file, "f\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  7c982f87d244 f
    ◉      90fb23310e1d e
    ├─┬─╮
    │ │ ◉  512dff087306 b
    │ ◉ │  5ee503da2262 c
    │ ├─╯
    ◉ │  cb214cffd91a d
    ├─╯
    ◉  37941ee54ace a
    ◉  000000000000
    "###);

    // Squash a few commits sideways
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["squash", "--from=b", "--from=c", "--into=d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    New conflicts appeared in these commits:
      yqosqzyt 50bd7d24 d | (conflict) (no description set)
    To resolve the conflicts, start by updating to it:
      jj new yqosqzytrlsw
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: kpqxywon dd653e49 f | (no description set)
    Parent commit      : yostqsxw e40f2544 e | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  dd653e494199 f
    ◉    e40f2544ad31 e
    ├─╮
    ◉ │  50bd7d246d8e d
    ├─╯
    ◉  37941ee54ace a b c
    ◉  000000000000
    "###);
    // The changes from the sources have been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=d", "file"]);
    insta::assert_snapshot!(stdout, @r###"
    <<<<<<<
    %%%%%%%
    -a
    +d
    %%%%%%%
    -a
    +b
    +++++++
    c
    >>>>>>>
    "###);

    // Squash a few commits up an down
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "--from=b|c|f", "--into=e"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: xznxytkn 59801ce3 (empty) (no description set)
    Parent commit      : yostqsxw b7bc1dda e f | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  59801ce3ff81
    ◉    b7bc1dda247e e f
    ├─╮
    ◉ │  cb214cffd91a d
    ├─╯
    ◉  37941ee54ace a b c
    ◉  000000000000
    "###);
    // The changes from the sources have been applied to the destination
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=e", "file"]);
    insta::assert_snapshot!(stdout, @r###"
    f
    "###);

    // Empty squash shouldn't crash
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "--from=none()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_squash_from_multiple_partial() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Create history like this:
    //   F
    //   |
    //   E
    //  /|\
    // B C D
    //  \|/
    //   A
    let file1 = repo_path.join("file1");
    let file2 = repo_path.join("file2");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "a"]);
    std::fs::write(&file1, "a\n").unwrap();
    std::fs::write(&file2, "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "b"]);
    std::fs::write(&file1, "b\n").unwrap();
    std::fs::write(&file2, "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "c"]);
    std::fs::write(&file1, "c\n").unwrap();
    std::fs::write(&file2, "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);
    std::fs::write(&file1, "d\n").unwrap();
    std::fs::write(&file2, "d\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "all:visible_heads()"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "e"]);
    std::fs::write(&file1, "e\n").unwrap();
    std::fs::write(&file2, "e\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "f"]);
    std::fs::write(&file1, "f\n").unwrap();
    std::fs::write(&file2, "f\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  5adc4b1fb0f9 f
    ◉      8ba764396a28 e
    ├─┬─╮
    │ │ ◉  2a2d19a3283f b
    │ ◉ │  864a16169cef c
    │ ├─╯
    ◉ │  5def0e76dfaf d
    ├─╯
    ◉  47a1e795d146 a
    ◉  000000000000
    "###);

    // Partially squash a few commits sideways
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["squash", "--from=b|c", "--into=d", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    New conflicts appeared in these commits:
      yqosqzyt 85d3ae29 d | (conflict) (no description set)
    To resolve the conflicts, start by updating to it:
      jj new yqosqzytrlsw
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: kpqxywon 97861bbf f | (no description set)
    Parent commit      : yostqsxw 2dbaf4e8 e | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  97861bbf7ae5 f
    ◉      2dbaf4e8c7f7 e
    ├─┬─╮
    │ │ ◉  ba60ddff2d41 b
    │ ◉ │  8ef5a315bf7d c
    │ ├─╯
    ◉ │  85d3ae290b9b d
    ├─╯
    ◉  47a1e795d146 a
    ◉  000000000000
    "###);
    // The selected changes have been removed from the sources
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=b", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=c", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    // The selected changes from the sources have been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=d", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    <<<<<<<
    %%%%%%%
    -a
    +d
    %%%%%%%
    -a
    +b
    +++++++
    c
    >>>>>>>
    "###);
    // The unselected change from the sources have not been applied to the
    // destination
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=d", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    d
    "###);

    // Partially squash a few commits up an down
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["squash", "--from=b|c|f", "--into=e", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: kpqxywon 610a144d f | (no description set)
    Parent commit      : yostqsxw ac27a136 e | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  610a144de39b f
    ◉      ac27a1361b09 e
    ├─┬─╮
    │ │ ◉  0c8eab864a32 b
    │ ◉ │  ad1776ad0b1b c
    │ ├─╯
    ◉ │  5def0e76dfaf d
    ├─╯
    ◉  47a1e795d146 a
    ◉  000000000000
    "###);
    // The selected changes have been removed from the sources
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=b", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=c", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=f", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    f
    "###);
    // The selected changes from the sources have been applied to the destination
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=e", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    f
    "###);
    // The unselected changes from the sources have not been applied
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "-r=d", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    d
    "###);
}

#[test]
fn test_squash_from_multiple_partial_no_op() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Create history like this:
    // B C D
    //  \|/
    //   A
    let file_a = repo_path.join("a");
    let file_b = repo_path.join("b");
    let file_c = repo_path.join("c");
    let file_d = repo_path.join("d");
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=a"]);
    std::fs::write(file_a, "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=b"]);
    std::fs::write(file_b, "b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m=c"]);
    std::fs::write(file_c, "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m=d"]);
    std::fs::write(file_d, "d\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  09441f0a6266 d
    │ ◉  5ad3ca4090a7 c
    ├─╯
    │ ◉  285201979c90 b
    ├─╯
    ◉  3df52ee1f8a9 a
    ◉  000000000000
    "###);

    // Source commits that didn't match the paths are not rewritten
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["squash", "--from=@-+ ~ @", "--into=@", "-m=d", "b"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl 9227d0d7 d
    Parent commit      : qpvuntsm 3df52ee1 a
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  9227d0d780fa d
    │ ◉  5ad3ca4090a7 c
    ├─╯
    ◉  3df52ee1f8a9 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "obslog",
            "-T",
            r#"separate(" ", commit_id.short(), description)"#,
        ],
    );
    // TODO: Commit c should not be a predecessor
    insta::assert_snapshot!(stdout, @r###"
    @      9227d0d780fa d
    ├─┬─╮
    ◉ │ │  09441f0a6266 d
    ◉ │ │  cba0f0aa472b d
      ◉ │  285201979c90 b
      ◉ │  81187418277d b
        ◉  5ad3ca4090a7 c
        ◉  7cfbaf71a279 c
    "###);

    // If no source commits match the paths, then the whole operation is a no-op
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["squash", "--from=@-+ ~ @", "--into=@", "-m=d", "a"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  09441f0a6266 d
    │ ◉  5ad3ca4090a7 c
    ├─╯
    │ ◉  285201979c90 b
    ├─╯
    ◉  3df52ee1f8a9 a
    ◉  000000000000
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"separate(" ", commit_id.short(), branches, description)"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}

#[test]
fn test_squash_description() {
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
    test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @"");

    // If the destination's description is empty and the source's description is
    // non-empty, the resulting description is from the source
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "source"]);
    test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    source
    "###);

    // If the destination description is non-empty and the source's description is
    // empty, the resulting description is from the destination
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "@-", "-m", "destination"]);
    test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    destination
    "###);

    // An explicit description on the command-line overrides this
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["squash", "-m", "custom"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    custom
    "###);

    // If both descriptions were non-empty, we get asked for a combined description
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "source"]);
    std::fs::write(&edit_script, "dump editor0").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["squash"]);
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

    // An explicit description on the command-line overrides prevents launching an
    // editor
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["squash", "-m", "custom"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    custom
    "###);

    // If the source's *content* doesn't become empty, then the source remains and
    // both descriptions are unchanged
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["squash", "file1"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    destination
    "###);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @r###"
    source
    "###);
}

#[test]
fn test_squash_empty() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "parent"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz e45abe2c (empty) (no description set)
    Parent commit      : qpvuntsm 1265289b (empty) parent
    "###);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    parent
    "###);

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "child"]);
    test_env.set_up_fake_editor();
    test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    parent

    child
    "###);
}

#[test]
fn test_squash_use_destination_message() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-m=a"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m=b"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_with_description(&test_env, &repo_path), @r###"
    @  71f7c810d8ed c
    ◉  10dd87c3b4e2 b
    ◉  4c5b3042d9e0 a
    ◉  000000000000
    "###);

    // Squash the current revision using the short name for the option.
    test_env.jj_cmd_ok(&repo_path, &["squash", "-u"]);
    insta::assert_snapshot!(get_log_output_with_description(&test_env, &repo_path), @r###"
    @  10e30ce4a910
    ◉  1c21278b775f b
    ◉  4c5b3042d9e0 a
    ◉  000000000000
    "###);

    // Undo and squash again, but this time squash both "b" and "c" into "a".
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "squash",
            "--use-destination-message",
            "--from",
            "description(b)::",
            "--into",
            "description(a)",
        ],
    );
    insta::assert_snapshot!(get_log_output_with_description(&test_env, &repo_path), @r###"
    @  da1507508bdf
    ◉  f1387f804776 a
    ◉  000000000000
    "###);
}

// The --use-destination-message and --message options are incompatible.
#[test]
fn test_squash_use_destination_message_and_message_mutual_exclusion() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m=a"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=b"]);
    insta::assert_snapshot!(test_env.jj_cmd_cli_error(
        &repo_path,
        &[
            "squash",
            "--message=123",
            "--use-destination-message",
        ],
    ), @r###"
    error: the argument '--message <MESSAGE>' cannot be used with '--use-destination-message'

    Usage: jj squash --message <MESSAGE> [PATHS]...

    For more information, try '--help'.
    "###);
}

fn get_description(test_env: &TestEnvironment, repo_path: &Path, rev: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "--no-graph", "-T", "description", "-r", rev],
    )
}

fn get_log_output_with_description(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"separate(" ", commit_id.short(), description)"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}
