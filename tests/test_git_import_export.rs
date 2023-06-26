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

use itertools::Itertools as _;
use jujutsu_lib::backend::{CommitId, ObjectId as _};

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_resolution_of_git_tracking_branches() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-r", "main", "-m", "old_message"]);

    // Create local-git tracking branch
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    // Move the local branch somewhere else
    test_env.jj_cmd_success(&repo_path, &["describe", "-r", "main", "-m", "new_message"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: 3af370264cdc new_message
      @git (ahead by 1 commits, behind by 1 commits): 16d541ca40f4 old_message
    "###);

    // Test that we can address both revisions
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-r=main",
            "-T",
            r#"commit_id ++ " " ++ description"#,
            "--no-graph",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    3af370264cdcbba791762f8ef6bc79b456dcbf3b new_message
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-r=main@git",
            "-T",
            r#"commit_id ++ " " ++ description"#,
            "--no-graph",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    16d541ca40f42baf2dea41aa61a0b5f1cbf1f91b old_message
    "###);
}

#[test]
fn test_git_export_conflicting_git_refs() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "main/sub"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Failed to export some branches:
      main/sub
    Hint: Git doesn't allow a branch name that looks like a parent directory of
    another (e.g. `foo` and `foo/bar`). Try to rename the branches that failed to
    export or their "parent" branches.
    "###);
}

#[test]
fn test_git_export_undo() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::open(repo_path.join(".jj/repo/store/git")).unwrap();

    test_env.jj_cmd_success(&repo_path, &["branch", "create", "a"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a: 230dd059e1b0 (no description set)
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "export"]), @"");

    // "git export" can't be undone.
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "undo"]), @r###"
    Nothing changed.
    "###);
    insta::assert_debug_snapshot!(get_git_repo_refs(&git_repo), @r###"
    [
        (
            "refs/heads/a",
            CommitId(
                "230dd059e1b059aefc0da06a2e5a7dbf22362f22",
            ),
        ),
    ]
    "###);

    // This would re-export branch "a" as the internal state has been rolled back.
    // It might be better to preserve the state, and say "Nothing changed" here.
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "export"]), @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_import_undo() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::open(repo_path.join(".jj/repo/store/git")).unwrap();

    // Create branch "a" in git repo
    let commit_id =
        test_env.jj_cmd_success(&repo_path, &["log", "-Tcommit_id", "--no-graph", "-r@"]);
    let commit = git_repo
        .find_commit(git2::Oid::from_str(&commit_id).unwrap())
        .unwrap();
    git_repo.branch("a", &commit, true).unwrap();

    // Initial state we will return to after `undo`. There are no branches.
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
    let base_operation_id = current_operation_id(&test_env, &repo_path);

    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a: 230dd059e1b0 (no description set)
    "###);

    // "git import" can be undone by default in non-colocated repositories.
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a (forgotten)
      @git: 230dd059e1b0 (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);
    // Try "git import" again, which should re-import the branch "a".
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a (forgotten)
      @git: 230dd059e1b0 (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);

    // If we don't restore the git_refs, undoing the import removes the local branch
    // but makes a following import a no-op.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "op",
            "restore",
            &base_operation_id,
            "--what=repo",
            "--what=remote-tracking",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a (forgotten)
      @git: 230dd059e1b0 (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);
    // Try "git import" again, which should *not* re-import the branch "a" and be a
    // no-op.
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a (forgotten)
      @git: 230dd059e1b0 (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);

    // We can restore *only* the git refs to make an import re-import the branch
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["op", "restore", &base_operation_id, "--what=git-tracking"],
    );
    insta::assert_snapshot!(stdout, @r###"
    "###);
    // The git-tracking branch disappears.
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
    // Try "git import" again, which should again re-import the branch "a".
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a: 230dd059e1b0 (no description set)
    "###);
}

#[test]
fn test_git_import_move_export_with_default_undo() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::open(repo_path.join(".jj/repo/store/git")).unwrap();

    // Create branch "a" in git repo
    let commit_id =
        test_env.jj_cmd_success(&repo_path, &["log", "-Tcommit_id", "--no-graph", "-r@"]);
    let commit = git_repo
        .find_commit(git2::Oid::from_str(&commit_id).unwrap())
        .unwrap();
    git_repo.branch("a", &commit, true).unwrap();

    // Initial state we will try to return to after `op restore`. There are no
    // branches.
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @"");
    let base_operation_id = current_operation_id(&test_env, &repo_path);

    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a: 230dd059e1b0 (no description set)
    "###);

    // Move branch "a" and export to git repo
    test_env.jj_cmd_success(&repo_path, &["new"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "set", "a"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a: 096dc80da670 (no description set)
      @git (behind by 1 commits): 230dd059e1b0 (no description set)
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "export"]), @"");
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
     a: 096dc80da670 (no description set)
     "###);

    // "git import" can be undone with the default `restore` behavior, as shown in
    // the previous test. However, "git export" can't: the branches in the git
    // repo stay where they were.
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "restore", &base_operation_id]), @r###"
    Working copy now at: 230dd059e1b0 (no description set)
    Parent commit      : 000000000000 (no description set)
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a (forgotten)
      @git: 096dc80da670 (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);
    insta::assert_debug_snapshot!(get_git_repo_refs(&git_repo), @r###"
    [
        (
            "refs/heads/a",
            CommitId(
                "096dc80da67094fbaa6683e2a205dddffa31f9a8",
            ),
        ),
    ]
    "###);

    // The last branch "a" state is imported from git. No idea what's the most
    // intuitive result here.
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["git", "import"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    a (forgotten)
      @git: 096dc80da670 (no description set)
      (this branch will be deleted from the underlying Git repo on the next `jj git export`)
    "###);
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list"])
}

fn current_operation_id(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let mut id = test_env.jj_cmd_success(repo_path, &["debug", "operation", "--display=id"]);
    let len_trimmed = id.trim_end().len();
    id.truncate(len_trimmed);
    id
}

fn get_git_repo_refs(git_repo: &git2::Repository) -> Vec<(String, CommitId)> {
    let mut refs: Vec<_> = git_repo
        .references()
        .unwrap()
        .filter_ok(|git_ref| git_ref.is_tag() || git_ref.is_branch() || git_ref.is_remote())
        .filter_map_ok(|git_ref| {
            let full_name = git_ref.name()?.to_owned();
            let git_commit = git_ref.peel_to_commit().ok()?;
            let commit_id = CommitId::from_bytes(git_commit.id().as_bytes());
            Some((full_name, commit_id))
        })
        .try_collect()
        .unwrap();
    refs.sort();
    refs
}
