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
fn test_new() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "a new commit"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 88436dbcdbedc2b8a6ebd0687981906d09ccc68f a new commit
    o 51e9c5819117991e4a6dc5a4a744283fc74f0746 add a file
    o 0000000000000000000000000000000000000000 (no description set)
    "###);

    // Start a new change off of a specific commit (the root commit in this case).
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "off of root", "root"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ d8c0a3e1570f1f5b08113a3427b3160900c3d48e off of root
    | o 88436dbcdbedc2b8a6ebd0687981906d09ccc68f a new commit
    | o 51e9c5819117991e4a6dc5a4a744283fc74f0746 add a file
    |/  
    o 0000000000000000000000000000000000000000 (no description set)
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
    @   5b37ef8ee8cd934dfe1e70adff66cd0679f5a573 (no description set)
    |\  
    o | 99814c62bec5c13d2053435b3d6bbeb1900cb57e add file2
    | o fe37af248a068697c6dcd7ebd17f5aac2205e7cb add file1
    |/  
    o 0000000000000000000000000000000000000000 (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @"a");
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @"b");

    // Same test with `jj merge`
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    test_env.jj_cmd_success(&repo_path, &["merge", "main", "@"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   c34d60aa33225c2080da52faa39980efe944bddd (no description set)
    |\  
    o | 99814c62bec5c13d2053435b3d6bbeb1900cb57e add file2
    | o fe37af248a068697c6dcd7ebd17f5aac2205e7cb add file1
    |/  
    o 0000000000000000000000000000000000000000 (no description set)
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
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "@", "c34d"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "@" and "c34d" resolved to the same revision c34d60aa3322
    "###);

    // merge with root
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "@", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot merge with root revision
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "commit_id \" \" description"])
}
