// Copyright 2024 The Jujutsu Authors
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

use std::path::{Path, PathBuf};

use crate::common::TestEnvironment;

fn set_up() -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    test_env.jj_cmd_ok(&origin_path, &["describe", "-m=public 1"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "-m=public 2"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    test_env.jj_cmd_ok(
        test_env.env_root(),
        &[
            "git",
            "clone",
            "--config-toml=git.auto-local-branch=true",
            origin_git_repo_path.to_str().unwrap(),
            "local",
        ],
    );
    let workspace_root = test_env.env_root().join("local");

    (test_env, workspace_root)
}

fn set_up_remote_at_main(test_env: &TestEnvironment, workspace_root: &Path, remote_name: &str) {
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", remote_name]);
    let other_path = test_env.env_root().join(remote_name);
    let other_git_repo_path = other_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");
    test_env.jj_cmd_ok(
        workspace_root,
        &[
            "git",
            "remote",
            "add",
            remote_name,
            other_git_repo_path.to_str().unwrap(),
        ],
    );
    test_env.jj_cmd_ok(
        workspace_root,
        &["git", "push", "--remote", remote_name, "-b=main"],
    );
}

#[test]
fn test_git_private_commits_block_pushing() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "main", "-m=private 1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "main"]);

    // Will not push when a pushed commit is contained in git.private-commits
    test_env.add_config(r#"git.private-commits = "description(glob:'private*')""#);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit aa3058ff8663 since it is private
    "###);

    // May push when the commit is removed from git.private-commits
    test_env.add_config(r#"git.private-commits = "none()""#);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch main from 7eb97bf230ad to aa3058ff8663
    Warning: The working-copy commit in workspace 'default' became immutable, so a new commit has been created on top of it.
    Working copy now at: znkkpsqq 2e1adf47 (empty) (no description set)
    Parent commit      : yqosqzyt aa3058ff main | (empty) private 1
    "###);
}

#[test]
fn test_git_private_commits_are_not_checked_if_immutable() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "main", "-m=private 1"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "main"]);

    test_env.add_config(r#"git.private-commits = "description(glob:'private*')""#);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "all()""#);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch main from 7eb97bf230ad to aa3058ff8663
    Warning: The working-copy commit in workspace 'default' became immutable, so a new commit has been created on top of it.
    Working copy now at: yostqsxw dce4a15c (empty) (no description set)
    Parent commit      : yqosqzyt aa3058ff main | (empty) private 1
    "###);
}

#[test]
fn test_git_private_commits_not_directly_in_line_block_pushing() {
    let (test_env, workspace_root) = set_up();

    // New private commit descended from root()
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=private 1"]);

    test_env.jj_cmd_ok(&workspace_root, &["new", "main", "@", "-m=public 3"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch1"]);

    test_env.add_config(r#"git.private-commits = "description(glob:'private*')""#);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-b=branch1"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit f1253a9b1ea9 since it is private
    "###);
}

#[test]
fn test_git_private_commits_descending_from_commits_pushed_do_not_block_pushing() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "main", "-m=public 3"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "move", "main"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m=private 1"]);

    test_env.add_config(r#"git.private-commits = "description(glob:'private*')""#);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-b=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch main from 7eb97bf230ad to 05ef53bc99ec
    "###);
}

#[test]
fn test_git_private_commits_already_on_the_remote_do_not_block_push() {
    let (test_env, workspace_root) = set_up();

    // Start a branch before a "private" commit lands in main
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch1", "-r=main"]);

    // Push a commit that would become a private_root if it weren't already on
    // the remote
    test_env.jj_cmd_ok(&workspace_root, &["new", "main", "-m=private 1"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m=public 3"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "main"]);
    let (_, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-b=main", "-b=branch1"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch main from 7eb97bf230ad to fbb352762352
      Add branch branch1 to 7eb97bf230ad
    Warning: The working-copy commit in workspace 'default' became immutable, so a new commit has been created on top of it.
    Working copy now at: kpqxywon a7b08364 (empty) (no description set)
    Parent commit      : yostqsxw fbb35276 main | (empty) public 3
    "###);

    test_env.add_config(r#"git.private-commits = "description(glob:'private*')""#);

    // Since "private 1" is already on the remote, pushing it should be allowed
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch1", "-r=main"]);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch branch1 from 7eb97bf230ad to fbb352762352
    "###);

    // Ensure that the already-pushed commit doesn't block a new branch from
    // being pushed
    test_env.jj_cmd_ok(
        &workspace_root,
        &["new", "description('private 1')", "-m=public 4"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "branch2"]);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-b=branch2"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Add branch branch2 to ee5b808b0b95
    "###);
}

#[test]
fn test_git_private_commits_are_evaluated_separately_for_each_remote() {
    let (test_env, workspace_root) = set_up();
    set_up_remote_at_main(&test_env, &workspace_root, "other");
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);

    // Push a commit that would become a private_root if it weren't already on
    // the remote
    test_env.jj_cmd_ok(&workspace_root, &["new", "main", "-m=private 1"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m=public 3"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "main"]);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-b=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Branch changes to push to origin:
      Move forward branch main from 7eb97bf230ad to d8632ce893ab
    "###);

    test_env.add_config(r#"git.private-commits = "description(glob:'private*')""#);

    // But pushing to a repo that doesn't have the private commit yet is still
    // blocked
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--remote=other", "-b=main"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 36b7ecd11ad9 since it is private
    "###);
}
