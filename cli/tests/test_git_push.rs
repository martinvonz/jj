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
use std::path::PathBuf;

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

    test_env.jj_cmd_ok(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "create", "bookmark1"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "root()", "-m=description 2"]);
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "create", "bookmark2"]);
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

#[test]
fn test_git_push_nothing() {
    let (test_env, workspace_root) = set_up();
    // Show the setup. `insta` has trouble if this is done inside `set_up()`
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: xtvrqkyv d13ecdbd (empty) description 1
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);
    // No bookmarks to push yet
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_current_bookmark() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    // Update some bookmarks. `bookmark1` is not a current bookmark, but
    // `bookmark2` and `my-bookmark` are.
    test_env.jj_cmd_ok(
        &workspace_root,
        &["describe", "bookmark1", "-m", "modified bookmark1 commit"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark2"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "set", "bookmark2"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    // Check the setup
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: xtvrqkyv 0f8dc656 (empty) modified bookmark1 commit
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv hidden d13ecdbd (empty) description 1
    bookmark2: yostqsxw bc7610b6 (empty) foo
      @origin (behind by 1 commits): rlzusymt 8476341e (empty) description 2
    my-bookmark: yostqsxw bc7610b6 (empty) foo
    "###);
    // First dry-run. `bookmark1` should not get pushed.
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move forward bookmark bookmark2 from 8476341eb395 to bc7610b65a91
      Add bookmark my-bookmark to bc7610b65a91
    Dry-run requested, not pushing.
    "#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move forward bookmark bookmark2 from 8476341eb395 to bc7610b65a91
      Add bookmark my-bookmark to bc7610b65a91
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: xtvrqkyv 0f8dc656 (empty) modified bookmark1 commit
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv hidden d13ecdbd (empty) description 1
    bookmark2: yostqsxw bc7610b6 (empty) foo
      @origin: yostqsxw bc7610b6 (empty) foo
    my-bookmark: yostqsxw bc7610b6 (empty) foo
      @origin: yostqsxw bc7610b6 (empty) foo
    "###);

    // Try pushing backwards
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "bookmark",
            "set",
            "bookmark2",
            "-rbookmark2-",
            "--allow-backwards",
        ],
    );
    // This behavior is a strangeness of our definition of the default push revset.
    // We could consider changing it.
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No bookmarks found in the default push revset: remote_bookmarks(remote=origin)..@
    Nothing changed.
    "###);
    // We can move a bookmark backwards
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-bbookmark2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move backward bookmark bookmark2 from bc7610b65a91 to 8476341eb395
    "#);
}

