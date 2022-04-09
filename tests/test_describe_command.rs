// Copyright 2022 Google LLC
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

    // Set a description using `-m` flag
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description from CLI"]);
    insta::assert_snapshot!(stdout, @"Working copy now at: 7e0db3b0ad17 description from CLI
");

    // Test making no changes
    std::fs::write(&edit_script, "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"Working copy now at: 45bfa10db64d description from CLI
");

    // Set a description in editor
    std::fs::write(&edit_script, "write\ndescription from editor").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"Working copy now at: f2ce8f1ad8fa description from editor
");

    // Lines in editor starting with "JJ: " are ignored
    std::fs::write(&edit_script, "write\nJJ: ignored\ndescription among comment\nJJ: ignored").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe"]);
    insta::assert_snapshot!(stdout, @"Working copy now at: 95664f6316ae description among comment
");
}
