// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

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
      kkmpptxz a2593769 (conflict) C
      rlvkpnrz 727244df (conflict) B
    To resolve the conflicts, start by updating to the first one:
      jj new rlvkpnrzqnoo
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln 30928080 (conflict) (empty) (no description set)
    Parent commit      : kkmpptxz a2593769 (conflict) C
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=description(A)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Existing conflicts were resolved or abandoned from these commits:
      kkmpptxz hidden a2593769 (conflict) C
      rlvkpnrz hidden 727244df (conflict) B
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
      rlvkpnrz 9df65f08 (conflict) B
      kkmpptxz 7530822d (conflict) C
    To resolve the conflicts, start by updating to one of the first ones:
      jj new rlvkpnrzqnoo
      jj new kkmpptxzrspx
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln 203be58b (conflict) (empty) (no description set)
    Parent commit      : kkmpptxz 7530822d (conflict) C
    Added 0 files, modified 1 files, removed 0 files
    "###);

    // Resolve one of the conflicts by (mostly) following the instructions
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["new", "rlvkpnrzqnoo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: vruxwmqv 406f84d0 (conflict) (empty) (no description set)
    Parent commit      : rlvkpnrz 9df65f08 (conflict) B
    Added 0 files, modified 1 files, removed 0 files
    "###);
    std::fs::write(repo_path.join("file"), "resolved\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Existing conflicts were resolved or abandoned from these commits:
      rlvkpnrz hidden 9df65f08 (conflict) B
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
      zsuskuln?? 76c40a95 (conflict) C3
      zsuskuln?? e92329f2 (conflict) C2
      kkmpptxz aed319ec (conflict) B
    To resolve the conflicts, start by updating to the first one:
      jj new kkmpptxzrspx
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln?? e92329f2 (conflict) C2
    Parent commit      : kkmpptxz aed319ec (conflict) B
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-d=description(A)"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 commits
    Existing conflicts were resolved or abandoned from these commits:
      zsuskuln hidden 76c40a95 (conflict) C3
      zsuskuln hidden e92329f2 (conflict) C2
      kkmpptxz hidden aed319ec (conflict) B
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
      zsuskuln?? 0d6cb6b7 (conflict) C2
    To resolve the conflicts, start by updating to it:
      jj new zsuskulnrvyr
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: zsuskuln?? 0d6cb6b7 (conflict) C2
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["rebase", "-s=description(C3)", "-d=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits
    New conflicts appeared in these commits:
      zsuskuln?? 9652a362 (conflict) C3
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
      zsuskuln hidden 0d6cb6b7 (conflict) C2
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
      zsuskuln hidden 9652a362 (conflict) C3
    "###);
}
