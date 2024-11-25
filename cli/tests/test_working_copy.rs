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
