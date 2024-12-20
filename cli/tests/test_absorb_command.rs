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
fn test_absorb_simple() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m0"]);
    std::fs::write(repo_path.join("file1"), "").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m2"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n2a\n2b\n").unwrap();

    // Empty commit
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @"Nothing changed.");

    // Insert first and last lines
    std::fs::write(repo_path.join("file1"), "1X\n1a\n1b\n2a\n2b\n2Z\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      zsuskuln 3027ca7a 2
      kkmpptxz d0f1e8dd 1
    Working copy now at: yqosqzyt 277bed24 (empty) (no description set)
    Parent commit      : zsuskuln 3027ca7a 2
    ");

    // Modify middle line in hunk
    std::fs::write(repo_path.join("file1"), "1X\n1A\n1b\n2a\n2b\n2Z\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      kkmpptxz d366d92c 1
    Rebased 1 descendant commits.
    Working copy now at: vruxwmqv 32eb72fe (empty) (no description set)
    Parent commit      : zsuskuln 5bf0bc06 2
    ");

    // Remove middle line from hunk
    std::fs::write(repo_path.join("file1"), "1X\n1A\n1b\n2a\n2Z\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      zsuskuln 6e2c4777 2
    Working copy now at: yostqsxw 4a48490c (empty) (no description set)
    Parent commit      : zsuskuln 6e2c4777 2
    ");

    // Insert ambiguous line in between
    std::fs::write(repo_path.join("file1"), "1X\n1A\n1b\nY\n2a\n2Z\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @"Nothing changed.");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @  yostqsxw 80965bcc (no description set)
    │  diff --git a/file1 b/file1
    │  index 8653ca354d..88eb438902 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -1,5 +1,6 @@
    │   1X
    │   1A
    │   1b
    │  +Y
    │   2a
    │   2Z
    ○  zsuskuln 6e2c4777 2
    │  diff --git a/file1 b/file1
    │  index ed237b5112..8653ca354d 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -1,3 +1,5 @@
    │   1X
    │   1A
    │   1b
    │  +2a
    │  +2Z
    ○  kkmpptxz d366d92c 1
    │  diff --git a/file1 b/file1
    │  index e69de29bb2..ed237b5112 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -0,0 +1,3 @@
    │  +1X
    │  +1A
    │  +1b
    ○  qpvuntsm 1a4edb91 0
    │  diff --git a/file1 b/file1
    ~  new file mode 100644
       index 0000000000..e69de29bb2
    ");
    insta::assert_snapshot!(get_evolog(&test_env, &repo_path, "description(1)"), @r"
    ○    kkmpptxz d366d92c 1
    ├─╮
    │ ○  yqosqzyt hidden c506fbc7 (no description set)
    │ ○  yqosqzyt hidden 277bed24 (empty) (no description set)
    ○    kkmpptxz hidden d0f1e8dd 1
    ├─╮
    │ ○  mzvwutvl hidden 8935ee61 (no description set)
    │ ○  mzvwutvl hidden 2bc3d2ce (empty) (no description set)
    ○  kkmpptxz hidden ee76d790 1
    ○  kkmpptxz hidden 677e62d5 (empty) 1
    ");
    insta::assert_snapshot!(get_evolog(&test_env, &repo_path, "description(2)"), @r"
    ○    zsuskuln 6e2c4777 2
    ├─╮
    │ ○  vruxwmqv hidden 7b1da5cd (no description set)
    │ ○  vruxwmqv hidden 32eb72fe (empty) (no description set)
    ○  zsuskuln hidden 5bf0bc06 2
    ○    zsuskuln hidden 3027ca7a 2
    ├─╮
    │ ○  mzvwutvl hidden 8935ee61 (no description set)
    │ ○  mzvwutvl hidden 2bc3d2ce (empty) (no description set)
    ○  zsuskuln hidden cca09b4d 2
    ○  zsuskuln hidden 7b092471 (empty) 2
    ");
}

#[test]
fn test_absorb_replace_single_line_hunk() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m2"]);
    std::fs::write(repo_path.join("file1"), "2a\n1a\n2b\n").unwrap();

    // Replace single-line hunk, which produces a conflict right now. If our
    // merge logic were based on interleaved delta, the hunk would be applied
    // cleanly.
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "2a\n1A\n2b\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      qpvuntsm 7e885236 (conflict) 1
    Rebased 1 descendant commits.
    Working copy now at: mzvwutvl e9c3b95b (empty) (no description set)
    Parent commit      : kkmpptxz 7c36845c 2
    New conflicts appeared in these commits:
      qpvuntsm 7e885236 (conflict) 1
    To resolve the conflicts, start by updating to it:
      jj new qpvuntsm
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @  mzvwutvl e9c3b95b (empty) (no description set)
    ○  kkmpptxz 7c36845c 2
    │  diff --git a/file1 b/file1
    │  index 0000000000..2f87e8e465 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -1,10 +1,3 @@
    │  -<<<<<<< Conflict 1 of 1
    │  -%%%%%%% Changes from base to side #1
    │  --2a
    │  - 1a
    │  --2b
    │  -+++++++ Contents of side #2
    │   2a
    │   1A
    │   2b
    │  ->>>>>>> Conflict 1 of 1 ends
    ×  qpvuntsm 7e885236 (conflict) 1
    │  diff --git a/file1 b/file1
    ~  new file mode 100644
       index 0000000000..0000000000
       --- /dev/null
       +++ b/file1
       @@ -0,0 +1,10 @@
       +<<<<<<< Conflict 1 of 1
       +%%%%%%% Changes from base to side #1
       +-2a
       + 1a
       +-2b
       ++++++++ Contents of side #2
       +2a
       +1A
       +2b
       +>>>>>>> Conflict 1 of 1 ends
    ");
}

#[test]
fn test_absorb_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m0"]);
    std::fs::write(repo_path.join("file1"), "0a\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n0a\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m2", "description(0)"]);
    std::fs::write(repo_path.join("file1"), "0a\n2a\n2b\n").unwrap();

    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["new", "-m3", "description(1)", "description(2)"],
    );
    insta::assert_snapshot!(stderr, @r"
    Working copy now at: mzvwutvl 08898161 (empty) 3
    Parent commit      : kkmpptxz 7e9df299 1
    Parent commit      : zsuskuln baf056cf 2
    Added 0 files, modified 1 files, removed 0 files
    ");

    // Modify first and last lines, absorb from merge
    std::fs::write(repo_path.join("file1"), "1A\n1b\n0a\n2a\n2B\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      zsuskuln 71d1ee56 2
      kkmpptxz 4d379399 1
    Rebased 1 descendant commits.
    Working copy now at: mzvwutvl 9db19b54 (empty) 3
    Parent commit      : kkmpptxz 4d379399 1
    Parent commit      : zsuskuln 71d1ee56 2
    ");

    // Add hunk to merge revision
    std::fs::write(repo_path.join("file2"), "3a\n").unwrap();

    // Absorb into merge
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file2"), "3A\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      mzvwutvl e93c0210 3
    Working copy now at: vruxwmqv 1b10dfa4 (empty) (no description set)
    Parent commit      : mzvwutvl e93c0210 3
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @  vruxwmqv 1b10dfa4 (empty) (no description set)
    ○    mzvwutvl e93c0210 3
    ├─╮  diff --git a/file2 b/file2
    │ │  new file mode 100644
    │ │  index 0000000000..44442d2d7b
    │ │  --- /dev/null
    │ │  +++ b/file2
    │ │  @@ -0,0 +1,1 @@
    │ │  +3A
    │ ○  zsuskuln 71d1ee56 2
    │ │  diff --git a/file1 b/file1
    │ │  index eb6e8821f1..4907935b9f 100644
    │ │  --- a/file1
    │ │  +++ b/file1
    │ │  @@ -1,1 +1,3 @@
    │ │   0a
    │ │  +2a
    │ │  +2B
    ○ │  kkmpptxz 4d379399 1
    ├─╯  diff --git a/file1 b/file1
    │    index eb6e8821f1..902dd8ef13 100644
    │    --- a/file1
    │    +++ b/file1
    │    @@ -1,1 +1,3 @@
    │    +1A
    │    +1b
    │     0a
    ○  qpvuntsm 3777b700 0
    │  diff --git a/file1 b/file1
    ~  new file mode 100644
       index 0000000000..eb6e8821f1
       --- /dev/null
       +++ b/file1
       @@ -0,0 +1,1 @@
       +0a
    ");
}

#[test]
fn test_absorb_discardable_merge_with_descendant() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m0"]);
    std::fs::write(repo_path.join("file1"), "0a\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n0a\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m2", "description(0)"]);
    std::fs::write(repo_path.join("file1"), "0a\n2a\n2b\n").unwrap();

    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["new", "description(1)", "description(2)"]);
    insta::assert_snapshot!(stderr, @r"
    Working copy now at: mzvwutvl f59b2364 (empty) (no description set)
    Parent commit      : kkmpptxz 7e9df299 1
    Parent commit      : zsuskuln baf056cf 2
    Added 0 files, modified 1 files, removed 0 files
    ");

    // Modify first and last lines in the merge commit
    std::fs::write(repo_path.join("file1"), "1A\n1b\n0a\n2a\n2B\n").unwrap();
    // Add new commit on top
    test_env.jj_cmd_ok(&repo_path, &["new", "-m3"]);
    std::fs::write(repo_path.join("file2"), "3a\n").unwrap();
    // Then absorb the merge commit
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb", "--from=@-"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      zsuskuln 02668cf6 2
      kkmpptxz fcabe394 1
    Rebased 1 descendant commits.
    Working copy now at: royxmykx f04f1247 3
    Parent commit      : kkmpptxz fcabe394 1
    Parent commit      : zsuskuln 02668cf6 2
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @    royxmykx f04f1247 3
    ├─╮  diff --git a/file2 b/file2
    │ │  new file mode 100644
    │ │  index 0000000000..31cd755d20
    │ │  --- /dev/null
    │ │  +++ b/file2
    │ │  @@ -0,0 +1,1 @@
    │ │  +3a
    │ ○  zsuskuln 02668cf6 2
    │ │  diff --git a/file1 b/file1
    │ │  index eb6e8821f1..4907935b9f 100644
    │ │  --- a/file1
    │ │  +++ b/file1
    │ │  @@ -1,1 +1,3 @@
    │ │   0a
    │ │  +2a
    │ │  +2B
    ○ │  kkmpptxz fcabe394 1
    ├─╯  diff --git a/file1 b/file1
    │    index eb6e8821f1..902dd8ef13 100644
    │    --- a/file1
    │    +++ b/file1
    │    @@ -1,1 +1,3 @@
    │    +1A
    │    +1b
    │     0a
    ○  qpvuntsm 3777b700 0
    │  diff --git a/file1 b/file1
    ~  new file mode 100644
       index 0000000000..eb6e8821f1
       --- /dev/null
       +++ b/file1
       @@ -0,0 +1,1 @@
       +0a
    ");
}

#[test]
fn test_absorb_conflict() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "root()"]);
    std::fs::write(repo_path.join("file1"), "2a\n2b\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["rebase", "-r@", "-ddescription(1)"]);
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 commits onto destination
    Working copy now at: kkmpptxz 74405a07 (conflict) (no description set)
    Parent commit      : qpvuntsm 3619e4e5 1
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file1    2-sided conflict
    New conflicts appeared in these commits:
      kkmpptxz 74405a07 (conflict) (no description set)
    To resolve the conflicts, start by updating to it:
      jj new kkmpptxz
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    "###);

    let conflict_content =
        String::from_utf8(std::fs::read(repo_path.join("file1")).unwrap()).unwrap();
    insta::assert_snapshot!(conflict_content, @r"
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    +1a
    +1b
    +++++++ Contents of side #2
    2a
    2b
    >>>>>>> Conflict 1 of 1 ends
    ");

    // Cannot absorb from conflict
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Warning: Skipping file1: Is a conflict
    Nothing changed.
    ");

    // Cannot absorb from resolved conflict
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "1A\n1b\n2a\n2B\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Warning: Skipping file1: Is a conflict
    Nothing changed.
    ");
}

#[test]
fn test_absorb_file_mode() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["file", "chmod", "x", "file1"]);

    // Modify content and mode
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "1A\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["file", "chmod", "n", "file1"]);

    // Mode change shouldn't be absorbed
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      qpvuntsm 991365da 1
    Rebased 1 descendant commits.
    Working copy now at: zsuskuln 77de368e (no description set)
    Parent commit      : qpvuntsm 991365da 1
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @  zsuskuln 77de368e (no description set)
    │  diff --git a/file1 b/file1
    │  old mode 100755
    │  new mode 100644
    ○  qpvuntsm 991365da 1
    │  diff --git a/file1 b/file1
    ~  new file mode 100755
       index 0000000000..268de3f3ec
       --- /dev/null
       +++ b/file1
       @@ -0,0 +1,1 @@
       +1A
    ");
}

