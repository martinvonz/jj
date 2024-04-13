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

use std::path::Path;

use crate::common::TestEnvironment;

#[test]
fn test_parallelize_no_descendants() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..6 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=6"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  b911505e443e 6
    ○  2e00cb15c7b6 5
    ○  9df3c87db1a2 4
    ○  9f5b59fa4622 3
    ○  d826910d21fb 2
    ○  dc0e5d6135ce 1
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "description(1)::"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  6c7b60a45eb6 6
    │ ○  296f48966777 5
    ├─╯
    │ ○  524062469789 4
    ├─╯
    │ ○  a9334ecaa379 3
    ├─╯
    │ ○  3a7b37ebe843 2
    ├─╯
    │ ○  761e67df44b7 1
    ├─╯
    ◆  000000000000
    "###);
}

// Only the head commit has descendants.
#[test]
fn test_parallelize_with_descendants_simple() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..6 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=6"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  b911505e443e 6
    ○  2e00cb15c7b6 5
    ○  9df3c87db1a2 4
    ○  9f5b59fa4622 3
    ○  d826910d21fb 2
    ○  dc0e5d6135ce 1
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(4)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  f28f986c7134 6
    ○        21e9963ac5ff 5
    ├─┬─┬─╮
    │ │ │ ○  524062469789 4
    │ │ ○ │  a9334ecaa379 3
    │ │ ├─╯
    │ ○ │  3a7b37ebe843 2
    │ ├─╯
    ○ │  761e67df44b7 1
    ├─╯
    ◆  000000000000
    "###);
}

// One of the commits being parallelized has a child that isn't being
// parallelized. That child will become a merge of any ancestors which are being
// parallelized.
#[test]
fn test_parallelize_where_interior_has_non_target_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..6 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(2)", "-m=2c"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(5)", "-m=6"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  d27ee705f7a9 6
    ○  2e00cb15c7b6 5
    ○  9df3c87db1a2 4
    ○  9f5b59fa4622 3
    │ ○  9c8865930f3c 2c
    ├─╯
    ○  d826910d21fb 2
    ○  dc0e5d6135ce 1
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "dc0::9df"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9f1bec0d6c46 6
    ○        7dd2f5648395 5
    ├─┬─┬─╮
    │ │ │ ○  b8f977c12383 4
    │ │ ○ │  7be8374575b9 3
    │ │ ├─╯
    │ │ │ ○  679fc870858c 2c
    ╭─┬───╯
    │ ○ │  96ce11389312 2
    │ ├─╯
    ○ │  2bfe3fe3e472 1
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_parallelize_where_root_has_non_target_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..4 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(1)", "-m=1c"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(3)", "-m=4"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  7636b3f489f4 4
    ○  9f5b59fa4622 3
    ○  d826910d21fb 2
    │ ○  50e2ced81124 1c
    ├─╯
    ○  dc0e5d6135ce 1
    ◆  000000000000
    "###);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(3)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    ○  ad35c9caf4fb 1c
    │ @    6ee674074e23 4
    ╭─┼─╮
    │ │ ○  5bd049136a7c 3
    │ ○ │  60f737a5a4a7 2
    │ ├─╯
    ○ │  79ebcd81a1ee 1
    ├─╯
    ◆  000000000000
    "###);
}

