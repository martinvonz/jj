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
fn test_rewrite_immutable_generic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
    @  mzvwutvl test.user@example.com 2001-02-03 08:05:12 78ebd449
    │  c
    │ ◉  kkmpptxz test.user@example.com 2001-02-03 08:05:10 main c8d4c7ca
    ├─╯  b
    ◉  qpvuntsm test.user@example.com 2001-02-03 08:05:08 46a8dc51
    │  a
    ◉  zzzzzzzz root() 00000000
    "###);

    // Cannot rewrite a commit in the configured set
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit c8d4c7ca95d0 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // Cannot rewrite an ancestor of the configured set
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "main-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 46a8dc5175be is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // Cannot rewrite the root commit even with an empty set of immutable commits
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
    "###);

    // Error mutating the repo if immutable_heads() uses a ref that can't be
    // resolved
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "branch_that_does_not_exist""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: Invalid `revset-aliases.immutable_heads()`
    Caused by: Revision "branch_that_does_not_exist" doesn't exist
    For help, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
    "###);

    // Can use --ignore-immutable to override
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["--ignore-immutable", "edit", "main"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz c8d4c7ca main | b
    Parent commit      : qpvuntsm 46a8dc51 a
    Added 0 files, modified 1 files, removed 0 files
    "###);
    // ... but not the root commit
    let stderr = test_env.jj_cmd_failure(&repo_path, &["--ignore-immutable", "edit", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
    "###);

    // Mutating the repo works if ref is wrapped in present()
    test_env.add_config(
        r#"revset-aliases."immutable_heads()" = "present(branch_that_does_not_exist)""#,
    );
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["new", "main"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: wqnwkozp de8b93b4 (empty) (no description set)
    Parent commit      : kkmpptxz c8d4c7ca main | b
    "###);

    // immutable_heads() of different arity doesn't shadow the 0-ary one
    test_env.add_config(r#"revset-aliases."immutable_heads(foo)" = "none()""#);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
    "###);
}

#[test]
fn test_new_wc_commit_when_wc_immutable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init"]);
    test_env.jj_cmd_ok(test_env.env_root(), &["branch", "create", "main"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);
    test_env.jj_cmd_ok(test_env.env_root(), &["new", "-m=a"]);
    let (_, stderr) = test_env.jj_cmd_ok(test_env.env_root(), &["branch", "set", "main"]);
    insta::assert_snapshot!(stderr, @r###"
Warning: The working-copy commit in workspace 'default' became immutable, so a new commit has been created on top of it.
Working copy now at: zsuskuln 87e33403 (empty) (no description set)
Parent commit      : kkmpptxz 7272528e main | (empty) a
    "###);
}

#[test]
fn test_immutable_heads_set_to_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init"]);
    test_env.jj_cmd_ok(test_env.env_root(), &["branch", "create", "main"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "@""#);
    let (_, stderr) = test_env.jj_cmd_ok(test_env.env_root(), &["new", "-m=a"]);
    insta::assert_snapshot!(stderr, @r###"
Warning: The working-copy commit in workspace 'default' became immutable, so a new commit has been created on top of it.
Working copy now at: pmmvwywv 09dafa31 (empty) (no description set)
Parent commit      : kkmpptxz 4963e243 (empty) a
    "###);
}

#[test]
fn test_new_wc_commit_when_wc_immutable_multi_workspace() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init"]);
    test_env.jj_cmd_ok(test_env.env_root(), &["branch", "create", "main"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);
    test_env.jj_cmd_ok(test_env.env_root(), &["new", "-m=a"]);
    test_env.jj_cmd_ok(test_env.env_root(), &["workspace", "add", "workspace1"]);
    let workspace1_envroot = test_env.env_root().join("workspace1");
    test_env.jj_cmd_ok(workspace1_envroot.as_path(), &["edit", "default@"]);
    let (_, stderr) = test_env.jj_cmd_ok(test_env.env_root(), &["branch", "set", "main"]);
    insta::assert_snapshot!(stderr, @r###"
Warning: The working-copy commit in workspace 'default' became immutable, so a new commit has been created on top of it.
Warning: The working-copy commit in workspace 'workspace1' became immutable, so a new commit has been created on top of it.
Working copy now at: royxmykx c37fd624 (empty) (no description set)
Parent commit      : kkmpptxz ada0ee19 main | a
    "###);
    test_env.jj_cmd_ok(workspace1_envroot.as_path(), &["workspace", "update-stale"]);
    let (stdout, _) = test_env.jj_cmd_ok(workspace1_envroot.as_path(), &["log", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
nppvrztz test.user@example.com 2001-02-03 08:05:11 workspace1@ f5e1b845
(empty) (no description set)
royxmykx test.user@example.com 2001-02-03 08:05:12 default@ c37fd624
(empty) (no description set)
kkmpptxz test.user@example.com 2001-02-03 08:05:12 main ada0ee19
a
zzzzzzzz root() 00000000
        "###);
}

#[test]
fn test_rewrite_immutable_commands() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    std::fs::write(repo_path.join("file"), "a").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=b"]);
    std::fs::write(repo_path.join("file"), "b").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m=c"]);
    std::fs::write(repo_path.join("file"), "c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "all:visible_heads()", "-m=merge"]);
    // Create another file to make sure the merge commit isn't empty (to satisfy `jj
    // split`) and still has a conflict (to satisfy `jj resolve`).
    std::fs::write(repo_path.join("file2"), "merged").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "description(b)"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "main""#);
    test_env.add_config(r#"revset-aliases."trunk()" = "main""#);

    // Log shows mutable commits, their parents, and trunk() by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  yqosqzyt test.user@example.com 2001-02-03 08:05:13 3f89addf
    │  (empty) (no description set)
    │ ◉  mzvwutvl test.user@example.com 2001-02-03 08:05:12 main 3e025082 conflict
    ╭─┤  merge
    │ │
    │ ~
    │
    ◉  kkmpptxz test.user@example.com 2001-02-03 08:05:10 c8d4c7ca
    │  b
    ~
    "###);

    // abandon
    let stderr = test_env.jj_cmd_failure(&repo_path, &["abandon", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // chmod
    let stderr = test_env.jj_cmd_failure(&repo_path, &["chmod", "-r=main", "x", "file"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // describe
    let stderr = test_env.jj_cmd_failure(&repo_path, &["describe", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // diffedit
    let stderr = test_env.jj_cmd_failure(&repo_path, &["diffedit", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // edit
    let stderr = test_env.jj_cmd_failure(&repo_path, &["edit", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // move --from
    let stderr = test_env.jj_cmd_failure(&repo_path, &["move", "--from=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // move --to
    let stderr = test_env.jj_cmd_failure(&repo_path, &["move", "--to=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj move` is deprecated; use `jj squash` instead, which is equivalent
    Warning: `jj move` will be removed in a future version, and this will be a hard error
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // new --insert-before
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "--insert-before", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // new --insert-after parent_of_main
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "--insert-after", "description(b)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // parallelize
    let stderr = test_env.jj_cmd_failure(&repo_path, &["parallelize", "description(b)", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // rebase -s
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s=main", "-d=@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // rebase -b
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-b=main", "-d=@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 6e11f430f297 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // rebase -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-r=main", "-d=@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // resolve
    let stderr = test_env.jj_cmd_failure(&repo_path, &["resolve", "-r=description(merge)", "file"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // restore -c
    let stderr = test_env.jj_cmd_failure(&repo_path, &["restore", "-c=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // restore --to
    let stderr = test_env.jj_cmd_failure(&repo_path, &["restore", "--to=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // split
    let stderr = test_env.jj_cmd_failure(&repo_path, &["split", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // squash -r
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash", "-r=description(b)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit c8d4c7ca95d0 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // squash --from
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash", "--from=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // squash --into
    let stderr = test_env.jj_cmd_failure(&repo_path, &["squash", "--into=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    // unsquash
    let stderr = test_env.jj_cmd_failure(&repo_path, &["unsquash", "-r=main"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Commit 3e0250828ca5 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
}
