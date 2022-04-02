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
fn test_edit() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("file3"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();

    let edit_script = test_env.set_up_fake_diff_editor();

    // Nothing happens if we make no changes
    std::fs::write(&edit_script, "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["edit"]);
    insta::assert_snapshot!(stdout, @"Nothing changed.
");
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    "###);

    // Nothing happens if the diff-editor exits with an error
    std::fs::write(&edit_script, "rm file2\0fail").unwrap();
    let stdout = test_env.jj_cmd_failure(&repo_path, &["edit"]);
    insta::assert_snapshot!(stdout, @"Error: Failed to edit diff: The diff tool exited with a non-zero code
");
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    "###);

    // Can edit changes to individual files
    std::fs::write(&edit_script, "reset file2").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["edit"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 8c79910b5033 
    Working copy now at: 8c79910b5033 
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"R file1
");

    // Changes to a commit are propagated to descendants
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "write file3\nmodified\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["edit", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 472de2debaff 
    Rebased 1 descendant commits
    Working copy now at: 6d19dc1ea106 
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let contents = String::from_utf8(std::fs::read(repo_path.join("file3")).unwrap()).unwrap();
    insta::assert_snapshot!(contents, @"modified
");
}
