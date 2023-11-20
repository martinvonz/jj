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

pub mod common;

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
