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

    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "d"]);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned commit b7c62f28ed10 d
    Rebased 1 descendant commits onto parents of abandoned commits
    Working copy now at: 11a2e10edf4e e
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   e
    |\  
    o | c d
    | | o b
    | |/  
    | o a
    |/  
    o 
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon"] /* abandons `e` */);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned commit 5557ece3e631 e
    Working copy now at: 6b5275139632 (no description set)
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    | o d e??
    | o c
    | | o b
    | |/  
    |/|   
    o | a e??
    |/  
    o 
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "descendants(c)"]);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned the following commits:
      5557ece3e631 e
      b7c62f28ed10 d
      fe2e8e8b50b3 c
    Working copy now at: e7bb061217d5 (no description set)
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    | o b
    |/  
    o a e??
    o c d e??
    "###);

    // Test abandoning the same commit twice directly
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "b", "b"]);
    // Note that the same commit is listed twice
    insta::assert_snapshot!(stdout, @r###"
    Abandoned the following commits:
      1394f625cbbd b
      1394f625cbbd b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   e
    |\  
    o | d
    o | c
    | o a b
    |/  
    o 
    "###);

    // Test abandoning the same commit twice indirectly
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "d:", "a:"]);
    // Note that the same commit is listed twice
    insta::assert_snapshot!(stdout, @r###"
    Abandoned the following commits:
      5557ece3e631 e
      b7c62f28ed10 d
      5557ece3e631 e
      1394f625cbbd b
      2443ea76b0b1 a
    Working copy now at: af874bffee6e (no description set)
    Added 0 files, modified 0 files, removed 4 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 
    | o c d e??
    |/  
    o a b e??
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["abandon", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rewrite the root commit
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}
