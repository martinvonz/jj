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

use std::path::{Path, PathBuf};

use crate::common::TestEnvironment;

#[test]
fn test_unsquash() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  382c9bad7d42 c
    ◉  d5d59175b481 b
    ◉  184ddbcce5a9 a
    ◉  000000000000
    "###);

    // Unsquashes into the working copy from its parent by default
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl 9177132c c | (no description set)
    Parent commit      : qpvuntsm 184ddbcc a b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  9177132cfbb9 c
    ◉  184ddbcce5a9 a b
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Can unsquash into a given commit from its parent
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash", "-r", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl b353b29c c | (no description set)
    Parent commit      : kkmpptxz 27772b15 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  b353b29c423d c
    ◉  27772b156771 b
    ◉  000000000000 a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
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
    @    b780e7469252 e
    ├─╮
    │ ◉  f86e2b3af3e3 d
    ◉ │  382c9bad7d42 c
    ├─╯
    ◉  d5d59175b481 b
    ◉  184ddbcce5a9 a
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
    Working copy now at: pzsxstzt bd05eb69 merge
    Parent commit      : mzvwutvl 382c9bad c e?? | (no description set)
    Parent commit      : xznxytkn f86e2b3a d e?? | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    bd05eb698d1e
    ├─╮
    │ ◉  f86e2b3af3e3 d e??
    ◉ │  382c9bad7d42 c e??
    ├─╯
    ◉  d5d59175b481 b
    ◉  184ddbcce5a9 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_unsquash_partial() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  a0b1a272ebc4 c
    ◉  d117da276a0f b
    ◉  54d3c1c0e9fd a
    ◉  000000000000
    "###);

    // If we don't make any changes in the diff-editor, the whole change is moved
    // from the parent
    let edit_script = test_env.set_up_fake_diff_editor();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: mzvwutvl 8802263d c | (no description set)
    Parent commit      : kkmpptxz 5bd83140 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  8802263dbd92 c
    ◉  5bd83140fd47 b
    ◉  c93de9257191 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);

    // Can unsquash only some changes in interactive mode
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    std::fs::write(edit_script, "reset file1").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["unsquash", "-i"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl a896ffde c | (no description set)
    Parent commit      : kkmpptxz 904111b4 b | (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  a896ffdebb85 c
    ◉  904111b4d3c4 b
    ◉  54d3c1c0e9fd a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2", "-r", "c"]);
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
    Working copy now at: mzvwutvl aaca9268 c | (no description set)
    Parent commit      : kkmpptxz fe8eb117 b | (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2", "-r", "c"]);
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

#[test]
fn test_unsquash_description_editor_avoids_unc() {
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
    test_env.jj_cmd_ok(&repo_path, &["unsquash"]);

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
