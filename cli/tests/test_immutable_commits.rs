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
fn test_rewrite_immutable_generic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    std::fs::write(repo_path.join("file"), "a").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=b"]);
    std::fs::write(repo_path.join("file"), "b").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "main-", "-m=c"]);
    std::fs::write(repo_path.join("file"), "c").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  mzvwutvl test.user@example.com 2001-02-03 04:05:12.000 +07:00 78ebd449
    │  c
    │ ◉  kkmpptxz test.user@example.com 2001-02-03 04:05:10.000 +07:00 main c8d4c7ca
    ├─╯  b
    ◉  qpvuntsm test.user@example.com 2001-02-03 04:05:08.000 +07:00 46a8dc51
    │  a
    ◉  zzzzzzzz root() 00000000
    "###);

    // Cannot rewrite a commit in the configured set
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit c8d4c7ca95d0 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // Cannot rewrite an ancestor of the configured set
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "main-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 46a8dc5175be is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // Cannot rewrite the root commit even with an empty set of immutable commits
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 000000000000 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // Error if we redefine immutable_heads() with an argument
    // TODO: This error comes from the built-in definition of
    // `revsets.short-prefixes`. That's not clear to the user.
    test_env.add_config(r#"revset-aliases."immutable_heads(foo)" = "none()""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: Invalid `revsets.short-prefixes`:  --> 1:31
      |
    1 | @ | ancestors(immutable_heads().., 2) | heads(immutable_heads())
      |                               ^
      |
      = Invalid arguments to revset function "immutable_heads": Expected 1 arguments
    For help, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
    "###);
    // ... even if we also update the built-in call sites
    test_env.add_config(r#"revsets.short-prefixes = "immutable_heads(root())""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The `revset-aliases.immutable_heads()` function must be declared without arguments.
    "###);
}

#[test]
fn test_rewrite_immutable_commands() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    std::fs::write(repo_path.join("file"), "a").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=b"]);
    std::fs::write(repo_path.join("file"), "b").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m=c"]);
    std::fs::write(repo_path.join("file"), "c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "all:visible_heads()", "-m=merge"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "description(b)"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);

    // Log shows mutable commits and immutable heads by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  yqosqzyt test.user@example.com 2001-02-03 04:05:13.000 +07:00 3f89addf
    │  (empty) (no description set)
    │ ◉  mzvwutvl test.user@example.com 2001-02-03 04:05:11.000 +07:00 main 16ca9d80 conflict
    ╭─┤  (empty) merge
    │ │
    │ ~
    │
    ◉  kkmpptxz test.user@example.com 2001-02-03 04:05:10.000 +07:00 c8d4c7ca
    │  b
    ~
    "###);

    // abandon
    let stderr = test_env.jj_cmd_failure(&repo_path, &["abandon", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // chmod
    let stderr = test_env.jj_cmd_failure(&repo_path, &["chmod", "-r=main", "x", "file"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // describe
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // diffedit
    let stderr = test_env.jj_cmd_failure(&repo_path, &["diffedit", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // edit
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // move --from
    let stderr = test_env.jj_cmd_failure(&repo_path, &["move", "--from=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // move --to
    let stderr = test_env.jj_cmd_failure(&repo_path, &["move", "--to=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // rebase -s
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s=main", "-d=@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // rebase -b
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-b=main", "-d=@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 6e11f430f297 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // rebase -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r=main", "-d=@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // resolve
    let stderr = test_env.jj_cmd_failure(&repo_path, &["resolve", "-r=description(merge)", "file"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // restore -c
    let stderr = test_env.jj_cmd_failure(&repo_path, &["restore", "-c=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // restore --to
    let stderr = test_env.jj_cmd_failure(&repo_path, &["restore", "--to=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // split
    let stderr = test_env.jj_cmd_failure(&repo_path, &["split", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // squash
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // unsquash
    let stderr = test_env.jj_cmd_failure(&repo_path, &["unsquash", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 16ca9d800b08 is immutable
    Hint: Configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
}
