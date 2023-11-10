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

use crate::common::TestEnvironment;

pub mod common;

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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev"]);
    // The working copy commit is now a child of "second".
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: mzvwutvl f7a0f876 (empty) (no description set)
    Parent commit      : rlvkpnrz 5c52832c (empty) second
    "###);
}

#[test]
fn test_prev_multiple_without_root() {
    // Move from fourth => second.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev", "2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: royxmykx 5647d685 (empty) (no description set)
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
fn test_next_fails_on_branching_children() {
    // TODO(#2126): Fix this behavior
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    // Create a main branch for this test
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    // Create a branching child.
    test_env.jj_cmd_ok(&repo_path, &["branch", "c", "into-the-future"]);
    test_env.jj_cmd_ok(&repo_path, &["co", "into-the-future"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "42"]);
    test_env.jj_cmd_ok(&repo_path, &["co", "main"]);
    // Make the default branch have two possible children.
    test_env.jj_cmd_ok(&repo_path, &["new", "into-the-future", "@-"]);
    // Try to advance the working copy commit.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["next"]);
    insta::assert_snapshot!(stderr,@r###"
    Error: Cannot run `jj next` on a merge commit
    "###);
}

#[test]
fn test_prev_fails_on_multiple_parents() {
    // TODO(#2126): Fix this behavior
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "c", "all-about"]);
    test_env.jj_cmd_ok(&repo_path, &["co", "all-about"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "soname"]);
    // Create a merge commit, which has two parents.
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "@--"]);
    // We have more than one parent, prev fails.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["prev"]);
    insta::assert_snapshot!(stderr,@r###"
    Error: Cannot run `jj prev` on a merge commit
    "###);
}

#[test]
fn test_prev_onto_root_fails() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "second"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "third"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "fourth"]);
    // The root commit is before "first".
    let stderr = test_env.jj_cmd_failure(&repo_path, &["prev", "6"]);
    insta::assert_snapshot!(stderr,@r###"
    Error: No ancestor found 6 commits back
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
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["prev", "--edit"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: zsuskuln 009f88bf (empty) fourth
    Parent commit      : kkmpptxz 3fa8931e (empty) third
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
    test_env.jj_cmd_ok(&repo_path, &["edit", "@--"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["next", "--edit"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: zsuskuln 009f88bf (empty) fourth
    Parent commit      : kkmpptxz 3fa8931e (empty) third
    "###);
}
