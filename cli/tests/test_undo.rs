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
fn test_undo_rewrite_with_child() {
    // Test that if we undo an operation that rewrote some commit, any descendants
    // after that will be rebased on top of the un-rewritten commit.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "modified"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let op_id_hex = stdout[3..15].to_string();
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "child"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  child
    ◉  modified
    ◉
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo", &op_id_hex]);

    // Since we undid the description-change, the child commit should now be on top
    // of the initial commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  child
    ◉  initial
    ◉
    "###);
}

#[test]
fn test_undo_in_dirty_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-mcommit1"]);
    std::fs::write(repo_path.join("file2"), "").unwrap();

    // This should undo the commit1, not the snapshot of the working copy.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "undo"]);
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: rlvkpnrz b811feb0 (no description set)
    Parent commit      : qpvuntsm 230dd059 (empty) (no description set)
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @  rlvkpnrz test.user@example.com 2001-02-03 04:05:09.000 +07:00 b811feb0
    │  (no description set)
    │  A file2
    ◉  qpvuntsm test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059
    │  (empty) (no description set)
    ◉  zzzzzzzz root() 00000000
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  593305332dfe test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  undo operation 10ad5feba032fd172057df92f45905c38133bbe0fe533c9d0ba1dcb9335b4ed8e644a54a2137817b9884c856aa5a5868c390c96139c872d82f24a76838e002d5
    │  args: jj op undo
    ◉  841868e6c503 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  snapshot working copy
    │  args: jj op undo
    ◉  10ad5feba032 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj commit -mcommit1
    ◉  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
}

#[test]
fn test_op_restore_in_dirty_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-mcommit1"]);
    std::fs::write(repo_path.join("file2"), "").unwrap();

    // This should restore to the previous state, not to the current state
    // before snapshotting the working copy.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "restore", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 230dd059 (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 0 files, removed 1 files
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059
    │  (empty) (no description set)
    ◉  zzzzzzzz root() 00000000
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  80b89dd58913 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  restore to operation 19b8089fc78b7c49171f3c8934248be6f89f52311005e961cab5780f9f138b142456d77b27d223d7ee84d21d8c30c4a80100eaf6735b548b1acd0da688f94c80
    │  args: jj op restore @-
    ◉  176b5fa491b0 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  snapshot working copy
    │  args: jj op restore @-
    ◉  10ad5feba032 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj commit -mcommit1
    ◉  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
}

#[test]
fn test_git_push_undo() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "git-repo", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "AA"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "BB"]);
    //   Refs at this point look as follows (-- means no ref)
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local `main`     | BB      |   --   | --
    //    remote-tracking  | AA      |   AA   | AA
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);
    let pre_push_opid = test_env.current_operation_id(&repo_path);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local  `main`    | BB      |   --   | --
    //    remote-tracking  | BB      |   BB   | BB
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin: qpvuntsm 8c05de15 (empty) BB
    "###);

    // Undo the push
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &pre_push_opid]);
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local  `main`    | BB      |   --   | --
    //    remote-tracking  | AA      |   AA   | BB
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "CC"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    // TODO: The user would probably not expect a conflict here. It currently is
    // because the undo made us forget that the remote was at v2, so the fetch
    // made us think it updated from v1 to v2 (instead of the no-op it could
    // have been).
    //
    // One option to solve this would be to have undo not restore remote-tracking
    // branches, but that also has undersired consequences: the second fetch in `jj
    // git fetch && jj undo && jj git fetch` would become a no-op.
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main (conflicted):
      - qpvuntsm hidden 0cffb614 (empty) AA
      + qpvuntsm?? 0a3e99f0 (empty) CC
      + qpvuntsm?? 8c05de15 (empty) BB
      @origin (behind by 1 commits): qpvuntsm?? 8c05de15 (empty) BB
    "###);
}

/// This test is identical to the previous one, except for one additional
/// import. It demonstrates that this changes the outcome.
#[test]
fn test_git_push_undo_with_import() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "git-repo", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "AA"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "BB"]);
    //   Refs at this point look as follows (-- means no ref)
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local `main`     | BB      |   --   | --
    //    remote-tracking  | AA      |   AA   | AA
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);
    let pre_push_opid = test_env.current_operation_id(&repo_path);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local  `main`    | BB      |   --   | --
    //    remote-tracking  | BB      |   BB   | BB
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin: qpvuntsm 8c05de15 (empty) BB
    "###);

    // Undo the push
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &pre_push_opid]);
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local  `main`    | BB      |   --   | --
    //    remote-tracking  | AA      |   AA   | BB
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);

    // PROBLEM: inserting this import changes the outcome compared to previous test
    // TODO: decide if this is the better behavior, and whether import of
    // remote-tracking branches should happen on every operation.
    test_env.jj_cmd_ok(&repo_path, &["git", "import"]);
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local  `main`    | BB      |   --   | --
    //    remote-tracking  | BB      |   BB   | BB
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin: qpvuntsm 8c05de15 (empty) BB
    "###);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "CC"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    // There is not a conflict. This seems like a good outcome; undoing `git push`
    // was essentially a no-op.
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 0a3e99f0 (empty) CC
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 8c05de15 (empty) BB
    "###);
}

