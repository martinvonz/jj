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
    warning: The argument "b" is being interpreted as a path. To specify a revset, pass -r "b" instead.
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl 1c4e5596 c | (no description set)
    Parent commit      : kkmpptxz 16cc94b4 b | (no description set)
    "###);
    insta::assert_snapshot!(stdout, @"");
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

    JJ: Description from the source commit:
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
