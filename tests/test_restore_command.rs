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
fn test_restore() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    std::fs::write(repo_path.join("file3"), "c\n").unwrap();

    // Restores from parent by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore"]);
    insta::assert_snapshot!(stdout, @r###"
    Created b05f8b84f2fc (no description set)
    Working copy now at: b05f8b84f2fc (no description set)
    Added 1 files, modified 1 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"");

    // Can restore from other revision
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "--from", "@--"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 9cb58509136b (no description set)
    Working copy now at: 9cb58509136b (no description set)
    Added 1 files, modified 0 files, removed 2 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file2
    "###);

    // Can restore into other revision
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "--to", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 5ed06151e039 (no description set)
    Rebased 1 descendant commits
    Working copy now at: ca6c95b68bd2 (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    A file2
    A file3
    "###);

    // Can combine `--from` and `--to`
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "--from", "@", "--to", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created c83e17dc46fd (no description set)
    Rebased 1 descendant commits
    Working copy now at: df9fb6892f99 (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    A file2
    A file3
    "###);

    // Can restore only specified paths
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "file2", "file3"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 28647642d4a5 (no description set)
    Working copy now at: 28647642d4a5 (no description set)
    Added 0 files, modified 1 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    "###);
}

// Much of this test is copied from test_resolve_command
#[test]
fn test_restore_conflicted_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[("file", "b\n")]);
    create_commit(&test_env, &repo_path, "conflict", &["a", "b"], &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    conflict
    ├─╮
    ◉ │  b
    │ ◉  a
    ├─╯
    ◉  base
    ◉
    "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
            <<<<<<<
            %%%%%%%
            -base
            +a
            +++++++
            b
            >>>>>>>
            "###);

    // Overwrite the file...
    std::fs::write(repo_path.join("file"), "resolution").unwrap();
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
    @r###"
    Resolved conflict in file:
       1     : <<<<<<<
       2     : %%%%%%%
       3     : -base
       4     : +a
       5     : +++++++
       6     : b
       7     : >>>>>>>
            1: resolution
    "###);

    // ...and restore it back again.
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "file"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 63198ca2e4aa conflict
    Working copy now at: 63198ca2e4aa conflict
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
    <<<<<<<
    %%%%%%%
    -base
    +a
    +++++++
    b
    >>>>>>>
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @"");

    // The same, but without the `file` argument. Overwrite the file...
    std::fs::write(repo_path.join("file"), "resolution").unwrap();
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
    @r###"
    Resolved conflict in file:
       1     : <<<<<<<
       2     : %%%%%%%
       3     : -base
       4     : +a
       5     : +++++++
       6     : b
       7     : >>>>>>>
            1: resolution
    "###);

    // ... and restore it back again.
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore"]);
    insta::assert_snapshot!(stdout, @r###"
    Created d955febceac1 conflict
    Working copy now at: d955febceac1 conflict
    Added 0 files, modified 1 files, removed 0 files
    "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
    <<<<<<<
    %%%%%%%
    -base
    +a
    +++++++
    b
    >>>>>>>
    "###);
}

fn create_commit(
    test_env: &TestEnvironment,
    repo_path: &Path,
    name: &str,
    parents: &[&str],
    files: &[(&str, &str)],
) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    for (name, content) in files {
        std::fs::write(repo_path.join(name), content).unwrap();
    }
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}
