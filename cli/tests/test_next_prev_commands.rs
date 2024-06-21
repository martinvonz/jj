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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    // Move @- from third to second.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  zsuskulnrvyr
    ◉  kkmpptxzrspx third
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
    // Move @- from fourth to second.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev", "2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: yqosqzyt d2edc95b (empty) (no description set)
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
}

#[test]
fn test_next_exceeding_history() {
    // Try to step beyond the current repos history.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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

// The working copy commit is a child of a "fork" with two children on each
// branch.
#[test]
fn test_next_parent_has_multiple_descendants() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    // Setup.
    test_env.jj_cmd_ok(&repo_path, &["desc", "-m", "1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "2"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "3"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "4"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(3)"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  mzvwutvlkqwt 4
    @  zsuskulnrvyr 3
    │ ◉  kkmpptxzrspx 2
    │ ◉  qpvuntsmwlqt 1
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);

    // --edit is implied since the working copy isn't a leaf commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next"]);
    insta::assert_snapshot!(stdout,@r###""###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: mzvwutvl 1b8531ce (empty) 4
    Parent commit      : zsuskuln b1394455 (empty) 3
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  mzvwutvlkqwt 4
    ◉  zsuskulnrvyr 3
    │ ◉  kkmpptxzrspx 2
    │ ◉  qpvuntsmwlqt 1
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_next_with_merge_commit_parent() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    // Setup.
    test_env.jj_cmd_ok(&repo_path, &["desc", "-m", "1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "2"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "description(1)", "description(2)", "-m", "3"],
    );
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "4"]);
    test_env.jj_cmd_ok(&repo_path, &["prev", "0"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  royxmykxtrkr
    │ ◉  mzvwutvlkqwt 4
    ├─╯
    ◉    zsuskulnrvyr 3
    ├─╮
    │ ◉  kkmpptxzrspx 2
    ◉ │  qpvuntsmwlqt 1
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next"]);
    insta::assert_snapshot!(stdout,@r###""###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: vruxwmqv 718bbcd9 (empty) (no description set)
    Parent commit      : mzvwutvl cb5881ec (empty) 4
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  vruxwmqvtpmx
    ◉  mzvwutvlkqwt 4
    ◉    zsuskulnrvyr 3
    ├─╮
    │ ◉  kkmpptxzrspx 2
    ◉ │  qpvuntsmwlqt 1
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_next_on_merge_commit() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    // Setup.
    test_env.jj_cmd_ok(&repo_path, &["desc", "-m", "1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "2"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "description(1)", "description(2)", "-m", "3"],
    );
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "4"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(3)"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  mzvwutvlkqwt 4
    @    zsuskulnrvyr 3
    ├─╮
    │ ◉  kkmpptxzrspx 2
    ◉ │  qpvuntsmwlqt 1
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);

    // --edit is implied since the working copy is not a leaf commit.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next"]);
    insta::assert_snapshot!(stdout,@r###""###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: mzvwutvl cb5881ec (empty) 4
    Parent commit      : zsuskuln 038acb86 (empty) 3
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  mzvwutvlkqwt 4
    ◉    zsuskulnrvyr 3
    ├─╮
    │ ◉  kkmpptxzrspx 2
    ◉ │  qpvuntsmwlqt 1
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_next_fails_on_branching_children_no_stdin() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
fn test_prev_on_merge_commit() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["desc", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "c", "left"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "c", "right"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "left", "right"]);

    // Check that the graph looks the way we expect.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    royxmykxtrkr
    ├─╮
    │ ◉  zsuskulnrvyr right second
    ◉ │  qpvuntsmwlqt left first
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: vruxwmqv 41658cf4 (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_stdin_ok(&repo_path, &["prev", "--edit"], "2\n");
    insta::assert_snapshot!(stdout, @r###"
    ambiguous prev commit, choose one to target:
    1: zsuskuln b0d21db3 right | (empty) second
    2: qpvuntsm 69542c19 left | (empty) first
    q: quit the prompt
    enter the index of the commit you want to target: 
    "###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: qpvuntsm 69542c19 left | (empty) first
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
}

#[test]
fn test_prev_on_merge_commit_with_parent_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["desc", "-m", "x"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "y"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "description(x)", "description(y)", "-m", "z"],
    );
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "1"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["new", "description(z)", "description(1)", "-m", "M"],
    );

    // Check that the graph looks the way we expect.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    royxmykxtrkr M
    ├─╮
    │ ◉  mzvwutvlkqwt 1
    ◉ │    zsuskulnrvyr z
    ├───╮
    │ │ ◉  kkmpptxzrspx y
    │ ├─╯
    ◉ │  qpvuntsmwlqt x
    ├─╯
    ◉  zzzzzzzzzzzz
    "###);

    let (stdout, stderr) = test_env.jj_cmd_stdin_ok(&repo_path, &["prev"], "2\n");
    insta::assert_snapshot!(stdout, @r###"
    ambiguous prev commit, choose one to target:
    1: kkmpptxz 146d5c67 (empty) y
    2: qpvuntsm c56e5035 (empty) x
    3: zzzzzzzz 00000000 (empty) (no description set)
    q: quit the prompt
    enter the index of the commit you want to target: 
    "###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: vruxwmqv e8ff4fa0 (empty) (no description set)
    Parent commit      : qpvuntsm c56e5035 (empty) x
    "###);

    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_stdin_ok(&repo_path, &["prev", "--edit"], "2\n");
    insta::assert_snapshot!(stdout, @r###"
    ambiguous prev commit, choose one to target:
    1: mzvwutvl 89b8a355 (empty) 1
    2: zsuskuln 1ef71474 (empty) z
    q: quit the prompt
    enter the index of the commit you want to target: 
    "###);
    insta::assert_snapshot!(stderr,@r###"
    Working copy now at: zsuskuln 1ef71474 (empty) z
    Parent commit      : qpvuntsm c56e5035 (empty) x
    Parent commit      : kkmpptxz 146d5c67 (empty) y
    "###);
}

#[test]
fn test_prev_prompts_on_multiple_parents() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@--"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // Create a merge commit, which has two parents.
    test_env.jj_cmd_ok(&repo_path, &["new", "all:@--+"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "merge"]);

    // Check that the graph looks the way we expect.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  vruxwmqvtpmx
    ◉      yqosqzytrlsw merge
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    // @- is at "fourth", and there is no parent 5 commits behind it.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["prev", "5"]);
    insta::assert_snapshot!(stderr,@r###"
    Error: No ancestor found 5 commits back
    "###);
}

#[test]
fn test_prev_editing() {
    // Edit the third commit.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    // Edit the "fourth" commit, which becomes the leaf.
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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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

#[test]
fn test_prev_conflict() {
    // Make the first commit our new parent.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let file_path = repo_path.join("content.txt");
    std::fs::write(&file_path, "first").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    std::fs::write(&file_path, "second").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // Create a conflict in the first commit, where we'll jump to.
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(first)"]);
    std::fs::write(&file_path, "first+1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "description(third)"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  yqosqzytrlsw conflict
    ◉  royxmykxtrkr conflict fourth
    ◉  kkmpptxzrspx conflict third
    ◉  rlvkpnrzqnoo conflict second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
    test_env.jj_cmd_ok(&repo_path, &["prev", "--conflict"]);
    // TODO: We now should be a child of `third`.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  yostqsxwqrlt conflict
    │ ◉  royxmykxtrkr conflict fourth
    │ ◉  kkmpptxzrspx conflict third
    ├─╯
    ◉  rlvkpnrzqnoo conflict second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_prev_conflict_editing() {
    // Edit the third commit.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let file_path = repo_path.join("content.txt");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    std::fs::write(&file_path, "second").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // Create a conflict in the third commit, where we'll jump to.
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(first)"]);
    std::fs::write(&file_path, "first text").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "description(third)"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  royxmykxtrkr conflict
    ◉  kkmpptxzrspx conflict third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
    test_env.jj_cmd_ok(&repo_path, &["prev", "--conflict", "--edit"]);
    // We now should be editing the third commit.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  kkmpptxzrspx conflict third
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_next_conflict() {
    // There is a conflict in the second commit, so after next it should be the new
    // parent.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let file_path = repo_path.join("content.txt");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    std::fs::write(&file_path, "second").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    // Create a conflict in the second commit.
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(first)"]);
    std::fs::write(&file_path, "first").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "description(second)"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "description(second)"]);
    // Test the setup
    // TODO: This test doesn't seem to test what it's supposed to (we're already on
    // the second commit)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  yqosqzytrlsw conflict
    │ ◉  mzvwutvlkqwt conflict third
    ├─╯
    ◉  rlvkpnrzqnoo conflict second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
    test_env.jj_cmd_ok(&repo_path, &["next", "--conflict"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  yostqsxwqrlt conflict
    │ ◉  mzvwutvlkqwt conflict third
    ├─╯
    ◉  rlvkpnrzqnoo conflict second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_next_conflict_editing() {
    // There is a conflict in the third commit, so after next it should be our
    // working copy.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let file_path = repo_path.join("content.txt");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    std::fs::write(&file_path, "second").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    // Create a conflict in the third commit.
    std::fs::write(&file_path, "third").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["edit", "description(second)"]);
    std::fs::write(&file_path, "modified second").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@+"]);
    // Test the setup
    // TODO: This test doesn't seem to test what it's supposed to (we're already on
    // top of the third commit)
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  mzvwutvlkqwt conflict
    ◉  kkmpptxzrspx conflict
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
    test_env.jj_cmd_ok(&repo_path, &["next", "--conflict", "--edit"]);
    // We now should be editing the third commit.
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  yqosqzytrlsw conflict
    ◉  kkmpptxzrspx conflict
    ◉  rlvkpnrzqnoo second
    ◉  qpvuntsmwlqt first
    ◉  zzzzzzzzzzzz
    "###);
}

#[test]
fn test_next_conflict_head() {
    // When editing a head with conflicts, `jj next --conflict [--edit]` errors out.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let file_path = repo_path.join("file");
    std::fs::write(&file_path, "first").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(&file_path, "second").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["abandon", "@-"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  rlvkpnrzqnoo conflict
    ◉  zzzzzzzzzzzz
    "###);
    // TODO: The command should be an error since there is no conflict after the
    // current one
    test_env.jj_cmd_ok(&repo_path, &["next", "--conflict"]);
    // TODO: The command should be an error since there is no conflict after the
    // current one
    test_env.jj_cmd_ok(&repo_path, &["next", "--conflict", "--edit"]);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"separate(" ", change_id.short(), local_branches, if(conflict, "conflict"), description)"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
