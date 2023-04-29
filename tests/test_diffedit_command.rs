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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_diffedit() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("file3"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();

    let edit_script = test_env.set_up_fake_diff_editor();

    // Nothing happens if we make no changes
    std::fs::write(
        &edit_script,
        "files-before file1 file2\0files-after JJ-INSTRUCTIONS file2",
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    "###);

    // Nothing happens if the diff-editor exits with an error
    std::fs::write(&edit_script, "rm file2\0fail").unwrap();
    insta::assert_snapshot!(&test_env.jj_cmd_failure(&repo_path, &["diffedit"]), @r###"
    Error: Failed to edit diff: Tool exited with a non-zero code (run with --verbose to see the exact invocation). Exit code: 1.
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    "###);

    // Can edit changes to individual files
    std::fs::write(&edit_script, "reset file2").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 1930da4a57e9 (no description set)
    Working copy now at: 1930da4a57e9 (no description set)
    Parent commit      : 613028a4693c (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    "###);

    // Changes to a commit are propagated to descendants
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "write file3\nmodified\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created c03ae96780b6 (no description set)
    Rebased 1 descendant commits
    Working copy now at: 2a4dc204a6ab (no description set)
    Parent commit      : c03ae96780b6 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let contents = String::from_utf8(std::fs::read(repo_path.join("file3")).unwrap()).unwrap();
    insta::assert_snapshot!(contents, @r###"
    modified
    "###);

    // Test diffedit --from @--
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    std::fs::write(
        &edit_script,
        "files-before file1\0files-after JJ-INSTRUCTIONS file2 file3\0reset file2",
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit", "--from", "@--"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 15f2c966d508 (no description set)
    Working copy now at: 15f2c966d508 (no description set)
    Parent commit      : 613028a4693c (no description set)
    Added 0 files, modified 0 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    R file2
    "###);
}

#[test]
fn test_diffedit_merge() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["co", "@-"]);
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "@", "b", "-m", "merge"]);
    // Resolve the conflict in file1, but leave the conflict in file2
    std::fs::write(repo_path.join("file1"), "d\n").unwrap();
    std::fs::write(repo_path.join("file3"), "d\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    // Test the setup
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-r", "@-", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    M file1
    A file3
    "###);

    let edit_script = test_env.set_up_fake_diff_editor();

    // Remove file1. The conflict remains in the working copy on top of the merge.
    std::fs::write(
        edit_script,
        "files-before file1\0files-after JJ-INSTRUCTIONS file1 file3\0rm file1",
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created a70eded7af9e merge
    Rebased 1 descendant commits
    Working copy now at: a5f1ce845f74 (no description set)
    Parent commit      : a70eded7af9e merge
    Added 0 files, modified 0 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    A file3
    "###);
    assert!(!repo_path.join("file1").exists());
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    <<<<<<<
    %%%%%%%
    -a
    +c
    +++++++
    b
    >>>>>>>
    "###);
}

#[test]
fn test_diffedit_old_restore_interactive_tests() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    std::fs::write(repo_path.join("file3"), "b\n").unwrap();

    let edit_script = test_env.set_up_fake_diff_editor();

    // Nothing happens if we make no changes
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit", "--from", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    A file3
    "###);

    // Nothing happens if the diff-editor exits with an error
    std::fs::write(&edit_script, "rm file2\0fail").unwrap();
    insta::assert_snapshot!(&test_env.jj_cmd_failure(&repo_path, &["diffedit", "--from", "@-"]), @r###"
    Error: Failed to edit diff: Tool exited with a non-zero code (run with --verbose to see the exact invocation). Exit code: 1.
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    A file3
    "###);

    // Can restore changes to individual files
    std::fs::write(&edit_script, "reset file2\0reset file3").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit", "--from", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created abdbf6271a1c (no description set)
    Working copy now at: abdbf6271a1c (no description set)
    Parent commit      : 2375fa164210 (no description set)
    Added 0 files, modified 1 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    "###);

    // Can make unrelated edits
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "write file3\nunrelated\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diffedit", "--from", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created e31f7f33ad07 (no description set)
    Working copy now at: e31f7f33ad07 (no description set)
    Parent commit      : 2375fa164210 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file1 b/file1
    deleted file mode 100644
    index 7898192261..0000000000
    --- a/file1
    +++ /dev/null
    @@ -1,1 +1,0 @@
    -a
    diff --git a/file2 b/file2
    index 7898192261...6178079822 100644
    --- a/file2
    +++ b/file2
    @@ -1,1 +1,1 @@
    -a
    +b
    diff --git a/file3 b/file3
    new file mode 100644
    index 0000000000..c21c9352f7
    --- /dev/null
    +++ b/file3
    @@ -1,0 +1,1 @@
    +unrelated
    "###);
}