#[test]
fn test_absorb_from_into() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n1c\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m2"]);
    std::fs::write(repo_path.join("file1"), "1a\n2a\n1b\n1c\n2b\n").unwrap();

    // Line "X" and "Z" have unambiguous adjacent line within the destinations
    // range. Line "Y" doesn't have such line.
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "1a\nX\n2a\n1b\nY\n1c\n2b\nZ\n").unwrap();
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb", "--into=@-"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      kkmpptxz 91df4543 2
    Rebased 1 descendant commits.
    Working copy now at: zsuskuln d5424357 (no description set)
    Parent commit      : kkmpptxz 91df4543 2
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "@-::"), @r"
    @  zsuskuln d5424357 (no description set)
    │  diff --git a/file1 b/file1
    │  index faf62af049..c2d0b12547 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -2,6 +2,7 @@
    │   X
    │   2a
    │   1b
    │  +Y
    │   1c
    │   2b
    │   Z
    ○  kkmpptxz 91df4543 2
    │  diff --git a/file1 b/file1
    ~  index 352e9b3794..faf62af049 100644
       --- a/file1
       +++ b/file1
       @@ -1,3 +1,7 @@
        1a
       +X
       +2a
        1b
        1c
       +2b
       +Z
    ");

    // Absorb all lines from the working-copy parent. An empty commit won't be
    // discarded because "absorb" isn't a command to squash commit descriptions.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb", "--from=@-"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      rlvkpnrz 3a5fd02e 1
    Rebased 2 descendant commits.
    Working copy now at: zsuskuln 53ce490b (no description set)
    Parent commit      : kkmpptxz c94cd773 (empty) 2
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @  zsuskuln 53ce490b (no description set)
    │  diff --git a/file1 b/file1
    │  index faf62af049..c2d0b12547 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -2,6 +2,7 @@
    │   X
    │   2a
    │   1b
    │  +Y
    │   1c
    │   2b
    │   Z
    ○  kkmpptxz c94cd773 (empty) 2
    ○  rlvkpnrz 3a5fd02e 1
    │  diff --git a/file1 b/file1
    │  new file mode 100644
    │  index 0000000000..faf62af049
    │  --- /dev/null
    │  +++ b/file1
    │  @@ -0,0 +1,7 @@
    │  +1a
    │  +X
    │  +2a
    │  +1b
    │  +1c
    │  +2b
    │  +Z
    ○  qpvuntsm 230dd059 (empty) (no description set)
    │
    ~
    ");
}

