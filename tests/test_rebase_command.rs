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
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

#[test]
fn test_rebase_invalid() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);

    // Missing destination
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["rebase"]);
    insta::assert_snapshot!(stderr, @r###"
    error: The following required arguments were not provided:
      --destination <DESTINATION>

    Usage: jj rebase --destination <DESTINATION>

    For more information try '--help'
    "###);

    // Both -r and -s
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-r", "a", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: The argument '--revision <REVISION>' cannot be used with '--source <SOURCE>'

    Usage: jj rebase --destination <DESTINATION> --revision <REVISION>

    For more information try '--help'
    "###);

    // Both -b and -s
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["rebase", "-b", "a", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    error: The argument '--branch <BRANCH>' cannot be used with '--source <SOURCE>'

    Usage: jj rebase --destination <DESTINATION> --branch <BRANCH>

    For more information try '--help'
    "###);

    // Rebase onto descendant with -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 873140c1fed9 onto descendant ad05f5d1407c
    "###);

    // Rebase onto descendant with -s
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s", "a", "-d", "b"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rebase 873140c1fed9 onto descendant ad05f5d1407c
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
    @ e
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
    @ e
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
    @   e
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
    Rebased 3 commits
    Working copy now at: b2674fa494af e
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ e
    o d
    o c
    o b
    o a
    o 
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-d", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: fef1da569696 e
    Added 1 files, modified 0 files, removed 0 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ e
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
    @ d
    o   c
    |\  
    o | b
    | o a
    |/  
    o 
    "###);

    // Descendants of the rebased commit "b" should be rebased onto parents. First
    // we test with a non-merge commit. Normally, the descendant "c" would still
    // have 2 parents afterwards: the parent of "b" -- the root commit -- and
    // "a". However, since the root commit is an ancestor of "a", we don't
    // actually want both to be parents of the same commit. So, only "a" becomes
    // a parent.
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 2 descendant commits onto parent of rebased commit
    Working copy now at: ed4d09bb181f d
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ d
    o c
    | o b
    |/  
    o a
    o 
    "###);
    test_env.jj_cmd_success(&repo_path, &["undo"]);

    // Now, let's try moving the merge commit. After, both parents of "c" ("a" and
    // "b") should become parents of "d".
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "c", "-d", "root"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: 59a16d87a26f d
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   d
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
fn test_rebase_single_revision_merge_parent() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["b"]);
    create_commit(&test_env, &repo_path, "d", &["a", "c"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   d
    |\  
    o | c
    o | b
    | o a
    |/  
    o 
    "###);

    // Descendants of the rebased commit should be rebased onto parents, and if
    // the descendant is a merge commit, it shouldn't forget its other parents.
    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "c", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Also rebased 1 descendant commits onto parent of rebased commit
    Working copy now at: a4fccbb7582d d
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   d
    |\  
    | | o c
    | |/  
    o | b
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
    @ c
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
    @ | c
    | o b
    |/  
    o 
    "###);

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["rebase", "-r", "a", "-d", "b", "-d", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot merge with root revision
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
    @ d
    o   c
    |\  
    o | b
    | o a
    |/  
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s", "b", "-d", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 3 commits
    Working copy now at: 9afba1135175 d
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ d
    o c
    o b
    o a
    o 
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}
