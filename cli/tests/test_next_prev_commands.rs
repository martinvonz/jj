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
//

use std::path::Path;

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

#[test]
fn test_next_simple() {
    // Move from first => second.
    // first
    // |
    // second
    // |
    // third
    //
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    // Create a simple linear history, which we'll traverse.
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // Move to `first`
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: royxmykx f039cf03 (empty) (no description set)
    Parent commit      : kkmpptxz 3fa8931e (empty) third
    "###);
}

#[test]
fn test_next_multiple() {
    // Move from first => fourth.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@---"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next", "2"]);
    // We should now be the child of the fourth commit.
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: yqosqzyt 52a2e8c2 (empty) (no description set)
    Parent commit      : zsuskuln 009f88bf (empty) fourth
    "###);
}
#[test]
fn test_prev_simple() {
    // Move from third => second.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // @ is an empty discardable commit created by `jj commit`.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  zsuskulnrvyr
    ◉  kkmpptxzrspx third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev"]);
    // The working copy commit is now a child of "second", even though the
    // commit before our empty working copy commit is actually "third". This is
    // because we skip the empty discardable @ commit when deciding how far to
    // move backwards.
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: royxmykx f039cf03 (empty) (no description set)
    Parent commit      : kkmpptxz 3fa8931e (empty) third
    "###);
}

#[test]
fn test_prev_non_discardable_working_copy() {
    // Move from third => second.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "third"]);
    // @ is NOT discardable because we described it.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  kkmpptxzrspx third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: royxmykx 5647d685 (empty) (no description set)
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
}

#[test]
fn test_prev_multiple_without_root() {
    // Move from fourth => first.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(fourth)"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  zsuskulnrvyr fourth
    ◉  kkmpptxzrspx third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev", "2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv 6c3e8d2a (empty) (no description set)
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
}

#[test]
fn test_next_exceeding_history() {
    // Try to step beyond the current repos history.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "-r", "@--"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["next", "3"]);
    // `jj next` beyond existing history fails.
    insta::assert_snapshot!(stderr, @r###"
    Error: No descendant found 3 commits forward
    "###);
}

#[test]
fn test_next_fails_on_merge_commit() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["branch", "c", "left"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "c", "right"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "left", "right"]);
    // Try to advance the working copy commit.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["next"]);
    insta::assert_snapshot!(stderr,@r###"
    Error: Cannot run `jj next` on a merge commit
    "###);
}

#[test]
fn test_next_fails_on_branching_children_no_stdin() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);

    // Try to advance the working copy commit.
    let assert = test_env.jj_cmd(&repo_path, &["next"]).assert().code(1);
    let stderr = test_env.normalize_output(&get_stderr_string(&assert));
    insta::assert_snapshot!(stderr,@r###"
    Error: Cannot prompt for input since the output is not connected to a terminal
    "###);
}

#[test]
fn test_next_fails_on_branching_children_quit_prompt() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);

    // Try to advance the working copy commit.
    let assert = test_env
        .jj_cmd_stdin(&repo_path, &["next"], "q\n")
        .assert()
        .code(1);
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    let stderr = test_env.normalize_output(&get_stderr_string(&assert));
    insta::assert_snapshot!(stdout,@r###"
    ambiguous next commit, choose one to target:
    1: zsuskuln 40a959a0 (empty) third
    2: rlvkpnrz 5c52832c (empty) second
    q: quit the prompt
    enter the index of the commit you want to target: 
    "###);
    insta::assert_snapshot!(stderr,@r###"
    Error: ambiguous target commit
    "###);
}

#[test]
fn test_next_choose_branching_child() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    // Advance the working copy commit.
    let (stdout, stderr) = test_env.jj_cmd_stdin_ok(&repo_path, &["next"], "2\n");
    insta::assert_snapshot!(stdout,@r###"
    ambiguous next commit, choose one to target:
    1: royxmykx e488d731 (empty) fourth
    2: zsuskuln 40a959a0 (empty) third
    3: rlvkpnrz 5c52832c (empty) second
    q: quit the prompt
    enter the index of the commit you want to target: 
    "###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: yostqsxw 3e7e69dc (empty) (no description set)
    Parent commit      : zsuskuln 40a959a0 (empty) third
    "###);
}

#[test]
fn test_prev_prompts_on_multiple_parents() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // Create a merge commit, which has two parents.
    test_env.jj_cmd_ok(&repo_path, &["new", "all:@--+"]);
    test_env.jj_cmd_ok(&repo_path, &["desc", "-m", "merge"]);

    // Check that the graph looks the way we expect.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @      yqosqzytrlsw merge
    ├─┬─╮
    │ │ ◉  qpvuntsmwlqt first
    │ ◉ │  kkmpptxzrspx second
    │ ├─╯
    ◉ │  mzvwutvlkqwt third
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);

    // Move @ backwards.
    let (stdout, stderr) = test_env.jj_cmd_stdin_ok(&repo_path, &["prev"], "3\n");
    insta::assert_snapshot!(stdout,@r###"
    ambiguous prev commit, choose one to target:
    1: mzvwutvl a082e25d (empty) third
    2: kkmpptxz 09881e5f (empty) second
    3: qpvuntsm 69542c19 (empty) first
    q: quit the prompt
    enter the index of the commit you want to target: 
    "###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: znkkpsqq 94715f3c (empty) (no description set)
    Parent commit      : qpvuntsm 69542c19 (empty) first
    "###);
}

#[test]
fn test_prev_beyond_root_fails() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  mzvwutvlkqwt
    ◉  zsuskulnrvyr fourth
    ◉  kkmpptxzrspx third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
    // The root commit is before "first", which is 5 commits back.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["prev", "6"]);
    insta::assert_snapshot!(stderr,@r###"
    Error: No ancestor found at requested offset.
    "###);
}

#[test]
fn test_prev_editing() {
    // Edit the third commit.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    // Edit the "fourth" commit, which becomes the tip when the empty @ commit
    // is dropped.
    test_env.jj_cmd_ok(&repo_path, &["edit", "@-"]);
    // Check that the graph looks the way we expect.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  zsuskulnrvyr fourth
    ◉  kkmpptxzrspx third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev", "--edit"]);
    insta::assert_snapshot!(stdout, @r"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz 3fa8931e (empty) third
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
    // --edit is implied when already editing a non-head commit
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: rlvkpnrz 5c52832c (empty) second
    Parent commit      : qpvuntsm 69542c19 (empty) first
    "###);
}

#[test]
fn test_next_editing() {
    // Edit the second commit.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "@---"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next", "--edit"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz 3fa8931e (empty) third
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
    // --edit is implied when already editing a non-head commit
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: zsuskuln 009f88bf (empty) fourth
    Parent commit      : kkmpptxz 3fa8931e (empty) third
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"separate(" ", change_id.short(), local_branches, description)"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
