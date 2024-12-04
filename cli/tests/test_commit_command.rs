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
fn test_commit_with_description_from_cli() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // Description applies to the current working-copy (not the new one)
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  e8ea92a8b6b3
    ○  fa15625b4a98 first
    ◆  000000000000
    "###);
}

#[test]
fn test_commit_with_editor() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    // Check that the text file gets initialized with the current description and
    // set a new one
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=initial"]);
    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(&edit_script, ["dump editor0", "write\nmodified"].join("\0")).unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["commit"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  a57b2c95fb75
    ○  159271101e05 modified
    ◆  000000000000
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    initial

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);

    // Check that the editor content includes diff summary
    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=add files"]);
    std::fs::write(&edit_script, "dump editor1").unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["commit"]);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor1")).unwrap(), @r###"
    add files

    JJ: This commit contains the following changes:
    JJ:     A file1
    JJ:     A file2

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);
}

#[test]
fn test_commit_with_editor_avoids_unc() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    let edit_script = test_env.set_up_fake_editor();

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["commit"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    // While `assert!(!edited_path.starts_with("//?/"))` could work here in most
    // cases, it fails when it is not safe to strip the prefix, such as paths
    // over 260 chars.
    assert_eq!(edited_path, dunce::simplified(&edited_path));
}

#[test]
fn test_commit_interactive() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=add files"]);
    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, ["dump editor"].join("\0")).unwrap();

    let diff_editor = test_env.set_up_fake_diff_editor();
    let diff_script = ["rm file2", "dump JJ-INSTRUCTIONS instrs"].join("\0");
    std::fs::write(diff_editor, diff_script).unwrap();

    // Create a commit interactively and select only file1
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-i"]);

    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("instrs")).unwrap(), @r###"
    You are splitting the working-copy commit: qpvuntsm 4219467e add files

    The diff initially shows all changes. Adjust the right side until it shows the
    contents you want for the first commit. The remainder will be included in the
    new working-copy commit.
    "###);

    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"
    add files

    JJ: This commit contains the following changes:
    JJ:     A file1

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);

    // Try again with --tool=<name>, which implies --interactive
    test_env.jj_cmd_ok(&workspace_path, &["undo"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &[
            "commit",
            "--config-toml=ui.diff-editor='false'",
            "--tool=fake-diff-editor",
        ],
    );

    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"
    add files

    JJ: This commit contains the following changes:
    JJ:     A file1

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);
}

#[test]
fn test_commit_with_default_description() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    test_env.add_config(r#"ui.default-description = "\n\nTESTED=TODO""#);
    let workspace_path = test_env.env_root().join("repo");

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();
    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, ["dump editor"].join("\0")).unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["commit"]);

    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  c65242099289
    ○  573b6df51aea TESTED=TODO
    ◆  000000000000
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"
    TESTED=TODO

    JJ: This commit contains the following changes:
    JJ:     A file1
    JJ:     A file2

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);
}

#[test]
fn test_commit_with_description_template() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    test_env.add_config(
        r#"
        [templates]
        draft_commit_description = '''
        concat(
          description,
          "\n",
          indent(
            "JJ: ",
            concat(
              "Author: " ++ format_detailed_signature(author) ++ "\n",
              "Committer: " ++ format_detailed_signature(committer)  ++ "\n",
              "\n",
              diff.stat(76),
            ),
          ),
        )
        '''
        "#,
    );
    let workspace_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, ["dump editor"].join("\0")).unwrap();

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();
    std::fs::write(workspace_path.join("file3"), "foobar\n").unwrap();

    // Only file1 should be included in the diff
    test_env.jj_cmd_ok(&workspace_path, &["commit", "file1"]);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"
    JJ: Author: Test User <test.user@example.com> (2001-02-03 08:05:08)
    JJ: Committer: Test User <test.user@example.com> (2001-02-03 08:05:08)

    JJ: file1 | 1 +
    JJ: 1 file changed, 1 insertion(+), 0 deletions(-)

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);

    // Only file2 with modified author should be included in the diff
    test_env.jj_cmd_ok(
        &workspace_path,
        &[
            "commit",
            "--author",
            "Another User <another.user@example.com>",
            "file2",
        ],
    );
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"
    JJ: Author: Another User <another.user@example.com> (2001-02-03 08:05:08)
    JJ: Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

    JJ: file2 | 1 +
    JJ: 1 file changed, 1 insertion(+), 0 deletions(-)

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);

    // Timestamp after the reset should be available to the template
    test_env.jj_cmd_ok(&workspace_path, &["commit", "--reset-author"]);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"
    JJ: Author: Test User <test.user@example.com> (2001-02-03 08:05:10)
    JJ: Committer: Test User <test.user@example.com> (2001-02-03 08:05:10)

    JJ: file3 | 1 +
    JJ: 1 file changed, 1 insertion(+), 0 deletions(-)

    JJ: Lines starting with "JJ:" (like this one) will be removed.
    "###);
}

#[test]
fn test_commit_without_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&workspace_path, &["workspace", "forget"]);
    let stderr = test_env.jj_cmd_failure(&workspace_path, &["commit", "-m=first"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: This command requires a working copy
    "###);
}

#[test]
fn test_commit_paths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();

    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first", "file1"]);
    let stdout = test_env.jj_cmd_success(&workspace_path, &["diff", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Added regular file file1:
            1: foo
    "###);

    let stdout = test_env.jj_cmd_success(&workspace_path, &["diff"]);
    insta::assert_snapshot!(stdout, @"
    Added regular file file2:
            1: bar
    ");
}

#[test]
fn test_commit_paths_warning() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=first", "file3"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: The given paths do not match any file: file3
    Working copy now at: rlvkpnrz d1872100 (no description set)
    Parent commit      : qpvuntsm fa15625b (empty) first
    "###);
    insta::assert_snapshot!(stdout, @"");

    let stdout = test_env.jj_cmd_success(&workspace_path, &["diff"]);
    insta::assert_snapshot!(stdout, @r###"
    Added regular file file1:
            1: foo
    Added regular file file2:
            1: bar
    "###);
}

#[test]
fn test_commit_reset_author() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r#"[template-aliases]
'format_signature(signature)' = 'signature.name() ++ " " ++ signature.email() ++ " " ++ signature.timestamp()'"#,
    );
    let get_signatures = || {
        test_env.jj_cmd_success(
            &repo_path,
            &[
                "log",
                "-r@",
                "-T",
                r#"format_signature(author) ++ "\n" ++ format_signature(committer)"#,
            ],
        )
    };
    insta::assert_snapshot!(get_signatures(), @r###"
    @  Test User test.user@example.com 2001-02-03 04:05:07.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:07.000 +07:00
    ~
    "###);

    // Reset the author (the committer is always reset)
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "commit",
            "--config-toml",
            r#"user.name = "Ove Ridder"
            user.email = "ove.ridder@example.com""#,
            "--reset-author",
            "-m1",
        ],
    );
    insta::assert_snapshot!(get_signatures(), @r###"
    @  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:09.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:09.000 +07:00
    ~
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