#[test]
fn test_git_push_parent_bookmark() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    test_env.jj_cmd_ok(&workspace_root, &["edit", "bookmark1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["describe", "-m", "modified bookmark1 commit"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "non-empty description"]);
    std::fs::write(workspace_root.join("file"), "file").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark bookmark1 from d13ecdbda2a2 to e612d524a5c6
    "#);
}

#[test]
fn test_git_push_no_matching_bookmark() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["new"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No bookmarks found in the default push revset: remote_bookmarks(remote=origin)..@
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_matching_bookmark_unchanged() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark1"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No bookmarks found in the default push revset: remote_bookmarks(remote=origin)..@
    Nothing changed.
    "###);
}

/// Test that `jj git push` without arguments pushes a bookmark to the specified
/// remote even if it's already up to date on another remote
/// (`remote_bookmarks(remote=<remote>)..@` vs. `remote_bookmarks()..@`).
#[test]
fn test_git_push_other_remote_has_bookmark() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    // Create another remote (but actually the same)
    let other_remote_path = test_env
        .env_root()
        .join("origin")
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "remote",
            "add",
            "other",
            other_remote_path.to_str().unwrap(),
        ],
    );
    // Modify bookmark1 and push it to `origin`
    test_env.jj_cmd_ok(&workspace_root, &["edit", "bookmark1"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m=modified"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark bookmark1 from d13ecdbda2a2 to a657f1b61b94
    "#);
    // Since it's already pushed to origin, nothing will happen if push again
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No bookmarks found in the default push revset: remote_bookmarks(remote=origin)..@
    Nothing changed.
    "###);
    // The bookmark was moved on the "other" remote as well (since it's actually the
    // same remote), but `jj` is not aware of that since it thinks this is a
    // different remote. So, the push should fail.
    //
    // But it succeeds! That's because the bookmark is created at the same location
    // as it is on the remote. This would also work for a descendant.
    //
    // TODO: Saner test?
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--remote=other"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to other:
      Add bookmark bookmark1 to a657f1b61b94
    "#);
}

#[test]
fn test_git_push_forward_unexpectedly_moved() {
    let (test_env, workspace_root) = set_up();

    // Move bookmark1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["new", "bookmark1", "-m=remote"]);
    std::fs::write(origin_path.join("remote"), "remote").unwrap();
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "set", "bookmark1"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Move bookmark1 forward to another commit locally
    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark1", "-m=local"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "set", "bookmark1"]);

    // Pushing should fail
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move forward bookmark bookmark1 from d13ecdbda2a2 to 6750425ff51c
    Error: Refusing to push a bookmark that unexpectedly moved on the remote. Affected refs: refs/heads/bookmark1
    Hint: Try fetching from the remote, then make the bookmark point to where you want it to be, and push again.
    "#);
}

#[test]
fn test_git_push_sideways_unexpectedly_moved() {
    let (test_env, workspace_root) = set_up();

    // Move bookmark1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["new", "bookmark1", "-m=remote"]);
    std::fs::write(origin_path.join("remote"), "remote").unwrap();
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "set", "bookmark1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &origin_path), @r###"
    bookmark1: vruxwmqv 80284bec remote
      @git (behind by 1 commits): qpvuntsm d13ecdbd (empty) description 1
    bookmark2: zsuskuln 8476341e (empty) description 2
      @git: zsuskuln 8476341e (empty) description 2
    "###);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Move bookmark1 sideways to another commit locally
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=local"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "set", "bookmark1", "--allow-backwards"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: kmkuslsw 0f8bf988 local
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark bookmark1 from d13ecdbda2a2 to 0f8bf988588e
    Error: Refusing to push a bookmark that unexpectedly moved on the remote. Affected refs: refs/heads/bookmark1
    Hint: Try fetching from the remote, then make the bookmark point to where you want it to be, and push again.
    "#);
}

// This tests whether the push checks that the remote bookmarks are in expected
// positions.
#[test]
fn test_git_push_deletion_unexpectedly_moved() {
    let (test_env, workspace_root) = set_up();

    // Move bookmark1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["new", "bookmark1", "-m=remote"]);
    std::fs::write(origin_path.join("remote"), "remote").unwrap();
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "set", "bookmark1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &origin_path), @r###"
    bookmark1: vruxwmqv 80284bec remote
      @git (behind by 1 commits): qpvuntsm d13ecdbd (empty) description 1
    bookmark2: zsuskuln 8476341e (empty) description 2
      @git: zsuskuln 8476341e (empty) description 2
    "###);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Delete bookmark1 locally
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "delete", "bookmark1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1 (deleted)
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--bookmark", "bookmark1"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
    Error: Refusing to push a bookmark that unexpectedly moved on the remote. Affected refs: refs/heads/bookmark1
    Hint: Try fetching from the remote, then make the bookmark point to where you want it to be, and push again.
    "#);
}

#[test]
fn test_git_push_unexpectedly_deleted() {
    let (test_env, workspace_root) = set_up();

    // Delete bookmark1 forward on the remote
    let origin_path = test_env.env_root().join("origin");
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "delete", "bookmark1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &origin_path), @r###"
    bookmark1 (deleted)
      @git: qpvuntsm d13ecdbd (empty) description 1
    bookmark2: zsuskuln 8476341e (empty) description 2
      @git: zsuskuln 8476341e (empty) description 2
    "###);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);

    // Move bookmark1 sideways to another commit locally
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=local"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "set", "bookmark1", "--allow-backwards"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: kpqxywon 1ebe27ba local
      @origin (ahead by 1 commits, behind by 1 commits): xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    // Pushing a moved bookmark fails if deleted on remote
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark bookmark1 from d13ecdbda2a2 to 1ebe27ba04bf
    Error: Refusing to push a bookmark that unexpectedly moved on the remote. Affected refs: refs/heads/bookmark1
    Hint: Try fetching from the remote, then make the bookmark point to where you want it to be, and push again.
    "#);

    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "delete", "bookmark1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1 (deleted)
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);
    // Pushing a *deleted* bookmark succeeds if deleted on remote, even if we expect
    // bookmark1@origin to exist and point somewhere.
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-bbookmark1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
    "#);
}

