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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..6 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=6"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  b911505e443e 6 parents: 5
    ◉  2e00cb15c7b6 5 parents: 4
    ◉  9df3c87db1a2 4 parents: 3
    ◉  9f5b59fa4622 3 parents: 2
    ◉  d826910d21fb 2 parents: 1
    ◉  dc0e5d6135ce 1 parents:
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "description(1)::"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  6c7b60a45eb6 6 parents:
    │ ◉  296f48966777 5 parents:
    ├─╯
    │ ◉  524062469789 4 parents:
    ├─╯
    │ ◉  a9334ecaa379 3 parents:
    ├─╯
    │ ◉  3a7b37ebe843 2 parents:
    ├─╯
    │ ◉  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

// Only the head commit has descendants.
#[test]
fn test_parallelize_with_descendants_simple() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..6 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=6"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  b911505e443e 6 parents: 5
    ◉  2e00cb15c7b6 5 parents: 4
    ◉  9df3c87db1a2 4 parents: 3
    ◉  9f5b59fa4622 3 parents: 2
    ◉  d826910d21fb 2 parents: 1
    ◉  dc0e5d6135ce 1 parents:
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(4)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  259d624373d7 6 parents: 5
    ◉        60d419591c77 5 parents: 1 2 3 4
    ├─┬─┬─╮
    │ │ │ ◉  524062469789 4 parents:
    │ │ ◉ │  a9334ecaa379 3 parents:
    │ │ ├─╯
    │ ◉ │  3a7b37ebe843 2 parents:
    │ ├─╯
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

// One of the commits being parallelized has a child that isn't being
// parallelized. That child will become a merge of any ancestors which are being
// parallelized.
#[test]
fn test_parallelize_where_interior_has_non_target_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..6 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(2)", "-m=2c"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(5)", "-m=6"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  d27ee705f7a9 6 parents: 5
    ◉  2e00cb15c7b6 5 parents: 4
    ◉  9df3c87db1a2 4 parents: 3
    ◉  9f5b59fa4622 3 parents: 2
    │ ◉  9c8865930f3c 2c parents: 2
    ├─╯
    ◉  d826910d21fb 2 parents: 1
    ◉  dc0e5d6135ce 1 parents:
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "dc0::9df"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  a42de3959cae 6 parents: 5
    ◉        d907c901bad0 5 parents: 1 2 3 4
    ├─┬─┬─╮
    │ │ │ ◉  b8f977c12383 4 parents:
    │ │ ◉ │  7be8374575b9 3 parents:
    │ │ ├─╯
    │ │ │ ◉  2a4c3dab2a50 2c parents: 1 2
    ╭─┬───╯
    │ ◉ │  96ce11389312 2 parents:
    │ ├─╯
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_where_root_has_non_target_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..4 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(1)", "-m=1c"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(3)", "-m=4"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  7636b3f489f4 4 parents: 3
    ◉  9f5b59fa4622 3 parents: 2
    ◉  d826910d21fb 2 parents: 1
    │ ◉  50e2ced81124 1c parents: 1
    ├─╯
    ◉  dc0e5d6135ce 1 parents:
    ◉  000000000000 parents:
    "###);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(3)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @      d024344469c3 4 parents: 1 2 3
    ├─┬─╮
    │ │ ◉  5bd049136a7c 3 parents:
    │ ◉ │  60f737a5a4a7 2 parents:
    │ ├─╯
    │ │ ◉  50e2ced81124 1c parents: 1
    ├───╯
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