#[test]
fn test_absorb_paths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n").unwrap();
    std::fs::write(repo_path.join("file2"), "1a\n").unwrap();

    // Modify both files
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "1A\n").unwrap();
    std::fs::write(repo_path.join("file2"), "1A\n").unwrap();

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb", "unknown"]);
    insta::assert_snapshot!(stderr, @"Nothing changed.");

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb", "file1"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      qpvuntsm ae044adb 1
    Rebased 1 descendant commits.
    Working copy now at: kkmpptxz c6f31836 (no description set)
    Parent commit      : qpvuntsm ae044adb 1
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, "mutable()"), @r"
    @  kkmpptxz c6f31836 (no description set)
    │  diff --git a/file2 b/file2
    │  index a8994dc188..268de3f3ec 100644
    │  --- a/file2
    │  +++ b/file2
    │  @@ -1,1 +1,1 @@
    │  -1a
    │  +1A
    ○  qpvuntsm ae044adb 1
    │  diff --git a/file1 b/file1
    ~  new file mode 100644
       index 0000000000..268de3f3ec
       --- /dev/null
       +++ b/file1
       @@ -0,0 +1,1 @@
       +1A
       diff --git a/file2 b/file2
       new file mode 100644
       index 0000000000..a8994dc188
       --- /dev/null
       +++ b/file2
       @@ -0,0 +1,1 @@
       +1a
    ");
}