#[test]
fn test_git_push_creation_unexpectedly_already_exists() {
    let (test_env, workspace_root) = set_up();

    // Forget bookmark1 locally
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "forget", "bookmark1"]);

    // Create a new branh1
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=new bookmark1"]);
    std::fs::write(workspace_root.join("local"), "local").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: yostqsxw cb17dcdc new bookmark1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    "###);

    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark bookmark1 to cb17dcdc74d5
    Error: Refusing to push a bookmark that unexpectedly moved on the remote. Affected refs: refs/heads/bookmark1
    Hint: Try fetching from the remote, then make the bookmark point to where you want it to be, and push again.
    "#);
}

#[test]
fn test_git_push_locally_created_and_rewritten() {
    let (test_env, workspace_root) = set_up();
    // Ensure that remote bookmarks aren't tracked automatically
    test_env.add_config("git.auto-local-branch = false");

    // Push locally-created bookmark
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-mlocal 1"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark my to fcc999921ce9
    "#);

    // Rewrite it and push again, which would fail if the pushed bookmark weren't
    // set to "tracking"
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-mlocal 2"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: xtvrqkyv d13ecdbd (empty) description 1
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: rlzusymt 8476341e (empty) description 2
      @origin: rlzusymt 8476341e (empty) description 2
    my: vruxwmqv bde1d2e4 (empty) local 2
      @origin (ahead by 1 commits, behind by 1 commits): vruxwmqv hidden fcc99992 (empty) local 1
    "###);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark my from fcc999921ce9 to bde1d2e44b2a
    "#);
}

#[test]
fn test_git_push_multiple() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "delete", "bookmark1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "set", "--allow-backwards", "bookmark2"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    // Check the setup
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1 (deleted)
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2: yqosqzyt c4a3c310 (empty) foo
      @origin (ahead by 1 commits, behind by 1 commits): rlzusymt 8476341e (empty) description 2
    my-bookmark: yqosqzyt c4a3c310 (empty) foo
    "###);
    // First dry-run
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
      Move sideways bookmark bookmark2 from 8476341eb395 to c4a3c3105d92
      Add bookmark my-bookmark to c4a3c3105d92
    Dry-run requested, not pushing.
    "#);
    // Dry run requesting two specific bookmarks
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-b=bookmark1", "-b=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
      Add bookmark my-bookmark to c4a3c3105d92
    Dry-run requested, not pushing.
    "#);
    // Dry run requesting two specific bookmarks twice
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "-b=bookmark1",
            "-b=my-bookmark",
            "-b=bookmark1",
            "-b=glob:my-*",
            "--dry-run",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
      Add bookmark my-bookmark to c4a3c3105d92
    Dry-run requested, not pushing.
    "#);
    // Dry run with glob pattern
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-b=glob:bookmark?", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
      Move sideways bookmark bookmark2 from 8476341eb395 to c4a3c3105d92
    Dry-run requested, not pushing.
    "#);

    // Unmatched bookmark name is error
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-b=foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such bookmark: foo
    "###);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "-b=foo", "-b=glob:?bookmark"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: No matching bookmarks for patterns: foo, ?bookmark
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
      Move sideways bookmark bookmark2 from 8476341eb395 to c4a3c3105d92
      Add bookmark my-bookmark to c4a3c3105d92
    "#);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark2: yqosqzyt c4a3c310 (empty) foo
      @origin: yqosqzyt c4a3c310 (empty) foo
    my-bookmark: yqosqzyt c4a3c310 (empty) foo
      @origin: yqosqzyt c4a3c310 (empty) foo
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-rall()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  yqosqzyt test.user@example.com 2001-02-03 08:05:17 bookmark2 my-bookmark c4a3c310
    │  (empty) foo
    │ ○  rlzusymt test.user@example.com 2001-02-03 08:05:10 8476341e
    ├─╯  (empty) description 2
    │ ○  xtvrqkyv test.user@example.com 2001-02-03 08:05:08 d13ecdbd
    ├─╯  (empty) description 1
    ◆  zzzzzzzz root() 00000000
    "###);
}