// One of the commits being parallelized has a child that is a merge commit.
#[test]
fn test_parallelize_with_merge_commit_child() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", "1"]);
    for n in 2..4 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", "-m", &n.to_string()]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()", "-m", "a"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(2)", "description(a)", "-m", "2a-c"],
    );
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(3)", "-m", "4"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  90a65779e2ec 4
    ○  9f5b59fa4622 3
    │ ○  a01c1fad8506 2a-c
    ╭─┤
    │ ○  1eb902150bb9 a
    ○ │  d826910d21fb 2
    ○ │  dc0e5d6135ce 1
    ├─╯
    ◆  000000000000
    "###);

    // After this finishes, child-2a will have three parents: "1", "2", and "a".
    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(3)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @      5a0dd49510d1 4
    ├─┬─╮
    │ │ ○  a9334ecaa379 3
    │ │ │ ○  605371712469 2a-c
    ╭─┬───┤
    │ │ │ ○  1eb902150bb9 a
    │ │ ├─╯
    │ ○ │  3a7b37ebe843 2
    │ ├─╯
    ○ │  761e67df44b7 1
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_parallelize_failure_disconnected_target_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..3 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9f5b59fa4622 3
    ○  d826910d21fb 2
    ○  dc0e5d6135ce 1
    ◆  000000000000
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_failure(
        &workspace_path, &["parallelize", "description(1)", "description(3)"]),@r###"
    Error: Cannot parallelize since the target revisions are not connected.
    "###);
}

#[test]
fn test_parallelize_head_is_a_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=2"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=a"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=b"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(2)", "description(b)", "-m=merged-head"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    1a8db14a8cf0 merged-head
    ├─╮
    │ ○  401e43e9461f b
    │ ○  66ea2ab19a70 a
    ○ │  d826910d21fb 2
    ○ │  dc0e5d6135ce 1
    ├─╯
    ◆  000000000000
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_failure(&workspace_path,&["parallelize", "description(1)::"]),
        @r###"
    Error: Only the roots of the target revset are allowed to have parents which are not being parallelized.
    "###);
}

#[test]
fn test_parallelize_interior_target_is_a_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()", "-m=a"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(1)", "description(a)", "-m=2"],
    );
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  299099c22761 3
    ○    0c4da981fc0a 2
    ├─╮
    │ ○  6d37472c632c a
    ○ │  dc0e5d6135ce 1
    ├─╯
    ◆  000000000000
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_failure(&workspace_path,&["parallelize", "description(1)::"]),
        @r###"
    Error: Only the roots of the target revset are allowed to have parents which are not being parallelized.
    "###);
}

#[test]
fn test_parallelize_root_is_a_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=y"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()", "-m=x"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(y)", "description(x)", "-m=1"],
    );
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=2"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9f66b50aa1f2 3
    ○  dd995ce87f21 2
    ○    4b4941342e06 1
    ├─╮
    │ ○  4035b23c8f72 x
    ○ │  f3ec359cf9ff y
    ├─╯
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(2)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    4e81469adb0d 3
    ├─╮
    │ ○    38945baf55f4 2
    │ ├─╮
    ○ │ │  9b1a1927720c 1
    ╰─┬─╮
      │ ○  4035b23c8f72 x
      ○ │  f3ec359cf9ff y
      ├─╯
      ◆  000000000000
    "###);
}

#[test]
fn test_parallelize_multiple_heads() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=0"]);
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(0)", "-m=2"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  8314addde180 2
    │ ○  a915696cf0ad 1
    ├─╯
    ○  a56846756248 0
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "description(0)::"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  e84481c26195 2
    │ ○  2047527ade93 1
    ├─╯
    │ ○  9d0c0750973c 0
    ├─╯
    ◆  000000000000
    "###);
}

// All heads must have the same children as the other heads, but only if they
// have children. In this test only one head has children, so the command
// succeeds.
#[test]
fn test_parallelize_multiple_heads_with_and_without_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=0"]);
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(0)", "-m=2"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  8314addde180 2
    │ ○  a915696cf0ad 1
    ├─╯
    ○  a56846756248 0
    ◆  000000000000
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(0)", "description(1)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  49fe9e130d15 2
    ○  9d0c0750973c 0
    │ ○  2047527ade93 1
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_parallelize_multiple_roots() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()", "-m=a"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(1)", "description(a)", "-m=2"],
    );
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  299099c22761 3
    ○    0c4da981fc0a 2
    ├─╮
    │ ○  6d37472c632c a
    ○ │  dc0e5d6135ce 1
    ├─╯
    ◆  000000000000
    "###);

    // Succeeds because the roots have the same parents.
    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "root().."]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  3c90598481cd 3
    │ ○  b96aa55582e5 2
    ├─╯
    │ ○  3178394e33e7 a
    ├─╯
    │ ○  1d9a0895e7d6 1
    ├─╯
    ◆  000000000000
    "###);
}

