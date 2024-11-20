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

/// Creates a remote Git repo containing a bookmark with the same name
fn init_git_remote(test_env: &TestEnvironment, remote: &str) {
    let git_repo_path = test_env.env_root().join(remote);
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(remote.as_bytes()).unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    git_repo
        .commit(
            Some(&format!("refs/heads/{remote}")),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();
}

/// Add a remote containing a bookmark with the same name
fn add_git_remote(test_env: &TestEnvironment, repo_path: &Path, remote: &str) {
    init_git_remote(test_env, remote);
    test_env.jj_cmd_ok(
        repo_path,
        &["git", "remote", "add", remote, &format!("../{remote}")],
    );
}

fn get_bookmark_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    // --quiet to suppress deleted bookmarks hint
    test_env.jj_cmd_success(repo_path, &["bookmark", "list", "--all-remotes", "--quiet"])
}

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    let descr = format!("descr_for_{name}");
    if parents.is_empty() {
        test_env.jj_cmd_ok(repo_path, &["new", "root()", "-m", &descr]);
    } else {
        let mut args = vec!["new", "-m", &descr];
        args.extend(parents);
        test_env.jj_cmd_ok(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_ok(repo_path, &["bookmark", "create", name]);
}

fn get_log_output(test_env: &TestEnvironment, workspace_root: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ description.first_line() ++ " " ++ bookmarks"#;
    test_env.jj_cmd_success(workspace_root, &["log", "-T", template, "-r", "all()"])
}

#[test]
fn test_git_fetch_with_default_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin@origin: oputwtnw ffecd2d6 message
    "###);
}

#[test]
fn test_git_fetch_default_remote() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: oputwtnw ffecd2d6 message
      @origin: oputwtnw ffecd2d6 message
    "###);
}

#[test]
fn test_git_fetch_single_remote() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Fetching from the only existing remote: rem1
    bookmark: rem1@rem1 [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    "###);
}

#[test]
fn test_git_fetch_single_remote_all_remotes_flag() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    test_env
        .jj_cmd(&repo_path, &["git", "fetch", "--all-remotes"])
        .assert()
        .success();
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    "###);
}

#[test]
fn test_git_fetch_single_remote_from_arg() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote", "rem1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    "###);
}

#[test]
fn test_git_fetch_single_remote_from_config() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    test_env.add_config(r#"git.fetch = "rem1""#);

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    "###);
}

#[test]
fn test_git_fetch_multiple_remotes() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    add_git_remote(&test_env, &repo_path, "rem2");

    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "fetch", "--remote", "rem1", "--remote", "rem2"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    rem2: yszkquru 2497a8a0 message
      @rem2: yszkquru 2497a8a0 message
    "###);
}

#[test]
fn test_git_fetch_all_remotes() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    add_git_remote(&test_env, &repo_path, "rem2");

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--all-remotes"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    rem2: yszkquru 2497a8a0 message
      @rem2: yszkquru 2497a8a0 message
    "###);
}

#[test]
fn test_git_fetch_multiple_remotes_from_config() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    add_git_remote(&test_env, &repo_path, "rem2");
    test_env.add_config(r#"git.fetch = ["rem1", "rem2"]"#);

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    rem2: yszkquru 2497a8a0 message
      @rem2: yszkquru 2497a8a0 message
    "###);
}

#[test]
fn test_git_fetch_nonexistent_remote() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    let stderr = &test_env.jj_cmd_failure(
        &repo_path,
        &["git", "fetch", "--remote", "rem1", "--remote", "rem2"],
    );
    insta::assert_snapshot!(stderr, @r###"
    bookmark: rem1@rem1 [new] untracked
    Error: No git remote named 'rem2'
    "###);
    // No remote should have been fetched as part of the failing transaction
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");
}