#[test]
fn test_absorb_immutable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config("revset-aliases.'immutable_heads()' = 'present(main)'");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m1"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new", "-m2"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "-r@-", "main"]);
    std::fs::write(repo_path.join("file1"), "1a\n1b\n2a\n2b\n").unwrap();

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "1A\n1b\n2a\n2B\n").unwrap();

    // Immutable revisions are excluded by default
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["absorb"]);
    insta::assert_snapshot!(stderr, @r"
    Absorbed changes into these revisions:
      kkmpptxz d80e3c2a 2
    Rebased 1 descendant commits.
    Working copy now at: mzvwutvl 3021153d (no description set)
    Parent commit      : kkmpptxz d80e3c2a 2
    ");

    // Immutable revisions shouldn't be rewritten
    let stderr = test_env.jj_cmd_failure(&repo_path, &["absorb", "--into=all()"]);
    insta::assert_snapshot!(stderr, @r"
    Error: Commit 3619e4e52fce is immutable
    Hint: Could not modify commit: qpvuntsm 3619e4e5 main | 1
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    ");

    insta::assert_snapshot!(get_diffs(&test_env, &repo_path, ".."), @r"
    @  mzvwutvl 3021153d (no description set)
    │  diff --git a/file1 b/file1
    │  index 75e4047831..428796ca20 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -1,4 +1,4 @@
    │  -1a
    │  +1A
    │   1b
    │   2a
    │   2B
    ○  kkmpptxz d80e3c2a 2
    │  diff --git a/file1 b/file1
    │  index 8c5268f893..75e4047831 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -1,2 +1,4 @@
    │   1a
    │   1b
    │  +2a
    │  +2B
    ◆  qpvuntsm 3619e4e5 1
    │  diff --git a/file1 b/file1
    ~  new file mode 100644
       index 0000000000..8c5268f893
       --- /dev/null
       +++ b/file1
       @@ -0,0 +1,2 @@
       +1a
       +1b
    ");
}

fn get_diffs(test_env: &TestEnvironment, repo_path: &Path, revision: &str) -> String {
    let template = r#"format_commit_summary_with_refs(self, "") ++ "\n""#;
    test_env.jj_cmd_success(repo_path, &["log", "-r", revision, "-T", template, "--git"])
}

fn get_evolog(test_env: &TestEnvironment, repo_path: &Path, revision: &str) -> String {
    let template = r#"format_commit_summary_with_refs(self, "") ++ "\n""#;
    test_env.jj_cmd_success(repo_path, &["evolog", "-r", revision, "-T", template])
}