// One of the commits being parallelized has a child that is a merge commit.
#[test]
fn test_parallelize_with_merge_commit_child() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  90a65779e2ec 4 parents: 3
    ◉  9f5b59fa4622 3 parents: 2
    │ ◉  a01c1fad8506 2a-c parents: 2 a
    ╭─┤
    │ ◉  1eb902150bb9 a parents:
    ◉ │  d826910d21fb 2 parents: 1
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    // After this finishes, child-2a will have three parents: "1", "2", and "a".
    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(3)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @      6107429ab54b 4 parents: 1 2 3
    ├─┬─╮
    │ │ ◉  a9334ecaa379 3 parents:
    │ │ │ ◉  a386386b94bc 2a-c parents: 1 2 a
    ╭─┬───┤
    │ │ │ ◉  1eb902150bb9 a parents:
    │ │ ├─╯
    │ ◉ │  3a7b37ebe843 2 parents:
    │ ├─╯
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_disconnected_target_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");

    for n in 1..3 {
        test_env.jj_cmd_ok(&workspace_path, &["commit", &format!("-m{n}")]);
    }
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9f5b59fa4622 3 parents: 2
    ◉  d826910d21fb 2 parents: 1
    ◉  dc0e5d6135ce 1 parents:
    ◉  000000000000 parents:
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)", "description(3)"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9f5b59fa4622 3 parents: 2
    ◉  d826910d21fb 2 parents: 1
    ◉  dc0e5d6135ce 1 parents:
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_head_is_a_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=0"]);
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
    @    f2087b66e475 merged-head parents: 2 b
    ├─╮
    │ ◉  5164ab888473 b parents: a
    │ ◉  f16fe8ac5ce9 a parents:
    ◉ │  fe79412860e8 2 parents: 1
    ◉ │  a915696cf0ad 1 parents: 0
    ◉ │  a56846756248 0 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "description(1)::"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    babb4191912d merged-head parents: 0 b
    ├─╮
    │ ◉  5164ab888473 b parents: a
    │ ◉  f16fe8ac5ce9 a parents:
    │ │ ◉  36b2f866a798 2 parents: 0
    ├───╯
    │ │ ◉  a915696cf0ad 1 parents: 0
    ├───╯
    ◉ │  a56846756248 0 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_interior_target_is_a_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=0"]);
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()", "-m=a"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(1)", "description(a)", "-m=2"],
    );
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  a6321093e3d3 3 parents: 2
    ◉    705c32f67ce1 2 parents: 1 a
    ├─╮
    │ ◉  427890ea3f2b a parents:
    ◉ │  a915696cf0ad 1 parents: 0
    ◉ │  a56846756248 0 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "description(1)::"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    cd0ac6ad1415 3 parents: 0 a
    ├─╮
    │ │ ◉  1c240e875670 2 parents: 0 a
    ╭─┬─╯
    │ ◉  427890ea3f2b a parents:
    │ │ ◉  a915696cf0ad 1 parents: 0
    ├───╯
    ◉ │  a56846756248 0 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_root_is_a_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  9f66b50aa1f2 3 parents: 2
    ◉  dd995ce87f21 2 parents: 1
    ◉    4b4941342e06 1 parents: y x
    ├─╮
    │ ◉  4035b23c8f72 x parents:
    ◉ │  f3ec359cf9ff y parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(1)::description(2)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    d6df04b236b0 3 parents: 1 2
    ├─╮
    │ ◉    38945baf55f4 2 parents: y x
    │ ├─╮
    ◉ │ │  4b4941342e06 1 parents: y x
    ╰─┬─╮
      │ ◉  4035b23c8f72 x parents:
      ◉ │  f3ec359cf9ff y parents:
      ├─╯
      ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_multiple_heads() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=0"]);
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(0)", "-m=2"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  8314addde180 2 parents: 0
    │ ◉  a915696cf0ad 1 parents: 0
    ├─╯
    ◉  a56846756248 0 parents:
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "description(0)::"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  e84481c26195 2 parents:
    │ ◉  2047527ade93 1 parents:
    ├─╯
    │ ◉  a56846756248 0 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