#[test]
fn test_git_fetch_nonexistent_remote_from_config() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");
    test_env.add_config(r#"git.fetch = ["rem1", "rem2"]"#);

    let stderr = &test_env.jj_cmd_failure(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    bookmark: rem1@rem1 [new] untracked
    Error: No git remote named 'rem2'
    "###);
    // No remote should have been fetched as part of the failing transaction
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");
}

#[test]
fn test_git_fetch_from_remote_named_git() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    let repo_path = test_env.env_root().join("repo");
    init_git_remote(&test_env, "git");
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    git_repo.remote("git", "../git").unwrap();

    // Existing remote named 'git' shouldn't block the repo initialization.
    test_env.jj_cmd_ok(&repo_path, &["git", "init", "--git-repo=."]);

    // Try fetching from the remote named 'git'.
    let stderr = &test_env.jj_cmd_failure(&repo_path, &["git", "fetch", "--remote=git"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to import refs from underlying Git repo
    Caused by: Git remote named 'git' is reserved for local Git repository
    Hint: Run `jj git remote rename` to give different name.
    "###);

    // Implicit import shouldn't fail because of the remote ref.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "list", "--all-remotes"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");

    // Explicit import is an error.
    // (This could be warning if we add mechanism to report ignored refs.)
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["git", "import"]), @r###"
    Error: Failed to import refs from underlying Git repo
    Caused by: Git remote named 'git' is reserved for local Git repository
    Hint: Run `jj git remote rename` to give different name.
    "###);

    // The remote can be renamed, and the ref can be imported.
    test_env.jj_cmd_ok(&repo_path, &["git", "remote", "rename", "git", "bar"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "list", "--all-remotes"]);
    insta::assert_snapshot!(stdout, @r###"
    git: mrylzrtu 76fc7466 message
      @bar: mrylzrtu 76fc7466 message
      @git: mrylzrtu 76fc7466 message
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Done importing changes from the underlying Git repo.
    "###);
}

#[test]
fn test_git_fetch_prune_before_updating_tips() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: oputwtnw ffecd2d6 message
      @origin: oputwtnw ffecd2d6 message
    "###);

    // Remove origin bookmark in git repo and create origin/subname
    let git_repo = git2::Repository::open(test_env.env_root().join("origin")).unwrap();
    git_repo
        .find_branch("origin", git2::BranchType::Local)
        .unwrap()
        .rename("origin/subname", false)
        .unwrap();

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin/subname: oputwtnw ffecd2d6 message
      @origin: oputwtnw ffecd2d6 message
    "###);
}

#[test]
fn test_git_fetch_conflicting_bookmarks() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "rem1");

    // Create a rem1 bookmark locally
    test_env.jj_cmd_ok(&repo_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "rem1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: kkmpptxz fcdbbd73 (empty) (no description set)
    "###);

    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "fetch", "--remote", "rem1", "--branch", "glob:*"],
    );
    // This should result in a CONFLICTED bookmark
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1 (conflicted):
      + kkmpptxz fcdbbd73 (empty) (no description set)
      + qxosxrvv 6a211027 message
      @rem1 (behind by 1 commits): qxosxrvv 6a211027 message
    "###);
}

#[test]
fn test_git_fetch_conflicting_bookmarks_colocated() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    let repo_path = test_env.env_root().join("repo");
    let _git_repo = git2::Repository::init(&repo_path).unwrap();
    // create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &repo_path);
    test_env.jj_cmd_ok(&repo_path, &["git", "init", "--git-repo", "."]);
    add_git_remote(&test_env, &repo_path, "rem1");
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");

    // Create a rem1 bookmark locally
    test_env.jj_cmd_ok(&repo_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "rem1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1: zsuskuln f652c321 (empty) (no description set)
      @git: zsuskuln f652c321 (empty) (no description set)
    "###);

    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "fetch", "--remote", "rem1", "--branch", "rem1"],
    );
    // This should result in a CONFLICTED bookmark
    // See https://github.com/martinvonz/jj/pull/1146#discussion_r1112372340 for the bug this tests for.
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    rem1 (conflicted):
      + zsuskuln f652c321 (empty) (no description set)
      + qxosxrvv 6a211027 message
      @git (behind by 1 commits): zsuskuln f652c321 (empty) (no description set)
      @rem1 (behind by 1 commits): qxosxrvv 6a211027 message
    "###);
}

