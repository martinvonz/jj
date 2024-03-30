// Copyright 2022 The Jujutsu Authors
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
fn test_new() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "a new commit"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  4f2d6e0a3482a6a34e4856a4a63869c0df109e79 a new commit
    ◉  5d5c60b2aa96b8dbf55710656c50285c66cdcd74 add a file
    ◉  0000000000000000000000000000000000000000
    "###);

    // Start a new change off of a specific commit (the root commit in this case).
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "off of root", "root()"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  026537ddb96b801b9cb909985d5443aab44616c1 off of root
    │ ◉  4f2d6e0a3482a6a34e4856a4a63869c0df109e79 a new commit
    │ ◉  5d5c60b2aa96b8dbf55710656c50285c66cdcd74 add a file
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);

    // --edit is a no-op
    test_env.jj_cmd_ok(&repo_path, &["new", "--edit", "-m", "yet another commit"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  101cbec5cae8049cb9850a906ef3675631ed48fa yet another commit
    ◉  026537ddb96b801b9cb909985d5443aab44616c1 off of root
    │ ◉  4f2d6e0a3482a6a34e4856a4a63869c0df109e79 a new commit
    │ ◉  5d5c60b2aa96b8dbf55710656c50285c66cdcd74 add a file
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);

    // --edit cannot be used with --no-edit
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["new", "--edit", "B", "--no-edit", "D"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--edit' cannot be used with '--no-edit'

    Usage: jj new <REVISIONS>...

    For more information, try '--help'.
    "###);
}

#[test]
fn test_new_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "add file1"]);
    std::fs::write(repo_path.join("file1"), "a").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "add file2"]);
    std::fs::write(repo_path.join("file2"), "b").unwrap();

    // Create a merge commit
    test_env.jj_cmd_ok(&repo_path, &["new", "main", "@"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    0c4e5b9b68ae0cbe7ce3c61042619513d09005bf
    ├─╮
    │ ◉  f399209d9dda06e8a25a0c8e9a0cde9f421ff35d add file2
    ◉ │  38e8e2f6c92ffb954961fc391b515ff551b41636 add file1
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @"a");
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file2"]);
    insta::assert_snapshot!(stdout, @"b");

    // Same test with `--no-edit`
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["new", "main", "@", "--no-edit"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Created new commit znkkpsqq 200ed1a1 (empty) (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉    200ed1a14c8acf09783dafefe5bebf2ff58f12fd
    ├─╮
    │ @  f399209d9dda06e8a25a0c8e9a0cde9f421ff35d add file2
    ◉ │  38e8e2f6c92ffb954961fc391b515ff551b41636 add file1
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);

    // Same test with `jj merge`
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    test_env.jj_cmd_ok(&repo_path, &["merge", "main", "@"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    3a44e52b073cbb5deb11bb8fa0763a369e96427a
    ├─╮
    │ ◉  f399209d9dda06e8a25a0c8e9a0cde9f421ff35d add file2
    ◉ │  38e8e2f6c92ffb954961fc391b515ff551b41636 add file1
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);

    // `jj merge` with less than two arguments is an error
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["merge"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj merge` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj merge` will be removed in a future version, and this will be a hard error
    Error: Merge requires at least two revisions
    "###);
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["merge", "main"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj merge` is deprecated; use `jj new` instead, which is equivalent
    Warning: `jj merge` will be removed in a future version, and this will be a hard error
    Error: Merge requires at least two revisions
    "###);

    // merge with non-unique revisions
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "@", "3a44e"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: More than one revset resolved to revision 3a44e52b073c
    "###);

    // merge with root
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "@", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The Git backend does not support creating merge commits with the root commit as one of the parents.
    "###);
}

#[test]
fn test_new_insert_after() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    F
    ├─╮
    │ ◉  E
    ◉ │  D
    ├─╯
    │ ◉  C
    │ ◉  B
    │ ◉  A
    ├─╯
    ◉  root
    "###);

    // --insert-after can be repeated (this does not affect the outcome); --after is
    // an alias
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "new",
            "--insert-after",
            "-m",
            "G",
            "--after",
            "B",
            "--after",
            "D",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    Working copy now at: kxryzmor ca7c6481 (empty) G
    Parent commit      : kkmpptxz 6041917c B | (empty) B
    Parent commit      : vruxwmqv c9257eff D | (empty) D
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    ◉  C
    │ ◉  F
    ╭─┤
    @ │    G
    ├───╮
    │ │ ◉  D
    ◉ │ │  B
    ◉ │ │  A
    ├───╯
    │ ◉  E
    ├─╯
    ◉  root
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["new", "--insert-after", "-m", "H", "D"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 3 descendant commits
    Working copy now at: uyznsvlq fcf8281b (empty) H
    Parent commit      : vruxwmqv c9257eff D | (empty) D
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    ◉  C
    │ ◉  F
    ╭─┤
    ◉ │    G
    ├───╮
    │ │ @  H
    │ │ ◉  D
    ◉ │ │  B
    ◉ │ │  A
    ├───╯
    │ ◉  E
    ├─╯
    ◉  root
    "###);

    // --after cannot be used with --before
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["new", "--after", "B", "--before", "D"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--insert-after' cannot be used with '--insert-before'

    Usage: jj new --insert-after <REVISIONS>...

    For more information, try '--help'.
    "###);
}

#[test]
fn test_new_insert_after_children() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    F
    ├─╮
    │ ◉  E
    ◉ │  D
    ├─╯
    │ ◉  C
    │ ◉  B
    │ ◉  A
    ├─╯
    ◉  root
    "###);

    // Check that inserting G after A and C doesn't try to rebase B (which is
    // initially a child of A) onto G as that would create a cycle since B is
    // a parent of C which is a parent G.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["new", "--insert-after", "-m", "G", "A", "C"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kxryzmor b48d4d73 (empty) G
    Parent commit      : qpvuntsm 65b1ef43 A | (empty) A
    Parent commit      : mzvwutvl ec18c57d C | (empty) C
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    G
    ├─╮
    │ ◉  C
    │ ◉  B
    ├─╯
    ◉  A
    │ ◉    F
    │ ├─╮
    │ │ ◉  E
    ├───╯
    │ ◉  D
    ├─╯
    ◉  root
    "###);
}

#[test]
fn test_new_insert_before() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    F
    ├─╮
    │ ◉  E
    ◉ │  D
    ├─╯
    │ ◉  C
    │ ◉  B
    │ ◉  A
    ├─╯
    ◉  root
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["new", "--insert-before", "-m", "G", "C", "F"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 2 descendant commits
    Working copy now at: kxryzmor ff6bbbc7 (empty) G
    Parent commit      : znkkpsqq 41a89ffc E | (empty) E
    Parent commit      : vruxwmqv c9257eff D | (empty) D
    Parent commit      : kkmpptxz 6041917c B | (empty) B
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    ◉  F
    │ ◉  C
    ├─╯
    @      G
    ├─┬─╮
    │ │ ◉  B
    │ │ ◉  A
    │ ◉ │  D
    │ ├─╯
    ◉ │  E
    ├─╯
    ◉  root
    "###);
}

#[test]
fn test_new_insert_before_root_successors() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    F
    ├─╮
    │ ◉  E
    ◉ │  D
    ├─╯
    │ ◉  C
    │ ◉  B
    │ ◉  A
    ├─╯
    ◉  root
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["new", "--insert-before", "-m", "G", "A", "D"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Rebased 5 descendant commits
    Working copy now at: kxryzmor 36541977 (empty) G
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    "###);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    ◉    F
    ├─╮
    │ ◉  E
    ◉ │  D
    │ │ ◉  C
    │ │ ◉  B
    │ │ ◉  A
    ├───╯
    @ │  G
    ├─╯
    ◉  root
    "###);
}

#[test]
fn test_new_insert_before_no_loop() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    let template = r#"commit_id.short() ++ " " ++ if(description, description, "root")"#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @    7705d353bf5d F
    ├─╮
    │ ◉  41a89ffcbba2 E
    ◉ │  c9257eff5bf9 D
    ├─╯
    │ ◉  ec18c57d72d8 C
    │ ◉  6041917ceeb5 B
    │ ◉  65b1ef43c737 A
    ├─╯
    ◉  000000000000 root
    "###);

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["new", "--insert-before", "-m", "G", "A", "C"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to create a loop: commit 6041917ceeb5 would be both an ancestor and a descendant of the new commit
    "###);
}

#[test]
fn test_new_insert_before_no_root_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    F
    ├─╮
    │ ◉  E
    ◉ │  D
    ├─╯
    │ ◉  C
    │ ◉  B
    │ ◉  A
    ├─╯
    ◉  root
    "###);

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["new", "--insert-before", "-m", "G", "B", "D"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The Git backend does not support creating merge commits with the root commit as one of the parents.
    "###);
}

#[test]
fn test_new_insert_before_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    setup_before_insertion(&test_env, &repo_path);
    insta::assert_snapshot!(get_short_log_output(&test_env, &repo_path), @r###"
    @    F
    ├─╮
    │ ◉  E
    ◉ │  D
    ├─╯
    │ ◉  C
    │ ◉  B
    │ ◉  A
    ├─╯
    ◉  root
    "###);

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["new", "--insert-before", "-m", "G", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The root commit 000000000000 is immutable
    "###);
}

#[test]
fn test_new_conflicting_branches() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "one"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "two", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "foo"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--at-op=@-",
            "branch",
            "create",
            "foo",
            "-r",
            r#"description("one")"#,
        ],
    );

    // Trigger resolution of concurrent operations
    test_env.jj_cmd_ok(&repo_path, &["st"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "foo" resolved to more than one revision
    Hint: Branch foo resolved to multiple revisions because it's conflicted.
    It resolved to these revisions:
      kkmpptxz 66c6502d foo?? | (empty) two
      qpvuntsm a9330854 foo?? | (empty) one
    Hint: Set which revision the branch points to with `jj branch set foo -r <REVISION>`.
    "###);
}

#[test]
fn test_new_conflicting_change_ids() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "one"]);
    test_env.jj_cmd_ok(&repo_path, &["--at-op=@-", "describe", "-m", "two"]);

    // Trigger resolution of concurrent operations
    test_env.jj_cmd_ok(&repo_path, &["st"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "qpvuntsm"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "qpvuntsm" resolved to more than one revision
    Hint: The revset "qpvuntsm" resolved to these revisions:
      qpvuntsm?? d2ae6806 (empty) two
      qpvuntsm?? a9330854 (empty) one
    Hint: Some of these commits have the same change id. Abandon one of them with `jj abandon -r <REVISION>`.
    "###);
}

#[test]
fn test_new_error_revision_does_not_exist() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "one"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "two"]);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "this"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "this" doesn't exist
    "###);
}

fn setup_before_insertion(test_env: &TestEnvironment, repo_path: &Path) {
    test_env.jj_cmd_ok(repo_path, &["branch", "create", "A"]);
    test_env.jj_cmd_ok(repo_path, &["commit", "-m", "A"]);
    test_env.jj_cmd_ok(repo_path, &["branch", "create", "B"]);
    test_env.jj_cmd_ok(repo_path, &["commit", "-m", "B"]);
    test_env.jj_cmd_ok(repo_path, &["branch", "create", "C"]);
    test_env.jj_cmd_ok(repo_path, &["describe", "-m", "C"]);
    test_env.jj_cmd_ok(repo_path, &["new", "-m", "D", "root()"]);
    test_env.jj_cmd_ok(repo_path, &["branch", "create", "D"]);
    test_env.jj_cmd_ok(repo_path, &["new", "-m", "E", "root()"]);
    test_env.jj_cmd_ok(repo_path, &["branch", "create", "E"]);
    // Any number of -r's is ignored
    test_env.jj_cmd_ok(repo_path, &["new", "-m", "F", "-r", "D", "-r", "E"]);
    test_env.jj_cmd_ok(repo_path, &["branch", "create", "F"]);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"commit_id ++ " " ++ description"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}

fn get_short_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"if(description, description, "root")"#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}
