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

use std::path::Path;

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_unsquash() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 90fe0a96fc90 c
    o fa5efbdf533c b
    o 90aeefd03044 a
    o 000000000000 
    "###);

    // Unsquashes into the working copy from its parent by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 1b10d78f6136 (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 1b10d78f6136 c
    o 90aeefd03044 a b
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Can unsquash into a given commit from its parent
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["unsquash", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: 45b8b3ddc25a (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 45b8b3ddc25a c
    o 9146bcc8d996 b
    o 000000000000 a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Cannot unsquash into a merge commit (because it's unclear which parent it
    // should come from)
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    test_env.jj_cmd_success(&repo_path, &["co", "b"]);
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "d"]);
    std::fs::write(repo_path.join("file2"), "d\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["merge", "-m", "merge", "c", "d"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "e", "-r", "@+"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   7789610d8ec6 e
    |\  
    @ | 5658521e0f8b d
    | o 90fe0a96fc90 c
    |/  
    o fa5efbdf533c b
    o 90aeefd03044 a
    o 000000000000 
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["unsquash", "-r", "e"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot unsquash merge commits
    "###);

    // Can unsquash from a merge commit
    test_env.jj_cmd_success(&repo_path, &["co", "e"]);
    std::fs::write(repo_path.join("file1"), "e\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["unsquash"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 60ac673b534b (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   60ac673b534b 
    |\  
    o | 5658521e0f8b d e?
    | o 90fe0a96fc90 c e?
    |/  
    o fa5efbdf533c b
    o 90aeefd03044 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_unsquash_partial() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "create", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ d989314f3df0 c
    o 2a2d19a3283f b
    o 47a1e795d146 a
    o 000000000000 
    "###);

    // If we don't make any changes in the diff-editor, the whole change is moved
    // from the parent
    let edit_script = test_env.set_up_fake_diff_editor();
    std::fs::write(&edit_script, "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["unsquash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: 37c961d0d1e2 (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 37c961d0d1e2 c
    o 000af22057b9 b
    o ee67504598b6 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);

    // Can unsquash only some changes in interactive mode
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "reset file1").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["unsquash", "-i"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: a8e8fded1021 (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ a8e8fded1021 c
    o 46cc06672a99 b
    o 47a1e795d146 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "c"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "-T", r#"commit_id.short() " " branches"#],
    )
}