// Helper functions to test obtaining multiple bookmarks at once and changed
// bookmarks
fn create_colocated_repo_and_bookmarks_from_trunk1(
    test_env: &TestEnvironment,
    repo_path: &Path,
) -> String {
    // Create a colocated repo in `source` to populate it more easily
    test_env.jj_cmd_ok(repo_path, &["git", "init", "--git-repo", "."]);
    create_commit(test_env, repo_path, "trunk1", &[]);
    create_commit(test_env, repo_path, "a1", &["trunk1"]);
    create_commit(test_env, repo_path, "a2", &["trunk1"]);
    create_commit(test_env, repo_path, "b", &["trunk1"]);
    format!(
        "   ===== Source git repo contents =====\n{}",
        get_log_output(test_env, repo_path)
    )
}

fn create_trunk2_and_rebase_bookmarks(test_env: &TestEnvironment, repo_path: &Path) -> String {
    create_commit(test_env, repo_path, "trunk2", &["trunk1"]);
    for br in ["a1", "a2", "b"] {
        test_env.jj_cmd_ok(repo_path, &["rebase", "-b", br, "-d", "trunk2"]);
    }
    format!(
        "   ===== Source git repo contents =====\n{}",
        get_log_output(test_env, repo_path)
    )
}

#[test]
fn test_git_fetch_all() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @  c7d4bdcbc215 descr_for_b b
    │ ○  decaa3966c83 descr_for_a2 a2
    ├─╯
    │ ○  359a9a02457d descr_for_a1 a1
    ├─╯
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);

    // Nothing in our repo before the fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    @  230dd059e1b0
    ◆  000000000000
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin     [new] tracked
    bookmark: a2@origin     [new] tracked
    bookmark: b@origin      [new] tracked
    bookmark: trunk1@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @r###"
    a1: nknoxmzm 359a9a02 descr_for_a1
      @origin: nknoxmzm 359a9a02 descr_for_a1
    a2: qkvnknrk decaa396 descr_for_a2
      @origin: qkvnknrk decaa396 descr_for_a2
    b: vpupmnsl c7d4bdcb descr_for_b
      @origin: vpupmnsl c7d4bdcb descr_for_b
    trunk1: zowqyktl ff36dc55 descr_for_trunk1
      @origin: zowqyktl ff36dc55 descr_for_trunk1
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);

    // ==== Change both repos ====
    // First, change the target repo:
    let source_log = create_trunk2_and_rebase_bookmarks(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    ○  babc49226c14 descr_for_b b
    │ ○  91e46b4b2653 descr_for_a2 a2
    ├─╯
    │ ○  0424f6dfc1ff descr_for_a1 a1
    ├─╯
    @  8f1f14fbbf42 descr_for_trunk2 trunk2
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);
    // Change a bookmark in the source repo as well, so that it becomes conflicted.
    test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &["describe", "b", "-m=new_descr_for_b_to_create_conflict"],
    );

    // Our repo before and after fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  061eddbb43ab new_descr_for_b_to_create_conflict b*
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @r###"
    a1: nknoxmzm 359a9a02 descr_for_a1
      @origin: nknoxmzm 359a9a02 descr_for_a1
    a2: qkvnknrk decaa396 descr_for_a2
      @origin: qkvnknrk decaa396 descr_for_a2
    b: vpupmnsl 061eddbb new_descr_for_b_to_create_conflict
      @origin (ahead by 1 commits, behind by 1 commits): vpupmnsl hidden c7d4bdcb descr_for_b
    trunk1: zowqyktl ff36dc55 descr_for_trunk1
      @origin: zowqyktl ff36dc55 descr_for_trunk1
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin     [updated] tracked
    bookmark: a2@origin     [updated] tracked
    bookmark: b@origin      [updated] tracked
    bookmark: trunk2@origin [new] tracked
    Abandoned 2 commits that are no longer reachable.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @r###"
    a1: quxllqov 0424f6df descr_for_a1
      @origin: quxllqov 0424f6df descr_for_a1
    a2: osusxwst 91e46b4b descr_for_a2
      @origin: osusxwst 91e46b4b descr_for_a2
    b (conflicted):
      - vpupmnsl hidden c7d4bdcb descr_for_b
      + vpupmnsl 061eddbb new_descr_for_b_to_create_conflict
      + vktnwlsu babc4922 descr_for_b
      @origin (behind by 1 commits): vktnwlsu babc4922 descr_for_b
    trunk1: zowqyktl ff36dc55 descr_for_trunk1
      @origin: zowqyktl ff36dc55 descr_for_trunk1
    trunk2: umznmzko 8f1f14fb descr_for_trunk2
      @origin: umznmzko 8f1f14fb descr_for_trunk2
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  babc49226c14 descr_for_b b?? b@origin
    │ │ ○  91e46b4b2653 descr_for_a2 a2
    │ ├─╯
    │ │ ○  0424f6dfc1ff descr_for_a1 a1
    │ ├─╯
    │ ○  8f1f14fbbf42 descr_for_trunk2 trunk2
    │ │ ○  061eddbb43ab new_descr_for_b_to_create_conflict b??
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);
}