#[test]
fn test_git_push_changes() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "bar"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Creating bookmark push-yostqsxwqrlt for revision yostqsxwqrlt
    Changes to push to origin:
      Add bookmark push-yostqsxwqrlt to cf1a53a8800a
    "#);
    // test pushing two changes at once
    std::fs::write(workspace_root.join("file"), "modified2").unwrap();
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "-c=(@|@-)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "(@|@-)" resolved to more than one revision
    Hint: The revset "(@|@-)" resolved to these revisions:
      yostqsxw 16c16966 push-yostqsxwqrlt* | bar
      yqosqzyt a050abf4 foo
    Hint: Prefix the expression with 'all:' to allow any number of revisions (i.e. 'all:(@|@-)').
    "###);
    // test pushing two changes at once, part 2
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-c=all:(@|@-)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Creating bookmark push-yqosqzytrlsw for revision yqosqzytrlsw
    Changes to push to origin:
      Move sideways bookmark push-yostqsxwqrlt from cf1a53a8800a to 16c169664e9f
      Add bookmark push-yqosqzytrlsw to a050abf4ff07
    "#);
    // specifying the same change twice doesn't break things
    std::fs::write(workspace_root.join("file"), "modified3").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-c=all:(@|@)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark push-yostqsxwqrlt from 16c169664e9f to ef6313d50ac1
    "#);

    // specifying the same bookmark with --change/--bookmark doesn't break things
    std::fs::write(workspace_root.join("file"), "modified4").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-c=@", "-b=push-yostqsxwqrlt"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark push-yostqsxwqrlt from ef6313d50ac1 to c1e65d3a64ce
    "#);

    // try again with --change that moves the bookmark forward
    std::fs::write(workspace_root.join("file"), "modified5").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "bookmark",
            "set",
            "-r=@-",
            "--allow-backwards",
            "push-yostqsxwqrlt",
        ],
    );
    let stdout = test_env.jj_cmd_success(&workspace_root, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    M file
    Working copy : yostqsxw 38cb417c bar
    Parent commit: yqosqzyt a050abf4 push-yostqsxwqrlt* push-yqosqzytrlsw | foo
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-c=@", "-b=push-yostqsxwqrlt"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Move sideways bookmark push-yostqsxwqrlt from c1e65d3a64ce to 38cb417ce3a6
    "#);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    M file
    Working copy : yostqsxw 38cb417c push-yostqsxwqrlt | bar
    Parent commit: yqosqzyt a050abf4 push-yqosqzytrlsw | foo
    "###);

    // Test changing `git.push-bookmark-prefix`. It causes us to push again.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "--config-toml",
            r"git.push-bookmark-prefix='test-'",
            "--change=@",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Creating bookmark test-yostqsxwqrlt for revision yostqsxwqrlt
    Changes to push to origin:
      Add bookmark test-yostqsxwqrlt to 38cb417ce3a6
    "#);

    // Test deprecation warning for `git.push-branch-prefix`
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "--config-toml",
            r"git.push-branch-prefix='branch-'",
            "--change=@",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Config git.push-branch-prefix is deprecated. Please switch to git.push-bookmark-prefix
    Creating bookmark branch-yostqsxwqrlt for revision yostqsxwqrlt
    Changes to push to origin:
      Add bookmark branch-yostqsxwqrlt to 38cb417ce3a6
    "#);
}

#[test]
fn test_git_push_revisions() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "bar"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark-1"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "baz"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark-2a"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark-2b"]);
    std::fs::write(workspace_root.join("file"), "modified again").unwrap();

    // Push an empty set
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=none()"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: No bookmarks point to the specified revisions: none()
    Nothing changed.
    "###);
    // Push a revision with no bookmarks
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=@--"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No bookmarks point to the specified revisions: @--
    Nothing changed.
    "###);
    // Push a revision with a single bookmark
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=@-", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark bookmark-1 to 5f432a855e59
    Dry-run requested, not pushing.
    "#);
    // Push multiple revisions of which some have bookmarks
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-r=@--", "-r=@-", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: No bookmarks point to the specified revisions: @--
    Changes to push to origin:
      Add bookmark bookmark-1 to 5f432a855e59
    Dry-run requested, not pushing.
    "#);
    // Push a revision with a multiple bookmarks
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-r=@", "--dry-run"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark bookmark-2a to 84f499037f5c
      Add bookmark bookmark-2b to 84f499037f5c
    Dry-run requested, not pushing.
    "#);
    // Repeating a commit doesn't result in repeated messages about the bookmark
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "-r=@-", "-r=@-", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark bookmark-1 to 5f432a855e59
    Dry-run requested, not pushing.
    "#);
}

