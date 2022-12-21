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
    "###);

    // Set the same description using `-m` flag, but with explicit newline
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description from CLI\n"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);

    // Check that the text file gets initialized with the current description and
    // make no changes
    std::fs::write(
        &edit_script,
        r#"expect
description from CLI

JJ: Lines starting with "JJ: " (like this one) will be removed.
"#,
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);

    // Set a description in editor
    std::fs::write(&edit_script, "write\ndescription from editor").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: bfdd972f9349 description from editor
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
    test_env.add_config(
        br#"[ui]
    editor = "bad-editor-from-config""#,
    );
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