#[test]
fn test_git_fetch_some_of_many_bookmarks() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @  c7d4bdcbc215 descr_for_b b
    │ ○  decaa3966c83 descr_for_a2 a2
    ├─╯
    │ ○  359a9a02457d descr_for_a1 a1
    ├─╯
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);

    // Test an error message
    let stderr = test_env.jj_cmd_failure(
        &target_jj_repo_path,
        &["git", "fetch", "--branch", "glob:^:a*"],
    );
    insta::assert_snapshot!(stderr, @"Error: Invalid branch pattern provided. When fetching, branch names and globs may not contain the characters `:`, `^`, `?`, `[`, `]`");
    let stderr = test_env.jj_cmd_failure(&target_jj_repo_path, &["git", "fetch", "--branch", "a*"]);
    insta::assert_snapshot!(stderr, @r"
    Error: Branch names may not include `*`.
    Hint: Prefix the pattern with `glob:` to expand `*` as a glob
    ");

    // Nothing in our repo before the fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    @  230dd059e1b0
    ◆  000000000000
    "###);
    // Fetch one bookmark...
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch", "--branch", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: b@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
    // ...check what the intermediate state looks like...
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @r###"
    b: vpupmnsl c7d4bdcb descr_for_b
      @origin: vpupmnsl c7d4bdcb descr_for_b
    "###);
    // ...then fetch two others with a glob.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &["git", "fetch", "--branch", "glob:a*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin [new] tracked
    bookmark: a2@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  decaa3966c83 descr_for_a2 a2
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ │ ○  c7d4bdcbc215 descr_for_b b
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
    // Fetching the same bookmark again
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch", "--branch", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  decaa3966c83 descr_for_a2 a2
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ │ ○  c7d4bdcbc215 descr_for_b b
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);

    // ==== Change both repos ====
    // First, change the target repo:
    let source_log = create_trunk2_and_rebase_bookmarks(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    ○  01d115196c39 descr_for_b b
    │ ○  31c7d94b1f29 descr_for_a2 a2
    ├─╯
    │ ○  6df2d34cf0da descr_for_a1 a1
    ├─╯
    @  2bb3ebd2bba3 descr_for_trunk2 trunk2
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);
    // Change a bookmark in the source repo as well, so that it becomes conflicted.
    test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &["describe", "b", "-m=new_descr_for_b_to_create_conflict"],
    );

    // Our repo before and after fetch of two bookmarks
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  6ebd41dc4f13 new_descr_for_b_to_create_conflict b*
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &["git", "fetch", "--branch", "b", "--branch", "a1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin [updated] tracked
    bookmark: b@origin  [updated] tracked
    Abandoned 1 commits that are no longer reachable.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  01d115196c39 descr_for_b b?? b@origin
    │ │ ○  6df2d34cf0da descr_for_a1 a1
    │ ├─╯
    │ ○  2bb3ebd2bba3 descr_for_trunk2
    │ │ ○  6ebd41dc4f13 new_descr_for_b_to_create_conflict b??
    │ ├─╯
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);

    // We left a2 where it was before, let's see how `jj bookmark list` sees this.
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @r###"
    a1: ypowunwp 6df2d34c descr_for_a1
      @origin: ypowunwp 6df2d34c descr_for_a1
    a2: qkvnknrk decaa396 descr_for_a2
      @origin: qkvnknrk decaa396 descr_for_a2
    b (conflicted):
      - vpupmnsl hidden c7d4bdcb descr_for_b
      + vpupmnsl 6ebd41dc new_descr_for_b_to_create_conflict
      + nxrpswuq 01d11519 descr_for_b
      @origin (behind by 1 commits): nxrpswuq 01d11519 descr_for_b
    "###);
    // Now, let's fetch a2 and double-check that fetching a1 and b again doesn't do
    // anything.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &["git", "fetch", "--branch", "b", "--branch", "glob:a*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a2@origin [updated] tracked
    Abandoned 1 commits that are no longer reachable.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  31c7d94b1f29 descr_for_a2 a2
    │ │ ○  01d115196c39 descr_for_b b?? b@origin
    │ ├─╯
    │ │ ○  6df2d34cf0da descr_for_a1 a1
    │ ├─╯
    │ ○  2bb3ebd2bba3 descr_for_trunk2
    │ │ ○  6ebd41dc4f13 new_descr_for_b_to_create_conflict b??
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &target_jj_repo_path), @r###"
    a1: ypowunwp 6df2d34c descr_for_a1
      @origin: ypowunwp 6df2d34c descr_for_a1
    a2: qrmzolkr 31c7d94b descr_for_a2
      @origin: qrmzolkr 31c7d94b descr_for_a2
    b (conflicted):
      - vpupmnsl hidden c7d4bdcb descr_for_b
      + vpupmnsl 6ebd41dc new_descr_for_b_to_create_conflict
      + nxrpswuq 01d11519 descr_for_b
      @origin (behind by 1 commits): nxrpswuq 01d11519 descr_for_b
    "###);
}