#[test]
fn test_git_push_mixed() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "bar"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark-1"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "baz"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark-2a"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark-2b"]);
    std::fs::write(workspace_root.join("file"), "modified again").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "--change=@--",
            "--bookmark=bookmark-1",
            "-r=@",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Creating bookmark push-yqosqzytrlsw for revision yqosqzytrlsw
    Changes to push to origin:
      Add bookmark push-yqosqzytrlsw to a050abf4ff07
      Add bookmark bookmark-1 to 5f432a855e59
      Add bookmark bookmark-2a to 84f499037f5c
      Add bookmark bookmark-2b to 84f499037f5c
    "#);
}

#[test]
fn test_git_push_existing_long_bookmark() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "bookmark",
            "create",
            "push-19b790168e73f7a73a98deae21e807c0",
        ],
    );

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change=@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark push-19b790168e73f7a73a98deae21e807c0 to a050abf4ff07
    "#);
}

#[test]
fn test_git_push_unsnapshotted_change() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change", "@"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--change", "@"]);
}

#[test]
fn test_git_push_conflict() {
    let (test_env, workspace_root) = set_up();
    std::fs::write(workspace_root.join("file"), "first").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["commit", "-m", "first"]);
    std::fs::write(workspace_root.join("file"), "second").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["commit", "-m", "second"]);
    std::fs::write(workspace_root.join("file"), "third").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["rebase", "-r", "@", "-d", "@--"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "third"]);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 73c265a92cfd since it has conflicts
    "###);
}

#[test]
fn test_git_push_no_description() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m="]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark", "my-bookmark"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 5b36783cd11c since it has no description
    "###);
    test_env.jj_cmd_ok(
        &workspace_root,
        &[
            "git",
            "push",
            "--bookmark",
            "my-bookmark",
            "--allow-empty-description",
        ],
    );
}

