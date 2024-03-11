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
fn test_unsquash() {
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

    // Unsquashes into the working copy from its parent by default
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl 1b10d78f c | (no description set)
    Parent commit      : qpvuntsm 90aeefd0 a b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  1b10d78f6136 c
    ◉  90aeefd03044 a b
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Can unsquash into a given commit from its parent
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash", "-r", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl 45b8b3dd c | (no description set)
    Parent commit      : kkmpptxz 9146bcc8 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  45b8b3ddc25a c
    ◉  9146bcc8d996 b
    ◉  000000000000 a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Cannot unsquash into a merge commit (because it's unclear which parent it
    // should come from)
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "d"]);
    std::fs::write(repo_path.join("file2"), "d\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "merge", "c", "d"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "e"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    1f8f152ff48e e
    ├─╮
    │ ◉  5658521e0f8b d
    ◉ │  90fe0a96fc90 c
    ├─╯
    ◉  fa5efbdf533c b
    ◉  90aeefd03044 a
    ◉  000000000000
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot unsquash merge commits
    "###);

    // Can unsquash from a merge commit
    test_env.jj_cmd_ok(&repo_path, &["new", "e"]);
    std::fs::write(repo_path.join("file1"), "e\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: pzsxstzt 3217340c merge
    Parent commit      : mzvwutvl 90fe0a96 c e?? | (no description set)
    Parent commit      : xznxytkn 5658521e d e?? | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    3217340cb761
    ├─╮
    │ ◉  5658521e0f8b d e??
    ◉ │  90fe0a96fc90 c e??
    ├─╯
    ◉  fa5efbdf533c b
    ◉  90aeefd03044 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_unsquash_partial() {
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
    // from the parent
    let edit_script = test_env.set_up_fake_diff_editor();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl 37c961d0 c | (no description set)
    Parent commit      : kkmpptxz 000af220 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  37c961d0d1e2 c
    ◉  000af22057b9 b
    ◉  ee67504598b6 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);

    // Can unsquash only some changes in interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    std::fs::write(edit_script, "reset file1").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash", "-i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl a8e8fded c | (no description set)
    Parent commit      : kkmpptxz 46cc0667 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  a8e8fded1021 c
    ◉  46cc06672a99 b
    ◉  47a1e795d146 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Try again with --tool=<name>, which implies --interactive
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "unsquash",
            "--config-toml=ui.diff-editor='false'",
            "--tool=fake-diff-editor",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl 1c82d27c c | (no description set)
    Parent commit      : kkmpptxz b9d23fd8 b | (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ branches"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}

#[test]
fn test_unsquash_description() {
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
    test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @"");

    // If the destination's description is empty and the source's description is
    // non-empty, the resulting description is from the source
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "@-", "-m", "source"]);
    test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @r###"
    source
    "###);

    // If the destination description is non-empty and the source's description is
    // empty, the resulting description is from the destination
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "destination"]);
    test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @r###"
    destination
    "###);

    // If both descriptions were non-empty, we get asked for a combined description
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "@-", "-m", "source"]);
    std::fs::write(&edit_script, "dump editor0").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @r###"
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
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@-"), @r###"
    source
    "###);
    insta::assert_snapshot!(get_description(&test_env, &repo_path, "@"), @r###"
    destination
    "###);
}

fn get_description(test_env: &TestEnvironment, repo_path: &Path, rev: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "--no-graph", "-T", "description", "-r", rev],
    )
}