#[test]
fn test_git_fetch_bookmarks_some_missing() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");
    add_git_remote(&test_env, &repo_path, "rem1");
    add_git_remote(&test_env, &repo_path, "rem2");

    // single missing bookmark, implicit remotes (@origin)
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--branch", "noexist"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branch matching `noexist` found on any specified/configured remote
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");

    // multiple missing bookmarks, implicit remotes (@origin)
    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "git", "fetch", "--branch", "noexist1", "--branch", "noexist2",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branch matching `noexist1` found on any specified/configured remote
    Warning: No branch matching `noexist2` found on any specified/configured remote
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");

    // single existing bookmark, implicit remotes (@origin)
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--branch", "origin"]);
    insta::assert_snapshot!(stderr, @r###"
    bookmark: origin@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: oputwtnw ffecd2d6 message
      @origin: oputwtnw ffecd2d6 message
    "###);

    // multiple existing bookmark, explicit remotes, each bookmark is only in one
    // remote.
    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "git", "fetch", "--branch", "rem1", "--branch", "rem2", "--remote", "rem1", "--remote",
            "rem2",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    bookmark: rem1@rem1 [new] tracked
    bookmark: rem2@rem2 [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: oputwtnw ffecd2d6 message
      @origin: oputwtnw ffecd2d6 message
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    rem2: yszkquru 2497a8a0 message
      @rem2: yszkquru 2497a8a0 message
    "###);

    // multiple bookmarks, one exists, one doesn't
    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "git", "fetch", "--branch", "rem1", "--branch", "notexist", "--remote", "rem1",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    Warning: No branch matching `notexist` found on any specified/configured remote
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: oputwtnw ffecd2d6 message
      @origin: oputwtnw ffecd2d6 message
    rem1: qxosxrvv 6a211027 message
      @rem1: qxosxrvv 6a211027 message
    rem2: yszkquru 2497a8a0 message
      @rem2: yszkquru 2497a8a0 message
    "###);
}

// See `test_undo_restore_commands.rs` for fetch-undo-push and fetch-undo-fetch
// of the same bookmarks for various kinds of undo.
#[test]
fn test_git_fetch_undo() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @  c7d4bdcbc215 descr_for_b b
    │ ○  decaa3966c83 descr_for_a2 a2
    ├─╯
    │ ○  359a9a02457d descr_for_a1 a1
    ├─╯
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);

    // Fetch 2 bookmarks
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &["git", "fetch", "--branch", "b", "--branch", "a1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin [new] tracked
    bookmark: b@origin  [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&target_jj_repo_path, &["undo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Undid operation: eb2029853b02 (2001-02-03 08:05:18) fetch from git remote(s) origin
    "#);
    // The undo works as expected
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    @  230dd059e1b0
    ◆  000000000000
    "###);
    // Now try to fetch just one bookmark
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch", "--branch", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: b@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
}

// Compare to `test_git_import_undo` in test_git_import_export
// TODO: Explain why these behaviors are useful
#[test]
fn test_fetch_undo_what() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @  c7d4bdcbc215 descr_for_b b
    │ ○  decaa3966c83 descr_for_a2 a2
    ├─╯
    │ ○  359a9a02457d descr_for_a1 a1
    ├─╯
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);

    // Initial state we will try to return to after `op restore`. There are no
    // bookmarks.
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");
    let base_operation_id = test_env.current_operation_id(&repo_path);

    // Fetch a bookmark
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--branch", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: b@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    b: vpupmnsl c7d4bdcb descr_for_b
      @origin: vpupmnsl c7d4bdcb descr_for_b
    "###);

    // We can undo the change in the repo without moving the remote-tracking
    // bookmark
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["op", "restore", "--what", "repo", &base_operation_id],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Restored to operation: eac759b9ab75 (2001-02-03 08:05:07) add workspace 'default'
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    b (deleted)
      @origin: vpupmnsl hidden c7d4bdcb descr_for_b
    "###);

    // Now, let's demo restoring just the remote-tracking bookmark. First, let's
    // change our local repo state...
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "c", "newbookmark"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    b (deleted)
      @origin: vpupmnsl hidden c7d4bdcb descr_for_b
    newbookmark: qpvuntsm 230dd059 (empty) (no description set)
    "###);
    // Restoring just the remote-tracking state will not affect `newbookmark`, but
    // will eliminate `b@origin`.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "op",
            "restore",
            "--what",
            "remote-tracking",
            &base_operation_id,
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Restored to operation: eac759b9ab75 (2001-02-03 08:05:07) add workspace 'default'
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    newbookmark: qpvuntsm 230dd059 (empty) (no description set)
    "###);
}

