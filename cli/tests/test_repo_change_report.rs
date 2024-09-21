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
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    Working copy now at: zsuskuln aa73e2ae (conflict) (empty) (no description set)
    Parent commit      : kkmpptxz 64bdec0c (conflict) C
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict including 1 deletion
    New conflicts appeared in these commits:
      kkmpptxz 64bdec0c (conflict) C
      rlvkpnrz 10a5fd45 (conflict) B
    To resolve the conflicts, start by updating to the first one:
      jj new rlvkpnrz
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=description(A)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: zsuskuln d70c003d (empty) (no description set)
    Parent commit      : kkmpptxz 43e94449 C
    Added 0 files, modified 1 files, removed 0 files
    Existing conflicts were resolved or abandoned from these commits:
      kkmpptxz hidden 64bdec0c (conflict) C
      rlvkpnrz hidden 10a5fd45 (conflict) B
    "###);

    // Can get hint about multiple root commits
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-r=description(B)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Rebased 2 descendant commits
    Working copy now at: zsuskuln 99fb9018 (conflict) (empty) (no description set)
    Parent commit      : kkmpptxz 17c72220 (conflict) C
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict
    New conflicts appeared in these commits:
      kkmpptxz 17c72220 (conflict) C
      rlvkpnrz eb93a73d (conflict) B
    To resolve the conflicts, start by updating to one of the first ones:
      jj new kkmpptxz
      jj new rlvkpnrz
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "###);

    // Resolve one of the conflicts by (mostly) following the instructions
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["new", "rlvkpnrzqnoo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv 1d87c702 (conflict) (empty) (no description set)
    Parent commit      : rlvkpnrz eb93a73d (conflict) B
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict including 1 deletion
    "###);
    std::fs::write(repo_path.join("file"), "resolved\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: yostqsxw f5a0cf8c (empty) (no description set)
    Parent commit      : rlvkpnrz 87370844 B
    Existing conflicts were resolved or abandoned from these commits:
      rlvkpnrz hidden eb93a73d (conflict) B
    "###);
}

#[test]
fn test_report_conflicts_with_divergent_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    Working copy now at: zsuskuln?? 97ce1783 (conflict) C2
    Parent commit      : kkmpptxz eb93a73d (conflict) B
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict including 1 deletion
    New conflicts appeared in these commits:
      zsuskuln?? b535189c (conflict) C3
      zsuskuln?? 97ce1783 (conflict) C2
      kkmpptxz eb93a73d (conflict) B
    To resolve the conflicts, start by updating to the first one:
      jj new kkmpptxz
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=description(A)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Working copy now at: zsuskuln?? f2d7a228 C2
    Parent commit      : kkmpptxz db069a22 B
    Added 0 files, modified 1 files, removed 0 files
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden b535189c (conflict) C3
      zsuskuln hidden 97ce1783 (conflict) C2
      kkmpptxz hidden eb93a73d (conflict) B
    "###);

    // Same thing when rebasing the divergent commits one at a time
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(C2)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    Working copy now at: zsuskuln?? b15416ac (conflict) C2
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict including 1 deletion
    New conflicts appeared in these commits:
      zsuskuln?? b15416ac (conflict) C2
    To resolve the conflicts, start by updating to it:
      jj new zsuskuln
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(C3)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r#"
    Rebased 1 commits
    New conflicts appeared in these commits:
      zsuskuln?? 8cc7fde6 (conflict) C3
    To resolve the conflicts, start by updating to it:
      jj new zsuskuln
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "#);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-s=description(C2)", "-d=description(B)"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    Working copy now at: zsuskuln?? 1f9680bd C2
    Parent commit      : kkmpptxz db069a22 B
    Added 0 files, modified 1 files, removed 0 files
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden b15416ac (conflict) C2
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["rebase", "-s=description(C3)", "-d=description(B)"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden 8cc7fde6 (conflict) C3
    "###);
}
