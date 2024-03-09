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

use crate::common::TestEnvironment;

#[test]
fn test_report_conflicts() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "A\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m=A"]);
    std::fs::write(repo_path.join("file"), "B\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m=B"]);
    std::fs::write(repo_path.join("file"), "C\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m=C"]);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(B)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    New conflicts appeared in these commits:
      kkmpptxz 9baab11e (conflict) C
      rlvkpnrz de73196a (conflict) B
    To resolve the conflicts, start by updating to the first one:
      jj new rlvkpnrzqnoo
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln 7dc9bf15 (conflict) (empty) (no description set)
    Parent commit      : kkmpptxz 9baab11e (conflict) C
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=description(A)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Existing conflicts were resolved or abandoned from these commits:
      kkmpptxz hidden 9baab11e (conflict) C
      rlvkpnrz hidden de73196a (conflict) B
    Working copy now at: zsuskuln 355a2e34 (empty) (no description set)
    Parent commit      : kkmpptxz ed071401 C
    Added 0 files, modified 1 files, removed 0 files
    "###);

    // Can get hint about multiple root commits
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r=description(B)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Also rebased 2 descendant commits onto parent of rebased commit
    New conflicts appeared in these commits:
      rlvkpnrz e93270ab (conflict) B
      kkmpptxz 4f0eeaa6 (conflict) C
    To resolve the conflicts, start by updating to one of the first ones:
      jj new rlvkpnrzqnoo
      jj new kkmpptxzrspx
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln 83074dac (conflict) (empty) (no description set)
    Parent commit      : kkmpptxz 4f0eeaa6 (conflict) C
    Added 0 files, modified 1 files, removed 0 files
    "###);

    // Resolve one of the conflicts by (mostly) following the instructions
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["new", "rlvkpnrzqnoo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv 2ec0b4c3 (conflict) (empty) (no description set)
    Parent commit      : rlvkpnrz e93270ab (conflict) B
    Added 0 files, modified 1 files, removed 0 files
    "###);
    std::fs::write(repo_path.join("file"), "resolved\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Existing conflicts were resolved or abandoned from these commits:
      rlvkpnrz hidden e93270ab (conflict) B
    Working copy now at: yostqsxw 8e160bc4 (empty) (no description set)
    Parent commit      : rlvkpnrz c5319490 B
    "###);
}

#[test]
fn test_report_conflicts_with_divergent_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=A"]);
    std::fs::write(repo_path.join("file"), "A\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=B"]);
    std::fs::write(repo_path.join("file"), "B\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=C"]);
    std::fs::write(repo_path.join("file"), "C\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=C2"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=C3", "--at-op=@-"]);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(B)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    Rebased 3 commits
    New conflicts appeared in these commits:
      zsuskuln?? 94be9a4c (conflict) C3
      zsuskuln?? cdae4322 (conflict) C2
      kkmpptxz b76d6a88 (conflict) B
    To resolve the conflicts, start by updating to the first one:
      jj new kkmpptxzrspx
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln?? cdae4322 (conflict) C2
    Parent commit      : kkmpptxz b76d6a88 (conflict) B
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=description(A)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden 94be9a4c (conflict) C3
      zsuskuln hidden cdae4322 (conflict) C2
      kkmpptxz hidden b76d6a88 (conflict) B
    Working copy now at: zsuskuln?? 9c33e9a9 C2
    Parent commit      : kkmpptxz 9ce42c2a B
    Added 0 files, modified 1 files, removed 0 files
    "###);

    // Same thing when rebasing the divergent commits one at a time
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(C2)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    New conflicts appeared in these commits:
      zsuskuln?? 33752e7e (conflict) C2
    To resolve the conflicts, start by updating to it:
      jj new zsuskulnrvyr
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln?? 33752e7e (conflict) C2
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(C3)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    New conflicts appeared in these commits:
      zsuskuln?? 37bb9c2f (conflict) C3
    To resolve the conflicts, start by updating to it:
      jj new zsuskulnrvyr
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-s=description(C2)", "-d=description(B)"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden 33752e7e (conflict) C2
    Working copy now at: zsuskuln?? 24f79296 C2
    Parent commit      : kkmpptxz 9ce42c2a B
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-s=description(C3)", "-d=description(B)"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden 37bb9c2f (conflict) C3
    "###);
}
