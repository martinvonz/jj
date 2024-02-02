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

use crate::common::{get_stderr_string, TestEnvironment};

#[test]
fn test_describe() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();

    // Set a description using `-m` flag
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description from CLI"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm cf3e8673 (empty) description from CLI
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
    Working copy now at: qpvuntsm 100943ae (empty) description from editor
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
    Working copy now at: qpvuntsm ccefa58b (empty) description among comment
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Multi-line description
    std::fs::write(&edit_script, "write\nline1\nline2\n\nline4\n\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm e932ba42 (empty) line1
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
    Working copy now at: qpvuntsm 13f903c1 (empty) line1
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
    Working copy now at: qpvuntsm 3196270d (empty) (no description set)
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
fn test_multiple_message_args() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    Working copy now at: qpvuntsm bdee9366 (empty) First Paragraph from CLI
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
    Working copy now at: qpvuntsm a7506fe0 (empty) First Paragraph from CLI
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    test_env.add_config(r#"ui.default-description = "\n\nTESTED=TODO""#);
    let workspace_path = test_env.env_root().join("repo");

    std::fs::write(workspace_path.join("file1"), "foo\n").unwrap();
    std::fs::write(workspace_path.join("file2"), "bar\n").unwrap();
    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, ["dump editor"].join("\0")).unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 7e780ba8 TESTED=TODO
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
    assert_eq!(
        std::fs::read_to_string(test_env.env_root().join("editor")).unwrap(),
        r#"

TESTED=TODO
JJ: This commit contains the following changes:
JJ:     A file1
JJ:     A file2

JJ: Lines starting with "JJ: " (like this one) will be removed.
"#
    );
}

#[test]
fn test_describe_author() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
            "describe",
            "--config-toml",
            r#"user.name = "Ove Ridder"
            user.email = "ove.ridder@example.com""#,
            "--no-edit",
            "--reset-author",
        ],
    );
    insta::assert_snapshot!(get_signatures(), @r###"
    @  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:09.000 +07:00
    │  Ove Ridder ove.ridder@example.com 2001-02-03 04:05:09.000 +07:00
    ~
    "###);
}
