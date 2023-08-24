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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_describe() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let edit_script = test_env.set_up_fake_editor();

    // Set an initial description using `-m` flag
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description from CLI"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: qpvuntsm cf3e8673 (empty) description from CLI
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Check that comment lines are commented and previous description is present
    // uncommented
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

    // Set a custom comment prefix
    // https://github.com/rust-lang/rust-clippy/issues/11068
    #[allow(clippy::needless_raw_string_hashes)]
    test_env.add_config(r##"ui.editor-comment-prefix = "#""##);

    // Check that the custom prefix is being used
    std::fs::write(&edit_script, "dump editor0").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    description from CLI

    # Lines starting with "#" (like this one) will be removed.
    "###);

    // Set an empty prefix
    #[allow(clippy::needless_raw_string_hashes)]
    // https://github.com/rust-lang/rust-clippy/issues/11068
    test_env.add_config(r##"ui.editor-comment-prefix = """##);

    // Check that we fall back to the default
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
}
