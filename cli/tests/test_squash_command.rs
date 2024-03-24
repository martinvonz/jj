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

    // If we specify only a non-existent file, then the squash still succeeds and
    // creates unchanged commits.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "-r", "b", "nonexistent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    Working copy now at: mzvwutvl 5e297967 c | (no description set)
    Parent commit      : kkmpptxz ac258609 b | (no description set)
    "###);

    // We get a warning if we pass a positional argument that looks like a revset
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: The argument "b" is being interpreted as a path. To specify a revset, pass -r "b" instead.
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl 1c4e5596 c | (no description set)
    Parent commit      : kkmpptxz 16cc94b4 b | (no description set)
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

    // If we specify only a non-existent file, then the move still succeeds and
    // creates unchanged commits.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["squash", "--from", "c", "nonexistent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv b670567d d | (no description set)
    Parent commit      : qpvuntsm 3db0a2f5 a | (no description set)
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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash", "--from=b|c", "--into=d"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    New conflicts appeared in these commits:
      yqosqzyt d5401742 d | (conflict) (no description set)
    To resolve the conflicts, start by updating to it:
      jj new yqosqzytrlsw
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: kpqxywon cc9f4cad f | (no description set)
    Parent commit      : yostqsxw 9f25b62d e | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  cc9f4cad1a29 f
    ◉    9f25b62ddffc e
    ├─╮
    ◉ │  d54017421f3f d
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
      yqosqzyt 13468b54 d | (conflict) (no description set)
    To resolve the conflicts, start by updating to it:
      jj new yqosqzytrlsw
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: kpqxywon 8aaa7910 f | (no description set)
    Parent commit      : yostqsxw 5aad25ea e | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  8aaa79109163 f
    ◉      5aad25eae5aa e
    ├─┬─╮
    │ │ ◉  ba60ddff2d41 b
    │ ◉ │  8ef5a315bf7d c
    │ ├─╯
    ◉ │  13468b546ba3 d
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

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ branches"#;
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

fn get_description(test_env: &TestEnvironment, repo_path: &Path, rev: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "--no-graph", "-T", "description", "-r", rev],
    )
}