// This test is currently *identical* to `test_git_push_undo` except the repo
// it's operating it is colocated.
#[test]
fn test_git_push_undo_colocated() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path.clone()).unwrap();
    let repo_path = test_env.env_root().join("clone");
    git2::Repository::clone(git_repo_path.to_str().unwrap(), &repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);

    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "AA"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "BB"]);
    //   Refs at this point look as follows (-- means no ref)
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local `main`     | BB      |   BB   | BB
    //    remote-tracking  | AA      |   AA   | AA
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @git: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);
    let pre_push_opid = test_env.current_operation_id(&repo_path);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local `main`     | BB      |   BB   | BB
    //    remote-tracking  | BB      |   BB   | BB
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @git: qpvuntsm 8c05de15 (empty) BB
      @origin: qpvuntsm 8c05de15 (empty) BB
    "###);

    // Undo the push
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &pre_push_opid]);
    //       === Before auto-export ====
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local `main`     | BB      |   BB   | BB
    //    remote-tracking  | AA      |   BB   | BB
    //       === After automatic `jj git export` ====
    //                     | jj refs | jj's   | git
    //                     |         | git    | repo
    //                     |         |tracking|
    //   ------------------------------------------
    //    local `main`     | BB      |   BB   | BB
    //    remote-tracking  | AA      |   AA   | AA
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @git: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "CC"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    // We have the same conflict as `test_git_push_undo`. TODO: why did we get the
    // same result in a seemingly different way?
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main (conflicted):
      - qpvuntsm hidden 0cffb614 (empty) AA
      + qpvuntsm?? 0a3e99f0 (empty) CC
      + qpvuntsm?? 8c05de15 (empty) BB
      @git (behind by 1 commits): qpvuntsm?? 0a3e99f0 (empty) CC
      @origin (behind by 1 commits): qpvuntsm?? 8c05de15 (empty) BB
    "###);
}

// This test is currently *identical* to `test_git_push_undo` except
// both the git_refs and the remote-tracking branches are preserved by undo.
// TODO: Investigate the different outcome
#[test]
fn test_git_push_undo_repo_only() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "git-repo", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "AA"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 0cffb614 (empty) AA
      @origin: qpvuntsm 0cffb614 (empty) AA
    "###);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "BB"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 0cffb614 (empty) AA
    "###);
    let pre_push_opid = test_env.current_operation_id(&repo_path);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);

    // Undo the push, but keep both the git_refs and the remote-tracking branches
    test_env.jj_cmd_ok(
        &repo_path,
        &["op", "restore", "--what=repo", &pre_push_opid],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 8c05de15 (empty) BB
      @origin: qpvuntsm 8c05de15 (empty) BB
    "###);
    test_env.advance_test_rng_seed_to_multiple_of(100_000);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "CC"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    // This currently gives an identical result to `test_git_push_undo_import`.
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    main: qpvuntsm 0a3e99f0 (empty) CC
      @origin (ahead by 1 commits, behind by 1 commits): qpvuntsm hidden 8c05de15 (empty) BB
    "###);
}

#[test]
fn test_branch_track_untrack_undo() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "git-repo", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-mcommit"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "feature1", "feature2"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "delete", "feature2"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: qpvuntsm 270721f5 (empty) commit
      @origin: qpvuntsm 270721f5 (empty) commit
    feature2 (deleted)
      @origin: qpvuntsm 270721f5 (empty) commit
      (this branch will be *deleted permanently* on the remote on the next `jj git push`. Use `jj branch forget` to prevent this)
    "###);

    // Track/untrack can be undone so long as states can be trivially merged.
    test_env.jj_cmd_ok(
        &repo_path,
        &["branch", "untrack", "feature1@origin", "feature2@origin"],
    );
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: qpvuntsm 270721f5 (empty) commit
    feature1@origin: qpvuntsm 270721f5 (empty) commit
    feature2@origin: qpvuntsm 270721f5 (empty) commit
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: qpvuntsm 270721f5 (empty) commit
      @origin: qpvuntsm 270721f5 (empty) commit
    feature2 (deleted)
      @origin: qpvuntsm 270721f5 (empty) commit
      (this branch will be *deleted permanently* on the remote on the next `jj git push`. Use `jj branch forget` to prevent this)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: qpvuntsm 270721f5 (empty) commit
    feature1@origin: qpvuntsm 270721f5 (empty) commit
    feature2@origin: qpvuntsm 270721f5 (empty) commit
    "###);

    test_env.jj_cmd_ok(&repo_path, &["branch", "track", "feature1@origin"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: qpvuntsm 270721f5 (empty) commit
      @origin: qpvuntsm 270721f5 (empty) commit
    feature2@origin: qpvuntsm 270721f5 (empty) commit
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(get_branch_output(&test_env, &repo_path), @r###"
    feature1: qpvuntsm 270721f5 (empty) commit
    feature1@origin: qpvuntsm 270721f5 (empty) commit
    feature2@origin: qpvuntsm 270721f5 (empty) commit
    "###);
}

fn get_branch_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["branch", "list", "--all"])
}
