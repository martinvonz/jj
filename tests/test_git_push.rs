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

use std::path::PathBuf;

use crate::common::TestEnvironment;

pub mod common;

fn set_up() -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init(&git_repo_path).unwrap();

    test_env.jj_cmd_success(
        test_env.env_root(),
        &["git", "clone", "git-repo", "jj-repo"],
    );
    let workspace_root = test_env.env_root().join("jj-repo");
    (test_env, workspace_root)
}

#[test]
fn test_git_push_nothing() {
    let (test_env, workspace_root) = set_up();
    // No branches to push yet
    let stdout = test_env.jj_cmd_success(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_conflict() {
    let (test_env, workspace_root) = set_up();
    std::fs::write(workspace_root.join("file"), "first").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "first"]);
    std::fs::write(workspace_root.join("file"), "second").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "second"]);
    std::fs::write(workspace_root.join("file"), "third").unwrap();
    test_env.jj_cmd_success(&workspace_root, &["rebase", "-r", "@", "-d", "@--"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", "third"]);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 50ccff1aeab0 since it has conflicts
    "###);
}

#[test]
fn test_git_push_no_description() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "my-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["close", "-m", ""]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--branch", "my-branch"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 4e5f01c842af since it has no description
    "###);
}

#[test]
fn test_git_push_missing_author() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    run_without_var("JJ_USER", &["checkout", "root"]);
    run_without_var("JJ_USER", &["branch", "create", "missing-name"]);
    run_without_var("JJ_USER", &["close", "-m", "initial"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-name"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 567e1ab3da0e since it has no author and/or committer set
    "###);
    run_without_var("JJ_EMAIL", &["checkout", "root"]);
    run_without_var("JJ_EMAIL", &["branch", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["close", "-m", "initial"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-email"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit ce7b456bb11a since it has no author and/or committer set
    "###);
}

#[test]
fn test_git_push_missing_committer() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "missing-name"]);
    run_without_var("JJ_USER", &["close", "-m", "no committer name"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-name"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit df8d9f6cf625 since it has no author and/or committer set
    "###);
    test_env.jj_cmd_success(&workspace_root, &["checkout", "root"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["close", "-m", "no committer email"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-email"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 61b8a14387d7 since it has no author and/or committer set
    "###);

    // Test message when there are multiple reasons (missing committer and
    // description)
    run_without_var("JJ_EMAIL", &["describe", "-m", "", "missing-email"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--branch", "missing-email"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 9e1aae45b6a3 since it has no description and it has no author and/or committer set
    "###);
}