#[test]
fn test_git_push_no_description_in_immutable() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "imm"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m="]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);

    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 5b36783cd11c since it has no description
    "###);

    test_env.add_config(r#"revset-aliases."immutable_heads()" = "imm""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--bookmark=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark my-bookmark to ea7373507ad9
    Dry-run requested, not pushing.
    "#);
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
    run_without_var("JJ_USER", &["checkout", "root()", "-m=initial"]);
    run_without_var("JJ_USER", &["bookmark", "create", "missing-name"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark", "missing-name"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 944313939bbd since it has no author and/or committer set
    "###);
    run_without_var("JJ_EMAIL", &["checkout", "root()", "-m=initial"]);
    run_without_var("JJ_EMAIL", &["bookmark", "create", "missing-email"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark=missing-email"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 59354714f789 since it has no author and/or committer set
    "###);
}

#[test]
fn test_git_push_missing_author_in_immutable() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    run_without_var("JJ_USER", &["new", "root()", "-m=no author name"]);
    run_without_var("JJ_EMAIL", &["new", "-m=no author email"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "imm"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);

    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 011f740bf8b5 since it has no author and/or committer set
    "###);

    test_env.add_config(r#"revset-aliases."immutable_heads()" = "imm""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--bookmark=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark my-bookmark to 68fdae89de4f
    Dry-run requested, not pushing.
    "#);
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
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "missing-name"]);
    run_without_var("JJ_USER", &["describe", "-m=no committer name"]);
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--bookmark=missing-name"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 4fd190283d1a since it has no author and/or committer set
    "###);
    test_env.jj_cmd_ok(&workspace_root, &["checkout", "root()"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "missing-email"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark=missing-email"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit eab97428a6ec since it has no author and/or committer set
    "###);

    // Test message when there are multiple reasons (missing committer and
    // description)
    run_without_var("JJ_EMAIL", &["describe", "-m=", "missing-email"]);
    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark=missing-email"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 1143ed607f54 since it has no description and it has no author and/or committer set
    "###);
}

#[test]
fn test_git_push_missing_committer_in_immutable() {
    let (test_env, workspace_root) = set_up();
    let run_without_var = |var: &str, args: &[&str]| {
        test_env
            .jj_cmd(&workspace_root, args)
            .env_remove(var)
            .assert()
            .success();
    };
    run_without_var("JJ_USER", &["describe", "-m=no committer name"]);
    test_env.jj_cmd_ok(&workspace_root, &["new"]);
    run_without_var("JJ_EMAIL", &["describe", "-m=no committer email"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "imm"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "my-bookmark"]);

    let stderr = test_env.jj_cmd_failure(
        &workspace_root,
        &["git", "push", "--bookmark=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Won't push commit 7e61dc727a8f since it has no author and/or committer set
    "###);

    test_env.add_config(r#"revset-aliases."immutable_heads()" = "imm""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_root,
        &["git", "push", "--bookmark=my-bookmark", "--dry-run"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Add bookmark my-bookmark to c79f85e90b4a
    Dry-run requested, not pushing.
    "#);
}

#[test]
fn test_git_push_deleted() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "delete", "bookmark1"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark1 from d13ecdbda2a2
    "#);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["log", "-rall()"]);
    insta::assert_snapshot!(stdout, @r###"
    ○  rlzusymt test.user@example.com 2001-02-03 08:05:10 bookmark2 8476341e
    │  (empty) description 2
    │ ○  xtvrqkyv test.user@example.com 2001-02-03 08:05:08 d13ecdbd
    ├─╯  (empty) description 1
    │ @  yqosqzyt test.user@example.com 2001-02-03 08:05:13 5b36783c
    ├─╯  (empty) (no description set)
    ◆  zzzzzzzz root() 00000000
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_conflicting_bookmarks() {
    let (test_env, workspace_root) = set_up();
    test_env.add_config("git.auto-local-branch = true");
    let git_repo = {
        let mut git_repo_path = workspace_root.clone();
        git_repo_path.extend([".jj", "repo", "store", "git"]);
        git2::Repository::open(&git_repo_path).unwrap()
    };

    // Forget remote ref, move local ref, then fetch to create conflict.
    git_repo
        .find_reference("refs/remotes/origin/bookmark2")
        .unwrap()
        .delete()
        .unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["git", "import"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-m=description 3"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark2"]);
    test_env.jj_cmd_ok(&workspace_root, &["git", "fetch"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: xtvrqkyv d13ecdbd (empty) description 1
      @origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2 (conflicted):
      + yostqsxw 8e670e2d (empty) description 3
      + rlzusymt 8476341e (empty) description 2
      @origin (behind by 1 commits): rlzusymt 8476341e (empty) description 2
    "###);

    let bump_bookmark1 = || {
        test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark1", "-m=bump"]);
        test_env.jj_cmd_ok(&workspace_root, &["bookmark", "set", "bookmark1"]);
    };

    // Conflicting bookmark at @
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: Bookmark bookmark2 is conflicted
    Hint: Run `jj bookmark list` to inspect, and use `jj bookmark set` to fix it up.
    Nothing changed.
    "###);

    // --bookmark should be blocked by conflicting bookmark
    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--bookmark", "bookmark2"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Bookmark bookmark2 is conflicted
    Hint: Run `jj bookmark list` to inspect, and use `jj bookmark set` to fix it up.
    "###);

    // --all shouldn't be blocked by conflicting bookmark
    bump_bookmark1();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Bookmark bookmark2 is conflicted
    Hint: Run `jj bookmark list` to inspect, and use `jj bookmark set` to fix it up.
    Changes to push to origin:
      Move forward bookmark bookmark1 from d13ecdbda2a2 to 8df52121b022
    "#);

    // --revisions shouldn't be blocked by conflicting bookmark
    bump_bookmark1();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "-rall()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Warning: Bookmark bookmark2 is conflicted
    Hint: Run `jj bookmark list` to inspect, and use `jj bookmark set` to fix it up.
    Changes to push to origin:
      Move forward bookmark bookmark1 from 8df52121b022 to 345e1f64a64d
    "#);
}

#[test]
fn test_git_push_deleted_untracked() {
    let (test_env, workspace_root) = set_up();

    // Absent local bookmark shouldn't be considered "deleted" compared to
    // non-tracking remote bookmark.
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "delete", "bookmark1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "untrack", "bookmark1@origin"],
    );
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--deleted"]);
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--bookmark=bookmark1"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such bookmark: bookmark1
    "###);
}

#[test]
fn test_git_push_tracked_vs_all() {
    let (test_env, workspace_root) = set_up();
    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark1", "-mmoved bookmark1"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "set", "bookmark1"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark2", "-mmoved bookmark2"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "delete", "bookmark2"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "untrack", "bookmark1@origin"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "bookmark3"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: vruxwmqv db059e3f (empty) moved bookmark1
    bookmark1@origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2 (deleted)
      @origin: rlzusymt 8476341e (empty) description 2
    bookmark3: znkkpsqq 1aa4f1f2 (empty) moved bookmark2
    "###);

    // At this point, only bookmark2 is still tracked. `jj git push --tracked` would
    // try to push it and no other bookmarks.
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--tracked", "--dry-run"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to origin:
      Delete bookmark bookmark2 from 8476341eb395
    Dry-run requested, not pushing.
    "#);

    // Untrack the last remaining tracked bookmark.
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "untrack", "bookmark2@origin"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &workspace_root), @r###"
    bookmark1: vruxwmqv db059e3f (empty) moved bookmark1
    bookmark1@origin: xtvrqkyv d13ecdbd (empty) description 1
    bookmark2@origin: rlzusymt 8476341e (empty) description 2
    bookmark3: znkkpsqq 1aa4f1f2 (empty) moved bookmark2
    "###);

    // Now, no bookmarks are tracked. --tracked does not push anything
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--tracked"]);
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);

    // All bookmarks are still untracked.
    // - --all tries to push bookmark1, but fails because a bookmark with the same
    // name exist on the remote.
    // - --all succeeds in pushing bookmark3, since there is no bookmark of the same
    // name on the remote.
    // - It does not try to push bookmark2.
    //
    // TODO: Not trying to push bookmark2 could be considered correct, or perhaps
    // we want to consider this as a deletion of the bookmark that failed because
    // the bookmark was untracked. In the latter case, an error message should be
    // printed. Some considerations:
    // - Whatever we do should be consistent with what `jj bookmark list` does; it
    //   currently does *not* list bookmarks like bookmark2 as "about to be
    //   deleted", as can be seen above.
    // - We could consider showing some hint on `jj bookmark untrack
    //   bookmark2@origin` instead of showing an error here.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push", "--all"]);
    insta::assert_snapshot!(stderr, @r#"
    Warning: Non-tracking remote bookmark bookmark1@origin exists
    Hint: Run `jj bookmark track bookmark1@origin` to import the remote bookmark.
    Changes to push to origin:
      Add bookmark bookmark3 to 1aa4f1f2ef7f
    "#);
}

#[test]
fn test_git_push_moved_forward_untracked() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark1", "-mmoved bookmark1"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "set", "bookmark1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "untrack", "bookmark1@origin"],
    );
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Non-tracking remote bookmark bookmark1@origin exists
    Hint: Run `jj bookmark track bookmark1@origin` to import the remote bookmark.
    Nothing changed.
    "###);
}

#[test]
fn test_git_push_moved_sideways_untracked() {
    let (test_env, workspace_root) = set_up();

    test_env.jj_cmd_ok(&workspace_root, &["new", "root()", "-mmoved bookmark1"]);
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "set", "--allow-backwards", "bookmark1"],
    );
    test_env.jj_cmd_ok(
        &workspace_root,
        &["bookmark", "untrack", "bookmark1@origin"],
    );
    let (_stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["git", "push"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Non-tracking remote bookmark bookmark1@origin exists
    Hint: Run `jj bookmark track bookmark1@origin` to import the remote bookmark.
    Nothing changed.
    "###);
}

#[test]
// TODO: This test fails with libgit2 v1.8.1 on Windows.
#[cfg(not(target_os = "windows"))]
fn test_git_push_to_remote_named_git() {
    let (test_env, workspace_root) = set_up();
    let git_repo = {
        let mut git_repo_path = workspace_root.clone();
        git_repo_path.extend([".jj", "repo", "store", "git"]);
        git2::Repository::open(&git_repo_path).unwrap()
    };
    git_repo.remote_rename("origin", "git").unwrap();

    let stderr =
        test_env.jj_cmd_failure(&workspace_root, &["git", "push", "--all", "--remote=git"]);
    insta::assert_snapshot!(stderr, @r#"
    Changes to push to git:
      Add bookmark bookmark1 to d13ecdbda2a2
      Add bookmark bookmark2 to 8476341eb395
    Error: Git remote named 'git' is reserved for local Git repository
    "#);
}

fn get_bookmark_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    // --quiet to suppress deleted bookmarks hint
    test_env.jj_cmd_success(repo_path, &["bookmark", "list", "--all-remotes", "--quiet"])
}
