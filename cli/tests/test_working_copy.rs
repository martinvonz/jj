// Copyright 2023 The Jujutsu Authors
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

use indoc::indoc;
use regex::Regex;

use crate::common::TestEnvironment;

#[test]
fn test_snapshot_large_file() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // test a small file using raw-integer-literal syntax, which is interpreted
    // in bytes
    test_env.add_config(r#"snapshot.max-new-file-size = 10"#);
    std::fs::write(repo_path.join("large"), "a lot of text").unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to snapshot the working copy
    The file '$TEST_ENV/repo/large' is too large to be snapshotted: it is 3 bytes too large; the maximum size allowed is 10 bytes (10.0B).
    Hint: This is to prevent large files from being added by accident. You can fix this error by:
      - Adding the file to `.gitignore`
      - Run `jj config set --repo snapshot.max-new-file-size 13`
        This will increase the maximum file size allowed for new files, in this repository only.
      - Run `jj --config-toml 'snapshot.max-new-file-size=13' st`
        This will increase the maximum file size allowed for new files, for this command only.
    "###);

    // test with a larger file using 'KB' human-readable syntax
    test_env.add_config(r#"snapshot.max-new-file-size = "10KB""#);
    let big_string = vec![0; 1024 * 11];
    std::fs::write(repo_path.join("large"), big_string).unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to snapshot the working copy
    The file '$TEST_ENV/repo/large' is too large to be snapshotted: it is 1024 bytes too large; the maximum size allowed is 10240 bytes (10.0KiB).
    Hint: This is to prevent large files from being added by accident. You can fix this error by:
      - Adding the file to `.gitignore`
      - Run `jj config set --repo snapshot.max-new-file-size 11264`
        This will increase the maximum file size allowed for new files, in this repository only.
      - Run `jj --config-toml 'snapshot.max-new-file-size=11264' st`
        This will increase the maximum file size allowed for new files, for this command only.
    "###);

    // test invalid configuration
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &[
            "file",
            "list",
            "--config-toml=snapshot.max-new-file-size = []",
        ],
    );
    insta::assert_snapshot!(stderr, @r"
    Config error: Invalid type or value for snapshot.max-new-file-size
    Caused by: Expected a positive integer or a string in '<number><unit>' form
    For help, see https://martinvonz.github.io/jj/latest/config/.
    ");

    // No error if we disable auto-tracking of the path
    test_env.add_config(r#"snapshot.auto-track = 'none()'"#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @"");
}

#[test]
fn test_materialize_and_snapshot_different_conflict_markers() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Configure to use Git-style conflict markers
    test_env.add_config(r#"ui.conflict-marker-style = "git""#);

    // Create a conflict in the working copy
    let conflict_file = repo_path.join("file");
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            line 2
            line 3
        "},
    )
    .unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "base"]);
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            line 2 - a
            line 3
        "},
    )
    .unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "side-a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "description(base)", "-m", "side-b"]);
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            line 2 - b
            line 3 - b
        "},
    )
    .unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "description(side-a)", "description(side-b)"],
    );

    // File should have Git-style conflict markers
    insta::assert_snapshot!(std::fs::read_to_string(&conflict_file).unwrap(), @r##"
    line 1
    <<<<<<< Side #1 (Conflict 1 of 1)
    line 2 - a
    line 3
    ||||||| Base
    line 2
    line 3
    =======
    line 2 - b
    line 3 - b
    >>>>>>> Side #2 (Conflict 1 of 1 ends)
    "##);

    // Configure to use JJ-style "snapshot" conflict markers
    test_env.add_config(r#"ui.conflict-marker-style = "snapshot""#);

    // Update the conflict, still using Git-style conflict markers
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            <<<<<<<
            line 2 - a
            line 3 - a
            |||||||
            line 2
            line 3
            =======
            line 2 - b
            line 3 - b
            >>>>>>>
        "},
    )
    .unwrap();

    // Git-style markers should be parsed, then rendered with new config
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "--git"]), @r##"
    diff --git a/file b/file
    --- a/file
    +++ b/file
    @@ -2,7 +2,7 @@
     <<<<<<< Conflict 1 of 1
     +++++++ Contents of side #1
     line 2 - a
    -line 3
    +line 3 - a
     ------- Contents of base
     line 2
     line 3
    "##);
}

