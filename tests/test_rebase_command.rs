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

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["co", "root"]);
    } else if parents.len() == 1 {
        test_env.jj_cmd_success(repo_path, &["co", parents[0]]);
    } else {
        let mut args = vec!["merge", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
        test_env.jj_cmd_success(repo_path, &["co", &format!(r#"description("{name}")"#)]);
        test_env.jj_cmd_success(repo_path, &["open", "@-"]);
        test_env.jj_cmd_success(repo_path, &["co", "@-"]);
    }
    std::fs::write(repo_path.join(name), &format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", name]);
    test_env.jj_cmd_success(repo_path, &["close", "-m", name]);
}

#[test]
fn test_rebase_invalid() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);

    // Missing destination
    test_env.jj_cmd_cli_error(&repo_path, &["rebase"]);

    // Both -r and -s
    test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-r", "a", "-s", "a", "-d", "b"]);

    // Both -b and -s
    test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-b", "a", "-s", "a", "-d", "b"]);

    // Rebase onto descendant with -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 247da0ddee3d onto descendant 18db23c14b3c
    "###);

    // Rebase onto descendant with -s
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 247da0ddee3d onto descendant 18db23c14b3c
    "###);
}

#[test]
fn test_rebase_branch() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    create_commit(&test_env, &repo_path, "d", &["b"]);
    create_commit(&test_env, &repo_path, "e", &["a"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o e
    | o d
    | | o c
    | |/  
    | o b
    |/  
    o a
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-b", "c", "-d", "e"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o d
    | o c
    |/  
    o b
    | @ 
    |/  
    o e
    o a
    o 
    "###);
}

#[test]
fn test_rebase_branch_with_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &[]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["a", "d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o   e
    |\  
    o | d
    o | c
    | | o b
    | |/  
    | o a
    |/  
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-b", "d", "-d", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 4 commits
    Working copy now at: f6eecf0d8f36 (no description set)
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o e
    o d
    o c
    o b
    o a
    o 
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-d", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 4 commits
    Working copy now at: a15dfb947f3f (no description set)
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o e
    o d
    o c
    o b
    o a
    o 
    "###);
}

#[test]
fn test_rebase_single_revision() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o d
    o   c
    |\  
    o | b
    | o a
    |/  
    o 
    "###);

    // Descendants of the rebased commit should be rebased onto parents. First we
    // test with a non-merge commit, so the descendants should be rebased onto
    // the single parent (commit "a"). Then we test with a merge commit, so the
    // descendants should be rebased onto the two parents.
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 3 descendant commits onto parent of rebased commit
    Working copy now at: ee6a5a3f71d4 (no description set)
    Added 0 files, modified 0 files, removed 2 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o d
    o c
    | o b
    | o a
    |/  
    o 
    "###);
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "c", "-d", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 2 descendant commits onto parent of rebased commit
    Working copy now at: 6dc5b752c6ad (no description set)
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o   d
    |\  
    | | o c
    o | | b
    | |/  
    |/|   
    | o a
    |/  
    o 
    "###);
}

#[test]
fn test_rebase_multiple_destinations() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o c
    | o b
    |/  
    | o a
    |/  
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "a", "-d", "b", "-d", "c"]);
    insta::assert_snapshot!(stdout, @r###""###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   a
    |\  
    | | @ 
    | |/  
    |/|   
    o | c
    | o b
    |/  
    o 
    "###);
}

#[test]
fn test_rebase_with_descendants() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o d
    o   c
    |\  
    o | b
    | o a
    |/  
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 4 commits
    Working copy now at: 60e083aa9086 (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    o d
    o c
    o b
    o a
    o 
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}