#[test]
fn test_parallelize_failure_multiple_heads_with_different_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=2"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=3"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=a"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=b"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=c"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9b5fa4b364d4
    ○  7b095ae9b21f c
    ○  5164ab888473 b
    ○  f16fe8ac5ce9 a
    │ ○  9f5b59fa4622 3
    │ ○  d826910d21fb 2
    │ ○  dc0e5d6135ce 1
    ├─╯
    ◆  000000000000
    "###);

    insta::assert_snapshot!(
    test_env.jj_cmd_failure(
        &workspace_path,
        &[
            "parallelize",
            "description(1)::description(2)",
            "description(a)::description(b)",
        ],
    ),@r###"
    Error: All heads of the target revisions must have the same children.
    "###);
}

#[test]
fn test_parallelize_failure_multiple_roots_with_different_parents() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=2"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=a"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=b"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(2)", "description(b)", "-m=merged-head"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    1a8db14a8cf0 merged-head
    ├─╮
    │ ○  401e43e9461f b
    │ ○  66ea2ab19a70 a
    ○ │  d826910d21fb 2
    ○ │  dc0e5d6135ce 1
    ├─╯
    ◆  000000000000
    "###);

    insta::assert_snapshot!(
    test_env.jj_cmd_failure(
        &workspace_path,
        &["parallelize", "description(2)::", "description(b)::"],
    ),@r###"
    Error: All roots of the target revisions must have the same parents.
    "###);
}

#[test]
fn test_parallelize_complex_nonlinear_target() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=0", "root()"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=1", "description(0)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=2", "description(0)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3", "description(0)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=4", "all:heads(..)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=1c", "description(1)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=2c", "description(2)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3c", "description(3)"]);
    insta::assert_snapshot!(get_log_output_with_parents(&test_env, &workspace_path), @r###"
    @  b043eb81416c 3c parents: 3
    │ ○    48277ee9afe0 4 parents: 3 2 1
    ╭─┼─╮
    ○ │ │  944922f0c69f 3 parents: 0
    │ │ │ ○  9d28e8e38435 2c parents: 2
    │ ├───╯
    │ ○ │  97d7522f40e8 2 parents: 0
    ├─╯ │
    │ ○ │  6c82c22a5e35 1c parents: 1
    │ ├─╯
    │ ○  0c058af014a6 1 parents: 0
    ├─╯
    ○  745bea8029c1 0 parents:
    ◆  000000000000 parents:
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(0)::description(4)"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: yostqsxw d193f3b7 (empty) 3c
    Parent commit      : rlvkpnrz cbb4e169 (empty) 0
    Parent commit      : mzvwutvl cb944786 (empty) 3
    "###);
    insta::assert_snapshot!(get_log_output_with_parents(&test_env, &workspace_path), @r###"
    @    d193f3b72495 3c parents: 0 3
    ├─╮
    │ ○  cb9447869bf0 3 parents:
    │ │ ○  80fbafb56917 2c parents: 0 2
    ╭───┤
    │ │ ○  8f4b8ef68676 2 parents:
    │ ├─╯
    │ │ ○  1985e0427139 1c parents: 0 1
    ╭───┤
    │ │ ○  82918d78c984 1 parents:
    │ ├─╯
    ○ │  cbb4e1692ef4 0 parents:
    ├─╯
    │ ○  14ca4df576b3 4 parents:
    ├─╯
    ◆  000000000000 parents:
    "###)
}

fn get_log_output_with_parents(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"
    separate(" ",
        commit_id.short(),
        description.first_line(),
        "parents:",
        parents.map(|c|c.description().first_line())
    )"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"separate(" ", commit_id.short(), local_branches, description)"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
