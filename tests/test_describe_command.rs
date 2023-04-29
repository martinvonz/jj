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

pub mod common;

#[test]
fn test_describe() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();

    // Set a description using `-m` flag
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description from CLI"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: cf3e86731c67 description from CLI
    Parent commit      : 000000000000 (no description set)
    "###);

    // Set the same description using `-m` flag, but with explicit newline
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description from CLI\n"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);

    // Check that the text file gets initialized with the current description and
    // make no changes
    std::fs::write(&edit_script, "dump editor0").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    description from CLI

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);

    // Set a description in editor
    std::fs::write(&edit_script, "write\ndescription from editor").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 100943aeee3f description from editor
    Parent commit      : 000000000000 (no description set)
    "###);

    // Lines in editor starting with "JJ: " are ignored
    std::fs::write(
        &edit_script,
        "write\nJJ: ignored\ndescription among comment\nJJ: ignored",
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: ccefa58bef47 description among comment
    Parent commit      : 000000000000 (no description set)
    "###);

    // Multi-line description
    std::fs::write(&edit_script, "write\nline1\nline2\n\nline4\n\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: e932ba42cef0 line1
    Parent commit      : 000000000000 (no description set)
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
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);

    // Clear description
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", ""]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: d6957294acdc (no description set)
    Parent commit      : 000000000000 (no description set)
    "###);
    std::fs::write(&edit_script, "write\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
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
fn test_describe_author() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_success(
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