// All heads must have the same children as the other heads, but only if they
// have children. In this test only one head has children, so the command
// succeeds.
#[test]
fn test_parallelize_multiple_heads_with_and_without_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=0"]);
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "description(0)", "-m=2"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  8314addde180 2 parents: 0
    │ ◉  a915696cf0ad 1 parents: 0
    ├─╯
    ◉  a56846756248 0 parents:
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(0)", "description(1)"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    ◉  2047527ade93 1 parents:
    │ @  8314addde180 2 parents: 0
    │ ◉  a56846756248 0 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_multiple_roots() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["describe", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()", "-m=a"]);
    test_env.jj_cmd_ok(
        &workspace_path,
        &["new", "description(1)", "description(a)", "-m=2"],
    );
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  299099c22761 3 parents: 2
    ◉    0c4da981fc0a 2 parents: 1 a
    ├─╮
    │ ◉  6d37472c632c a parents:
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    // Succeeds because the roots have the same parents.
    test_env.jj_cmd_ok(&workspace_path, &["parallelize", "root().."]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  3c90598481cd 3 parents:
    │ ◉  b96aa55582e5 2 parents:
    ├─╯
    │ ◉  6d37472c632c a parents:
    ├─╯
    │ ◉  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_multiple_heads_with_different_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=1"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=2"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=3"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=a"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=b"]);
    test_env.jj_cmd_ok(&workspace_path, &["commit", "-m=c"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  9b5fa4b364d4 parents: c
    ◉  7b095ae9b21f c parents: b
    ◉  5164ab888473 b parents: a
    ◉  f16fe8ac5ce9 a parents:
    │ ◉  9f5b59fa4622 3 parents: 2
    │ ◉  d826910d21fb 2 parents: 1
    │ ◉  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &[
            "parallelize",
            "description(1)::description(2)",
            "description(a)::description(b)",
        ],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  582c6bd1e1fd parents: c
    ◉    dd2db8b60a69 c parents: a b
    ├─╮
    │ ◉  190b857f6cdd b parents:
    ◉ │  f16fe8ac5ce9 a parents:
    ├─╯
    │ ◉    bbc313370f45 3 parents: 1 2
    │ ├─╮
    │ │ ◉  96ce11389312 2 parents:
    ├───╯
    │ ◉  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_multiple_roots_with_different_parents() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @    1a8db14a8cf0 merged-head parents: 2 b
    ├─╮
    │ ◉  401e43e9461f b parents: a
    │ ◉  66ea2ab19a70 a parents:
    ◉ │  d826910d21fb 2 parents: 1
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);

    test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(2)::", "description(b)::"],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    4224f9c9e598 merged-head parents: 1 a
    ├─╮
    │ │ ◉  401e43e9461f b parents: a
    │ ├─╯
    │ ◉  66ea2ab19a70 a parents:
    │ │ ◉  d826910d21fb 2 parents: 1
    ├───╯
    ◉ │  dc0e5d6135ce 1 parents:
    ├─╯
    ◉  000000000000 parents:
    "###);
}

#[test]
fn test_parallelize_complex_nonlinear_target() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=0", "root()"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=1", "description(0)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=2", "description(0)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3", "description(0)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=4", "all:heads(..)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=1c", "description(1)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=2c", "description(2)"]);
    test_env.jj_cmd_ok(&workspace_path, &["new", "-m=3c", "description(3)"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @  b043eb81416c 3c parents: 3
    │ ◉    48277ee9afe0 4 parents: 3 2 1
    ╭─┼─╮
    ◉ │ │  944922f0c69f 3 parents: 0
    │ │ │ ◉  9d28e8e38435 2c parents: 2
    │ ├───╯
    │ ◉ │  97d7522f40e8 2 parents: 0
    ├─╯ │
    │ ◉ │  6c82c22a5e35 1c parents: 1
    │ ├─╯
    │ ◉  0c058af014a6 1 parents: 0
    ├─╯
    ◉  745bea8029c1 0 parents:
    ◉  000000000000 parents:
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &workspace_path,
        &["parallelize", "description(0)::description(4)"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: yostqsxw 59a216e5 (empty) 3c
    Parent commit      : rlvkpnrz 745bea80 (empty) 0
    Parent commit      : mzvwutvl cb944786 (empty) 3
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_path), @r###"
    @    59a216e537c4 3c parents: 0 3
    ├─╮
    │ ◉  cb9447869bf0 3 parents:
    │ │ ◉  248ce1ffd76b 2c parents: 0 2
    ╭───┤
    │ │ ◉  8f4b8ef68676 2 parents:
    │ ├─╯
    │ │ ◉  55c626d090e2 1c parents: 0 1
    ╭───┤
    │ │ ◉  82918d78c984 1 parents:
    │ ├─╯
    ◉ │  745bea8029c1 0 parents:
    ├─╯
    │ ◉  14ca4df576b3 4 parents:
    ├─╯
    ◉  000000000000 parents:
    "###)
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"
    separate(" ",
        commit_id.short(),
        description.first_line(),
        "parents:",
        parents.map(|c|c.description().first_line())
    )"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
