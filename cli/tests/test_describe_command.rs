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

use indoc::indoc;

use crate::common::get_stderr_string;
use crate::common::TestEnvironment;

#[test]
fn test_describe() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();

    // Set a description using `-m` flag
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description from CLI"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 95979928 (empty) description from CLI
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Set the same description using `-m` flag, but with explicit newline
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description from CLI\n"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // Check that the text file gets initialized with the current description and
    // make no changes
    std::fs::write(&edit_script, "dump editor0").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    description from CLI

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);

    // Set a description in editor
    std::fs::write(&edit_script, "write\ndescription from editor").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 94fcb906 (empty) description from editor
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Lines in editor starting with "JJ: " are ignored
    std::fs::write(
        &edit_script,
        "write\nJJ: ignored\ndescription among comment\nJJ: ignored",
    )
    .unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 7a348923 (empty) description among comment
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Multi-line description
    std::fs::write(&edit_script, "write\nline1\nline2\n\nline4\n\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 749361b5 (empty) line1
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r@", "-Tdescription"]);
    insta::assert_snapshot!(stdout, @r###"
    line1
    line2

    line4
    "###);

    // Multi-line description again with CRLF, which should make no changes
    std::fs::write(&edit_script, "write\nline1\r\nline2\r\n\r\nline4\r\n\r\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // Multi-line description starting with newlines
    std::fs::write(&edit_script, "write\n\n\nline1\nline2").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm dc44dbee (empty) line1
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r@", "-Tdescription"]);
    insta::assert_snapshot!(stdout, @r#"
    line1
    line2
    "#);

    // Clear description
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe", "-m", ""]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 6296963b (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
    std::fs::write(&edit_script, "write\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // Fails if the editor fails
    std::fs::write(&edit_script, "fail").unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe"]);
    assert!(stderr.contains("exited with an error"));

    // Fails if the editor doesn't exist
    std::fs::write(&edit_script, "").unwrap();
    let assert = test_env
        .jj_cmd(&repo_path, &["describe"])
        .env("EDITOR", "this-editor-does-not-exist")
        .assert()
        .failure();
    assert!(get_stderr_string(&assert).contains("Failed to run"));

    // `$VISUAL` overrides `$EDITOR`
    let assert = test_env
        .jj_cmd(&repo_path, &["describe"])
        .env("VISUAL", "bad-editor-from-visual-env")
        .env("EDITOR", "bad-editor-from-editor-env")
        .assert()
        .failure();
    assert!(get_stderr_string(&assert).contains("bad-editor-from-visual-env"));

    // `ui.editor` config overrides `$VISUAL`
    test_env.add_config(r#"ui.editor = "bad-editor-from-config""#);
    let assert = test_env
        .jj_cmd(&repo_path, &["describe"])
        .env("VISUAL", "bad-editor-from-visual-env")
        .assert()
        .failure();
    assert!(get_stderr_string(&assert).contains("bad-editor-from-config"));

    // `$JJ_EDITOR` overrides `ui.editor` config
    let assert = test_env
        .jj_cmd(&repo_path, &["describe"])
        .env("JJ_EDITOR", "bad-jj-editor-from-jj-editor-env")
        .assert()
        .failure();
    assert!(get_stderr_string(&assert).contains("bad-jj-editor-from-jj-editor-env"));
}

#[test]
fn test_describe_multiple_commits() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();

    // Initial setup
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  c6349e79bbfd
    ○  65b6b74e0897
    ○  230dd059e1b0
    ◆  000000000000
    "###);

    // Set the description of multiple commits using `-m` flag
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "@", "@--", "-m", "description from CLI"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Updated 2 commits
    Rebased 1 descendant commits
    Working copy now at: kkmpptxz 41659b84 (empty) description from CLI
    Parent commit      : rlvkpnrz 8d650510 (empty) (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  41659b846096 description from CLI
    ○  8d650510daad
    ○  a42f5755e688 description from CLI
    ◆  000000000000
    "###);

    // Check that the text file gets initialized with the current description of
    // each commit and doesn't update commits if no changes are made.
    // Commit descriptions are edited in topological order
    std::fs::write(&edit_script, "dump editor0").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe", "@", "@-"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    JJ: Enter or edit commit descriptions after the `JJ: describe` lines.
    JJ: Warning:
    JJ: - The text you enter will be lost on a syntax error.
    JJ: - The syntax of the separator lines may change in the future.

    JJ: describe 8d650510daad -------

    JJ: describe 41659b846096 -------
    description from CLI

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);

    // Set the description of multiple commits in the editor
    std::fs::write(
        &edit_script,
        indoc! {"
            write
            JJ: Enter or edit commit descriptions after the `JJ: describe` lines.

            JJ: More header tests. Library tests verify parsing in other situations.

            JJ: describe 8d650510daad -------
            description from editor of @-

            further commit message of @-

            JJ: describe 41659b846096 -------
            description from editor of @

            further commit message of @

            JJ: Lines starting with \"JJ: \" (like this one) will be removed.
        "},
    )
    .unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe", "@", "@-"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Updated 2 commits
    Working copy now at: kkmpptxz f203494a (empty) description from editor of @
    Parent commit      : rlvkpnrz 0d76a92c (empty) description from editor of @-
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  f203494a4507 description from editor of @
    │
    │  further commit message of @
    ○  0d76a92ca7cc description from editor of @-
    │
    │  further commit message of @-
    ○  a42f5755e688 description from CLI
    ◆  000000000000
    "###);

    // Fails if the edited message has a commit with multiple descriptions
    std::fs::write(
        &edit_script,
        indoc! {"
            write
            JJ: describe 0d76a92ca7cc -------
            first description from editor of @-

            further commit message of @-

            JJ: describe 0d76a92ca7cc -------
            second description from editor of @-

            further commit message of @-

            JJ: describe f203494a4507 -------
            updated description from editor of @

            further commit message of @

            JJ: Lines starting with \"JJ: \" (like this one) will be removed.
        "},
    )
    .unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "@", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The following commits were found in the edited message multiple times: 0d76a92ca7cc
    "###);

    // Fails if the edited message has unexpected commit IDs
    std::fs::write(
        &edit_script,
        indoc! {"
            write
            JJ: describe 000000000000 -------
            unexpected commit ID

            JJ: describe 0d76a92ca7cc -------
            description from editor of @-

            further commit message of @-

            JJ: describe f203494a4507 -------
            description from editor of @

            further commit message of @

            JJ: Lines starting with \"JJ: \" (like this one) will be removed.
        "},
    )
    .unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "@", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The following commits were not being edited, but were found in the edited message: 000000000000
    "###);

    // Fails if the edited message has missing commit messages
    std::fs::write(
        &edit_script,
        indoc! {"
            write
            JJ: describe f203494a4507 -------
            description from editor of @

            further commit message of @

            JJ: Lines starting with \"JJ: \" (like this one) will be removed.
        "},
    )
    .unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "@", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The description for the following commits were not found in the edited message: 0d76a92ca7cc
    "###);

    // Fails if the edited message has a line which does not have any preceding
    // `JJ: describe` headers
    std::fs::write(
        &edit_script,
        indoc! {"
            write
            description from editor of @-

            JJ: describe f203494a4507 -------
            description from editor of @

            JJ: Lines starting with \"JJ: \" (like this one) will be removed.
        "},
    )
    .unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "@", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Found the following line without a commit header: "description from editor of @-"
    "###);

    // Fails if the editor fails
    std::fs::write(&edit_script, "fail").unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "@", "@-"]);
    assert!(stderr.contains("exited with an error"));
}

#[test]
fn test_multiple_message_args() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Set a description using `-m` flag
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "-m",
            "First Paragraph from CLI",
            "-m",
            "Second Paragraph from CLI",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 99a36a50 (empty) First Paragraph from CLI
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r@", "-Tdescription"]);
    insta::assert_snapshot!(stdout, @r###"
    First Paragraph from CLI

    Second Paragraph from CLI
    "###);

    // Set the same description, with existing newlines
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "-m",
            "First Paragraph from CLI\n",
            "-m",
            "Second Paragraph from CLI\n",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // Use an empty -m flag between paragraphs to insert an extra blank line
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "-m",
            "First Paragraph from CLI\n",
            "--message",
            "",
            "-m",
            "Second Paragraph from CLI",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 01ac40b3 (empty) First Paragraph from CLI
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r@", "-Tdescription"]);
    insta::assert_snapshot!(stdout, @r###"
    First Paragraph from CLI


    Second Paragraph from CLI
    "###);
}

#[test]
fn test_describe_default_description() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    test_env.add_config(r#"ui.default-description = "\n\nTESTED=TODO""#);
    let workspace_path = test_env.env_root().join("repo");

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();
    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, ["dump editor"].join("\0")).unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 573b6df5 TESTED=TODO
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r###"


    TESTED=TODO

    JJ: This commit contains the following changes:
    JJ:     A file1
    JJ:     A file2

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);
}

#[test]
fn test_describe_author() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, ["dump editor"].join("\0")).unwrap();

    test_env.add_config(indoc! {r#"
        [template-aliases]
        'format_signature(signature)' = 'signature.name() ++ " " ++ signature.email() ++ " " ++ signature.timestamp()'

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
    "#});
    let get_signatures = || {
        test_env.jj_cmd_success(
            &repo_path,
            &[
                "log",
                "-r..",
                "-T",
                r#"format_signature(author) ++ "\n" ++ format_signature(committer)"#,
            ],
        )
    };

    // Initial setup
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    insta::assert_snapshot!(get_signatures(), @r###"
    @  Test User test.user@example.com 2001-02-03 04:05:10.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:10.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:09.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:09.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:08.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:08.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:07.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:07.000 +07:00
    ~
    "###);

    // Change the author for the latest commit (the committer is always reset)
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "--author",
            "Super Seeder <super.seeder@example.com>",
        ],
    );
    insta::assert_snapshot!(get_signatures(), @r#"
    @  Super Seeder super.seeder@example.com 2001-02-03 04:05:12.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:12.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:09.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:09.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:08.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:08.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:07.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:07.000 +07:00
    ~
    "#);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r#"

    JJ: Author: Super Seeder <super.seeder@example.com> (2001-02-03 08:05:10)
    JJ: Committer: Test User <test.user@example.com> (2001-02-03 08:05:12)

    JJ: 0 files changed, 0 insertions(+), 0 deletions(-)

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "#);

    // Change the author for multiple commits (the committer is always reset)
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "@---",
            "@-",
            "--no-edit",
            "--author",
            "Super Seeder <super.seeder@example.com>",
        ],
    );
    insta::assert_snapshot!(get_signatures(), @r#"
    @  Super Seeder super.seeder@example.com 2001-02-03 04:05:12.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ○  Super Seeder super.seeder@example.com 2001-02-03 04:05:14.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ○  Super Seeder super.seeder@example.com 2001-02-03 04:05:14.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ~
    "#);

    // Reset the author for the latest commit (the committer is always reset)
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "--config-toml",
            r#"user.name = "Ove Ridder"
            user.email = "ove.ridder@example.com""#,
            "--no-edit",
            "--reset-author",
        ],
    );
    insta::assert_snapshot!(get_signatures(), @r#"
    @  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:16.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:16.000 +07:00
    ○  Super Seeder super.seeder@example.com 2001-02-03 04:05:14.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ○  Super Seeder super.seeder@example.com 2001-02-03 04:05:14.000 +07:00
    │  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    ~
    "#);

    // Reset the author for multiple commits (the committer is always reset)
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "@---",
            "@-",
            "--config-toml",
            r#"user.name = "Ove Ridder"
            user.email = "ove.ridder@example.com""#,
            "--reset-author",
        ],
    );
    insta::assert_snapshot!(get_signatures(), @r#"
    @  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    ○  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    ○  Test User test.user@example.com 2001-02-03 04:05:14.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    ○  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:18.000 +07:00
    ~
    "#);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(), @r#"
    JJ: Enter or edit commit descriptions after the `JJ: describe` lines.
    JJ: Warning:
    JJ: - The text you enter will be lost on a syntax error.
    JJ: - The syntax of the separator lines may change in the future.

    JJ: describe eae86afaa20c -------

    JJ: Author: Ove Ridder <ove.ridder@example.com> (2001-02-03 08:05:18)
    JJ: Committer: Ove Ridder <ove.ridder@example.com> (2001-02-03 08:05:18)

    JJ: 0 files changed, 0 insertions(+), 0 deletions(-)

    JJ: describe ba485659f76a -------

    JJ: Author: Ove Ridder <ove.ridder@example.com> (2001-02-03 08:05:18)
    JJ: Committer: Ove Ridder <ove.ridder@example.com> (2001-02-03 08:05:18)

    JJ: 0 files changed, 0 insertions(+), 0 deletions(-)

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "#);
}

#[test]
fn test_describe_avoids_unc() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    let edit_script = test_env.set_up_fake_editor();

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["describe"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    // While `assert!(!edited_path.starts_with("//?/"))` could work here in most
    // cases, it fails when it is not safe to strip the prefix, such as paths
    // over 260 chars.
    assert_eq!(edited_path, dunce::simplified(&edited_path));
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ description"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}