#[test]
fn test_conflict_marker_length_stored_in_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Create a conflict in the working copy with long markers on one side
    let conflict_file = repo_path.join("file");
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            line 2
            line 3
        "},
    )
    .unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "base"]);
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            line 2 - left
            line 3 - left
        "},
    )
    .unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "side-a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "description(base)", "-m", "side-b"]);
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            ======= fake marker
            line 2 - right
            ======= fake marker
            line 3
        "},
    )
    .unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "description(side-a)", "description(side-b)"],
    );

    // File should be materialized with long conflict markers
    insta::assert_snapshot!(std::fs::read_to_string(&conflict_file).unwrap(), @r##"
    line 1
    <<<<<<<<<<< Conflict 1 of 1
    %%%%%%%%%%% Changes from base to side #1
    -line 2
    -line 3
    +line 2 - left
    +line 3 - left
    +++++++++++ Contents of side #2
    ======= fake marker
    line 2 - right
    ======= fake marker
    line 3
    >>>>>>>>>>> Conflict 1 of 1 ends
    "##);

    // The timestamps in the `jj debug local-working-copy` output change, so we want
    // to remove them before asserting the snapshot
    let timestamp_regex = Regex::new(r"\b\d{10,}\b").unwrap();
    // On Windows, executable is always `()`, but on Unix-like systems, it's `true`
    // or `false`, so we want to remove it from the output as well
    let executable_regex = Regex::new("executable: [^ ]+").unwrap();

    let redact_output = |output: &str| {
        let output = timestamp_regex.replace_all(output, "<timestamp>");
        let output = executable_regex.replace_all(&output, "<executable>");
        output.into_owned()
    };

    // Working copy should contain conflict marker length
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "local-working-copy"]);
    insta::assert_snapshot!(redact_output(&stdout), @r#"
    Current operation: OperationId("da133d2605b63b84c53b512007b32bd5822e4821d7f8ca69b03a0bbd702cd61fad7857e430e911011aaecf3bf6942e81a95180792c7e0056af18bc956ee834a4")
    Current tree: Merge(Conflicted([TreeId("381273b50cf73f8c81b3f1502ee89e9bbd6c1518"), TreeId("771f3d31c4588ea40a8864b2a981749888e596c2"), TreeId("f56b8223da0dab22b03b8323ced4946329aeb4e0")]))
    Normal { <executable> }           249 <timestamp> Some(MaterializedConflictData { conflict_marker_len: 11 }) "file"
    "#);

    // Update the conflict with more fake markers, and it should still parse
    // correctly (the markers should be ignored)
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            <<<<<<<<<<< Conflict 1 of 1
            %%%%%%%%%%% Changes from base to side #1
            -line 2
            -line 3
            +line 2 - left
            +line 3 - left
            +++++++++++ Contents of side #2
            <<<<<<< fake marker
            ||||||| fake marker
            line 2 - right
            ======= fake marker
            line 3
            >>>>>>> fake marker
            >>>>>>>>>>> Conflict 1 of 1 ends
        "},
    )
    .unwrap();

    // The file should still be conflicted, and the new content should be saved
    let stdout = test_env.jj_cmd_success(&repo_path, &["st"]);
    insta::assert_snapshot!(stdout, @r#"
    Working copy changes:
    M file
    There are unresolved conflicts at these paths:
    file    2-sided conflict
    Working copy : mzvwutvl b7dadc87 (conflict) (no description set)
    Parent commit: rlvkpnrz ce613b49 side-a
    Parent commit: zsuskuln 7b2b03ab side-b
    "#);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "--git"]), @r##"
    diff --git a/file b/file
    --- a/file
    +++ b/file
    @@ -6,8 +6,10 @@
     +line 2 - left
     +line 3 - left
     +++++++++++ Contents of side #2
    -======= fake marker
    +<<<<<<< fake marker
    +||||||| fake marker
     line 2 - right
     ======= fake marker
     line 3
    +>>>>>>> fake marker
     >>>>>>>>>>> Conflict 1 of 1 ends
    "##);

    // Working copy should still contain conflict marker length
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "local-working-copy"]);
    insta::assert_snapshot!(redact_output(&stdout), @r#"
    Current operation: OperationId("65b1b6a0da226e45694fda78d85efa5397176204b166f107b10c8ac0dcecfcfa9346b59317d8b572711666a3e5f168bcb561c278095a83363885911e246b2230")
    Current tree: Merge(Conflicted([TreeId("381273b50cf73f8c81b3f1502ee89e9bbd6c1518"), TreeId("771f3d31c4588ea40a8864b2a981749888e596c2"), TreeId("3329c18c95f7b7a55c278c2259e9c4ce711fae59")]))
    Normal { <executable> }           289 <timestamp> Some(MaterializedConflictData { conflict_marker_len: 11 }) "file"
    "#);

    // Resolve the conflict
    std::fs::write(
        &conflict_file,
        indoc! {"
            line 1
            <<<<<<< fake marker
            ||||||| fake marker
            line 2 - left
            line 2 - right
            ======= fake marker
            line 3 - left
            >>>>>>> fake marker
        "},
    )
    .unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["st"]);
    insta::assert_snapshot!(stdout, @r#"
    Working copy changes:
    M file
    Working copy : mzvwutvl 1aefd866 (no description set)
    Parent commit: rlvkpnrz ce613b49 side-a
    Parent commit: zsuskuln 7b2b03ab side-b
    "#);

    // When the file is resolved, the conflict marker length is removed from the
    // working copy
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "local-working-copy"]);
    insta::assert_snapshot!(redact_output(&stdout), @r#"
    Current operation: OperationId("6dc38b23e076d05a7c80327559e6de48d2fbc0811b06e9319bdbbff392bc991385e1ecbc378613101ba862e07dad1e6703247c5239a5a672a4761411815fe9fa")
    Current tree: Merge(Resolved(TreeId("6120567b3cb2472d549753ed3e4b84183d52a650")))
    Normal { <executable> }           130 <timestamp> None "file"
    "#);
}