#[test]
fn test_git_fetch_remove_fetch() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "origin"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: qpvuntsm 230dd059 (empty) (no description set)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + oputwtnw ffecd2d6 message
      @origin (behind by 1 commits): oputwtnw ffecd2d6 message
    "###);

    test_env.jj_cmd_ok(&repo_path, &["git", "remote", "remove", "origin"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + oputwtnw ffecd2d6 message
    "###);

    test_env.jj_cmd_ok(&repo_path, &["git", "remote", "add", "origin", "../origin"]);

    // Check that origin@origin is properly recreated
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: origin@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + oputwtnw ffecd2d6 message
      @origin (behind by 1 commits): oputwtnw ffecd2d6 message
    "###);
}

#[test]
fn test_git_fetch_rename_fetch() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    add_git_remote(&test_env, &repo_path, "origin");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "origin"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin: qpvuntsm 230dd059 (empty) (no description set)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + oputwtnw ffecd2d6 message
      @origin (behind by 1 commits): oputwtnw ffecd2d6 message
    "###);

    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "rename", "origin", "upstream"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    origin (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + oputwtnw ffecd2d6 message
      @upstream (behind by 1 commits): oputwtnw ffecd2d6 message
    "###);

    // Check that jj indicates that nothing has changed
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote", "upstream"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_fetch_removed_bookmark() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @  c7d4bdcbc215 descr_for_b b
    │ ○  decaa3966c83 descr_for_a2 a2
    ├─╯
    │ ○  359a9a02457d descr_for_a1 a1
    ├─╯
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);

    // Fetch all bookmarks
    let (stdout, stderr) = test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin     [new] tracked
    bookmark: a2@origin     [new] tracked
    bookmark: b@origin      [new] tracked
    bookmark: trunk1@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);

    // Remove a2 bookmark in origin
    test_env.jj_cmd_ok(&source_git_repo_path, &["bookmark", "forget", "a2"]);

    // Fetch bookmark a1 from origin and check that a2 is still there
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch", "--branch", "a1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);

    // Fetch bookmarks a2 from origin, and check that it has been removed locally
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch", "--branch", "a2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a2@origin [deleted] untracked
    Abandoned 1 commits that are no longer reachable.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);
}

