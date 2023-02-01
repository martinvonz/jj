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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_new() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "a new commit"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 4f2d6e0a3482a6a34e4856a4a63869c0df109e79 a new commit
    o 5d5c60b2aa96b8dbf55710656c50285c66cdcd74 add a file
    o 0000000000000000000000000000000000000000 
    "###);

    // Start a new change off of a specific commit (the root commit in this case).
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "off of root", "root"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 026537ddb96b801b9cb909985d5443aab44616c1 off of root
    | o 4f2d6e0a3482a6a34e4856a4a63869c0df109e79 a new commit
    | o 5d5c60b2aa96b8dbf55710656c50285c66cdcd74 add a file
    |/  
    o 0000000000000000000000000000000000000000 
    "###);
}

#[test]
fn test_new_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add file1"]);
    std::fs::write(repo_path.join("file1"), "a").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "root", "-m", "add file2"]);
    std::fs::write(repo_path.join("file2"), "b").unwrap();

    // Create a merge commit
    test_env.jj_cmd_success(&repo_path, &["new", "main", "@"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   0c4e5b9b68ae0cbe7ce3c61042619513d09005bf 
    |\  
    o | f399209d9dda06e8a25a0c8e9a0cde9f421ff35d add file2
    | o 38e8e2f6c92ffb954961fc391b515ff551b41636 add file1
    |/  
    o 0000000000000000000000000000000000000000 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @"a");
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @"b");

    // Same test with `jj merge`
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    test_env.jj_cmd_success(&repo_path, &["merge", "main", "@"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   200ed1a14c8acf09783dafefe5bebf2ff58f12fd 
    |\  
    o | f399209d9dda06e8a25a0c8e9a0cde9f421ff35d add file2
    | o 38e8e2f6c92ffb954961fc391b515ff551b41636 add file1
    |/  
    o 0000000000000000000000000000000000000000 
    "###);

    // `jj merge` with less than two arguments is an error
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["merge"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Merge requires at least two revisions
    "###);
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["merge", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Merge requires at least two revisions
    "###);

    // merge with non-unique revisions
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "@", "200e"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "@" and "200e" resolved to the same revision 200ed1a14c8a
    "###);

    // merge with root
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "@", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot merge with root revision
    "###);
}

#[test]
fn test_new_rebase_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @   F
    |\  
    o | E
    | o D
    |/  
    | o C
    | o B
    | o A
    |/  
    o root
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["new", "--rebase-children", "-m", "G", "B", "D"],
    );
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 descendant commits
    Working copy now at: ca7c6481a8dd G
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    o C
    | o   F
    | |\  
    |/ /  
    @ |   G
    |\ \  
    | | o E
    o | | D
    | |/  
    |/|   
    | o B
    | o A
    |/  
    o root
    "###);
}

#[test]
fn test_new_insert() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @   F
    |\  
    o | E
    | o D
    |/  
    | o C
    | o B
    | o A
    |/  
    o root
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["new", "--insert", "-m", "G", "C", "F"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 descendant commits
    Working copy now at: ff6bbbc7b8df G
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    o F
    | o C
    |/  
    @-.   G
    |\ \  
    o | | E
    | o | D
    |/ /  
    | o B
    | o A
    |/  
    o root
    "###);
}

fn setup_before_insertion(test_env: &TestEnvironment, repo_path: &Path) {
    test_env.jj_cmd_success(repo_path, &["branch", "create", "A"]);
    test_env.jj_cmd_success(repo_path, &["commit", "-m", "A"]);
    test_env.jj_cmd_success(repo_path, &["branch", "create", "B"]);
    test_env.jj_cmd_success(repo_path, &["commit", "-m", "B"]);
    test_env.jj_cmd_success(repo_path, &["branch", "create", "C"]);
    test_env.jj_cmd_success(repo_path, &["describe", "-m", "C"]);
    test_env.jj_cmd_success(repo_path, &["new", "-m", "D", "root"]);
    test_env.jj_cmd_success(repo_path, &["branch", "create", "D"]);
    test_env.jj_cmd_success(repo_path, &["new", "-m", "E", "root"]);
    test_env.jj_cmd_success(repo_path, &["branch", "create", "E"]);
    test_env.jj_cmd_success(repo_path, &["new", "-m", "F", "D", "E"]);
    test_env.jj_cmd_success(repo_path, &["branch", "create", "F"]);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "commit_id \" \" description"])
}

fn get_short_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "-T", r#"if(description, description, "root")"#],
    )
}
