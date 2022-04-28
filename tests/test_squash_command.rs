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
fn test_squash() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    // Test the setup
    let template = r#"commit_id.short() " " branches"#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ 90fe0a96fc90 c
    o fa5efbdf533c b
    o 90aeefd03044 a
    o 000000000000 
    "###);

    // Squashes the working copy into the parent by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 6ca29c9d2e7c 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ 6ca29c9d2e7c b c
    o 90aeefd03044 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Can squash a given commit into its parent
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["squash", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: e87cf8ebc7e1 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ e87cf8ebc7e1 c
    o 893c93ae2a87 a b
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Cannot squash a merge commit (because it's unclear which parent it should go
    // into)
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    test_env.jj_cmd_success(&repo_path, &["co", "b"]);
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "d"]);
    std::fs::write(repo_path.join("file2"), "d\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["merge", "-m", "merge", "c", "d"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "e", "-r", "@+"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    o   b9ad3fdfc2c4 e
    |\  
    @ | 9a18f8da1e69 d
    | o 90fe0a96fc90 c
    |/  
    o fa5efbdf533c b
    o 90aeefd03044 a
    o 000000000000 
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash", "-r", "e"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot squash merge commits
    "###);

    // Can squash into a merge commit
    test_env.jj_cmd_success(&repo_path, &["co", "e"]);
    std::fs::write(repo_path.join("file1"), "e\n").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy now at: 626d78245205 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ 626d78245205 
    o   2a25465aba5f e
    |\  
    o | 9a18f8da1e69 d
    | o 90fe0a96fc90 c
    |/  
    o fa5efbdf533c b
    o 90aeefd03044 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "e"]);
    insta::assert_snapshot!(stdout, @r###"
    e
    "###);
}

#[test]
fn test_squash_partial() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "a"]);
    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "b"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    // Test the setup
    let template = r#"commit_id.short() " " branches"#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ d989314f3df0 c
    o 2a2d19a3283f b
    o 47a1e795d146 a
    o 000000000000 
    "###);

    // If we don't make any changes in the diff-editor, the whole change is moved
    // into the parent
    let edit_script = test_env.set_up_fake_diff_editor();
    std::fs::write(&edit_script, "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["squash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: f03d5ce4a973 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ f03d5ce4a973 c
    o c9f931cd78af a b
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // Can squash only some changes in interactive mode
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    std::fs::write(&edit_script, "reset file1").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["squash", "-r", "b", "-i"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: e7a40106bee6 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ e7a40106bee6 c
    o 05d951646873 b
    o 0c5ddc685260 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // Can squash only some changes in non-interactive mode
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    // Clear the script so we know it won't be used even without -i
    std::fs::write(&edit_script, "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["squash", "-r", "b", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    Working copy now at: a911fa1d0627 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ a911fa1d0627 c
    o fb73ad17899f b
    o 70621f4c7a42 a
    o 000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2", "-r", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
}