#[test]
fn test_git_fetch_removed_parent_bookmark() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_bookmarks_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @  c7d4bdcbc215 descr_for_b b
    │ ○  decaa3966c83 descr_for_a2 a2
    ├─╯
    │ ○  359a9a02457d descr_for_a1 a1
    ├─╯
    ○  ff36dc55760e descr_for_trunk1 trunk1
    ◆  000000000000
    "###);

    // Fetch all bookmarks
    let (stdout, stderr) = test_env.jj_cmd_ok(&target_jj_repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin     [new] tracked
    bookmark: a2@origin     [new] tracked
    bookmark: b@origin      [new] tracked
    bookmark: trunk1@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ │ ○  359a9a02457d descr_for_a1 a1
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1 trunk1
    ├─╯
    ◆  000000000000
    "#);

    // Remove all bookmarks in origin.
    test_env.jj_cmd_ok(&source_git_repo_path, &["bookmark", "forget", "glob:*"]);

    // Fetch bookmarks master, trunk1 and a1 from origin and check that only those
    // bookmarks have been removed and that others were not rebased because of
    // abandoned commits.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &target_jj_repo_path,
        &[
            "git", "fetch", "--branch", "master", "--branch", "trunk1", "--branch", "a1",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: a1@origin     [deleted] untracked
    bookmark: trunk1@origin [deleted] untracked
    Abandoned 1 commits that are no longer reachable.
    Warning: No branch matching `master` found on any specified/configured remote
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r#"
    @  230dd059e1b0
    │ ○  c7d4bdcbc215 descr_for_b b
    │ │ ○  decaa3966c83 descr_for_a2 a2
    │ ├─╯
    │ ○  ff36dc55760e descr_for_trunk1
    ├─╯
    ◆  000000000000
    "#);
}

#[test]
fn test_git_fetch_remote_only_bookmark() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Create non-empty git repo to add as a remote
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    // Create a commit and a bookmark in the git repo
    git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();

    // Fetch using git.auto_local_bookmark = true
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    git_repo
        .commit(
            Some("refs/heads/feature2"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();

    // Fetch using git.auto_local_bookmark = false
    test_env.add_config("git.auto-local-bookmark = false");
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @  230dd059e1b0
    │ ◆  9f01a0e04879 message feature1 feature2@origin
    ├─╯
    ◆  000000000000
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    feature2@origin: mzyxwzks 9f01a0e0 message
    "###);
}
