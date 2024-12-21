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
fn test_bookmark_multiple_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Created 2 bookmarks pointing to qpvuntsm 230dd059 bar foo | (empty) (no description set)
    Hint: Use -r to specify the target revision.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 230dd059e1b0
    ◆   000000000000
    "###);

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Moved 2 bookmarks to zsuskuln 8bb159bc bar foo | (empty) (no description set)
    Hint: Use -r to specify the target revision.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar foo 8bb159bc30a9
    ○   230dd059e1b0
    ◆   000000000000
    "###);

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "foo", "bar", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Deleted 2 bookmarks.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   8bb159bc30a9
    ○   230dd059e1b0
    ◆   000000000000
    "###);

    // Hint should be omitted if -r is specified
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "-r@-", "foo", "bar"]);
    insta::assert_snapshot!(stderr, @r###"
    Created 2 bookmarks pointing to qpvuntsm 230dd059 bar foo | (empty) (no description set)
    "###);

    // Create and move with explicit -r
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "-r@", "bar", "baz"]);
    insta::assert_snapshot!(stderr, @r###"
    Created 1 bookmarks pointing to zsuskuln 8bb159bc bar baz | (empty) (no description set)
    Moved 1 bookmarks to zsuskuln 8bb159bc bar baz | (empty) (no description set)
    Hint: Consider using `jj bookmark move` if your intention was to move existing bookmarks.
    "###);

    // Noop changes should not be included in the stats
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "-r@", "foo", "bar", "baz"]);
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to zsuskuln 8bb159bc bar baz foo | (empty) (no description set)
    "###);
}

#[test]
fn test_bookmark_at_root() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "fred", "-r=root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Created 1 bookmarks pointing to zzzzzzzz 00000000 fred | (empty) (no description set)
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    Warning: Failed to export some bookmarks:
      fred: Ref cannot point to the root commit in Git
    "###);
}

#[test]
fn test_bookmark_empty_name() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "create", ""]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '<NAMES>...' but none was supplied

    For more information, try '--help'.
    "###);

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "set", ""]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '<NAMES>...' but none was supplied

    For more information, try '--help'.
    "###);
}

#[test]
fn test_bookmark_move() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up remote
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "move", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such bookmark: foo
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Created 1 bookmarks pointing to qpvuntsm 230dd059 foo | (empty) (no description set)
    Hint: Consider using `jj bookmark move` if your intention was to move existing bookmarks.
    "###);

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "create", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Bookmark already exists: foo
    Hint: Use `jj bookmark set` to update it.
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to mzvwutvl 167f90e7 foo | (empty) (no description set)
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "set", "-r@-", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to move bookmark backwards or sideways: foo
    Hint: Use --allow-backwards to allow it.
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "set", "-r@-", "--allow-backwards", "foo"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to qpvuntsm 230dd059 foo | (empty) (no description set)
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "move", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to mzvwutvl 167f90e7 foo | (empty) (no description set)
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "move", "--to=@-", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to move bookmark backwards or sideways: foo
    Hint: Use --allow-backwards to allow it.
    "###);

    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "move", "--to=@-", "--allow-backwards", "foo"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to qpvuntsm 230dd059 foo | (empty) (no description set)
    "###);

    // Delete bookmark locally, but is still tracking remote
    test_env.jj_cmd_ok(&repo_path, &["describe", "@-", "-mcommit"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push", "--allow-new", "-r@-"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "foo"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    foo (deleted)
      @origin: qpvuntsm 1eb845f3 (empty) commit
    "###);

    // Deleted tracking bookmark name should still be allocated
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "create", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Tracked remote bookmarks exist for deleted bookmark: foo
    Hint: Use `jj bookmark set` to recreate the local bookmark. Run `jj bookmark untrack 'glob:foo@*'` to disassociate them.
    "###);

    // Restoring local target shouldn't invalidate tracking state
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to mzvwutvl 66d48752 foo* | (empty) (no description set)
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    foo: mzvwutvl 66d48752 (empty) (no description set)
      @origin (behind by 1 commits): qpvuntsm 1eb845f3 (empty) commit
    "###);

    // Untracked remote bookmark shouldn't block creation of local bookmark
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "untrack", "foo@origin"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "foo"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Created 1 bookmarks pointing to mzvwutvl 66d48752 foo | (empty) (no description set)
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    foo: mzvwutvl 66d48752 (empty) (no description set)
    foo@origin: qpvuntsm 1eb845f3 (empty) commit
    "###);
}

#[test]
fn test_bookmark_move_matching() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a1", "a2"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mhead1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b1"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "c1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mhead2"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   a2781dd9ee37
    ○  c1 f4f38657a3dd
    ○  b1 f652c32197cf
    │ ○   6b5e840ea72b
    │ ○  a1 a2 230dd059e1b0
    ├─╯
    ◆   000000000000
    "###);

    // The default could be considered "--from=all() glob:*", but is disabled
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "move"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      <--from <REVSETS>|NAMES>

    Usage: jj bookmark move <--from <REVSETS>|NAMES>

    For more information, try '--help'.
    "###);

    // No bookmarks pointing to the source revisions
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "move", "--from=none()"]);
    insta::assert_snapshot!(stderr, @r###"
    No bookmarks to update.
    "###);

    // No matching bookmarks within the source revisions
    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["bookmark", "move", "--from=::@", "glob:a?"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No matching bookmarks for patterns: a?
    "###);

    // Noop move
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "move", "--to=a1", "a2"]);
    insta::assert_snapshot!(stderr, @r###"
    No bookmarks to update.
    "###);

    // Move from multiple revisions
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "move", "--from=::@"]);
    insta::assert_snapshot!(stderr, @r###"
    Moved 2 bookmarks to vruxwmqv a2781dd9 b1 c1 | (empty) head2
    Hint: Specify bookmark by name to update just one of the bookmarks.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  b1 c1 a2781dd9ee37
    ○   f4f38657a3dd
    ○   f652c32197cf
    │ ○   6b5e840ea72b
    │ ○  a1 a2 230dd059e1b0
    ├─╯
    ◆   000000000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);

    // Try to move multiple bookmarks, but one of them isn't fast-forward
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "move", "glob:?1"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to move bookmark backwards or sideways: a1
    Hint: Use --allow-backwards to allow it.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   a2781dd9ee37
    ○  c1 f4f38657a3dd
    ○  b1 f652c32197cf
    │ ○   6b5e840ea72b
    │ ○  a1 a2 230dd059e1b0
    ├─╯
    ◆   000000000000
    "###);

    // Select by revision and name
    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "move", "--from=::a1+", "--to=a1+", "glob:?1"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to kkmpptxz 6b5e840e a1 | (empty) head1
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   a2781dd9ee37
    ○  c1 f4f38657a3dd
    ○  b1 f652c32197cf
    │ ○  a1 6b5e840ea72b
    │ ○  a2 230dd059e1b0
    ├─╯
    ◆   000000000000
    "###);
}

#[test]
fn test_bookmark_move_conflicting() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let get_log = || {
        let template = r#"separate(" ", description.first_line(), bookmarks)"#;
        let (stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-T", template]);
        stdout
    };

    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-mA0"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-mB0"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-mC0"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "description(A0)", "-mA1"]);

    // Set up conflicting bookmark.
    test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "create", "-rdescription(A0)", "foo"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "bookmark",
            "create",
            "--at-op=@-",
            "-rdescription(B0)",
            "foo",
        ],
    );
    insta::assert_snapshot!(get_log(), @r###"
    @  A1
    ○  A0 foo??
    │ ○  C0
    ├─╯
    │ ○  B0 foo??
    ├─╯
    ◆
    "###);

    // Can't move the bookmark to C0 since it's sibling.
    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["bookmark", "set", "-rdescription(C0)", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Refusing to move bookmark backwards or sideways: foo
    Hint: Use --allow-backwards to allow it.
    "###);

    // Can move the bookmark to A1 since it's descendant of A0. It's not
    // descendant of B0, though.
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "-rdescription(A1)", "foo"]);
    insta::assert_snapshot!(stderr, @r###"
    Moved 1 bookmarks to mzvwutvl 9328d344 foo | (empty) A1
    "###);
    insta::assert_snapshot!(get_log(), @r###"
    @  A1 foo
    ○  A0
    │ ○  C0
    ├─╯
    │ ○  B0
    ├─╯
    ◆
    "###);
}

#[test]
fn test_bookmark_rename() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up remote
    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "rename", "bnoexist", "blocal"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such bookmark: bnoexist
    "###);

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=commit-0"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "blocal"]);
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "rename", "blocal", "blocal1"]);
    insta::assert_snapshot!(stderr, @"");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=commit-1"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bexist"]);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "rename", "blocal1", "bexist"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Bookmark already exists: bexist
    "###);

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=commit-2"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bremote"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push", "--allow-new", "-b=bremote"]);
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "rename", "bremote", "bremote2"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Tracked remote bookmarks for bookmark bremote were not renamed.
    Hint: To rename the bookmark on the remote, you can `jj git push --bookmark bremote` first (to delete it on the remote), and then `jj git push --bookmark bremote2`. `jj git push --all` would also be sufficient.
    "###);
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "rename", "bremote2", "bremote"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Tracked remote bookmarks for bookmark bremote exist.
    Hint: Run `jj bookmark untrack 'glob:bremote@*'` to disassociate them.
    "###);
}

#[test]
fn test_bookmark_rename_colocated() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo", "--colocate"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=commit-0"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "blocal"]);

    // Make sure that git tracking bookmarks don't cause a warning
    let (_stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "rename", "blocal", "blocal1"]);
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_bookmark_forget_glob() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo-1"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar-2"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo-3"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo-4"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 230dd059e1b0
    ◆   000000000000
    "###);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Forgot 2 bookmarks.
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Forgot 2 bookmarks.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-4 230dd059e1b0
    ◆   000000000000
    "###);

    // Forgetting a bookmark via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "forget", "foo-4", "glob:foo-*", "glob:foo-*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Forgot 1 bookmarks.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 230dd059e1b0
    ◆   000000000000
    "###);

    // Malformed glob
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "forget", "glob:foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'glob:foo-[1-3' for '<NAMES>...': Pattern syntax error near position 4: invalid range pattern

    For more information, try '--help'.
    "###);

    // We get an error if none of the globs match anything
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["bookmark", "forget", "glob:bar*", "glob:baz*", "glob:boom*"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: No matching bookmarks for patterns: baz*, boom*
    "###);
}

#[test]
fn test_bookmark_delete_glob() {
    // Set up a git repo with a bookmark and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=commit"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo-1"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar-2"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo-3"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo-4"]);
    // Push to create remote-tracking bookmarks
    test_env.jj_cmd_ok(&repo_path, &["git", "push", "--all"]);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1 foo-3 foo-4 312a98d6f27b
    ◆   000000000000
    "###);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Deleted 2 bookmarks.
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Deleted 2 bookmarks.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1@origin foo-3@origin foo-4 312a98d6f27b
    ◆   000000000000
    "###);

    // We get an error if none of the globs match live bookmarks. Unlike `jj
    // bookmark forget`, it's not allowed to delete already deleted bookmarks.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "delete", "glob:foo-[1-3]"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No matching bookmarks for patterns: foo-[1-3]
    "###);

    // Deleting a bookmark via both explicit name and glob pattern, or with
    // multiple glob patterns, shouldn't produce an error.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "delete", "foo-4", "glob:foo-*", "glob:foo-*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Deleted 1 bookmarks.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  bar-2 foo-1@origin foo-3@origin foo-4@origin 312a98d6f27b
    ◆   000000000000
    "###);

    // The deleted bookmarks are still there
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    bar-2: qpvuntsm 312a98d6 (empty) commit
      @origin: qpvuntsm 312a98d6 (empty) commit
    foo-1 (deleted)
      @origin: qpvuntsm 312a98d6 (empty) commit
    foo-3 (deleted)
      @origin: qpvuntsm 312a98d6 (empty) commit
    foo-4 (deleted)
      @origin: qpvuntsm 312a98d6 (empty) commit
    "###);

    // Malformed glob
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "delete", "glob:foo-[1-3"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'glob:foo-[1-3' for '<NAMES>...': Pattern syntax error near position 4: invalid range pattern

    For more information, try '--help'.
    "###);

    // Unknown pattern kind
    let stderr =
        test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "forget", "whatever:bookmark"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'whatever:bookmark' for '<NAMES>...': Invalid string pattern kind "whatever:"

    For more information, try '--help'.
    Hint: Try prefixing with one of `exact:`, `glob:`, `regex:`, or `substring:`
    "###);
}

#[test]
fn test_bookmark_delete_export() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "foo"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "list", "--all-remotes"]);
    insta::assert_snapshot!(stdout, @r###"
    foo (deleted)
      @git: rlvkpnrz 65b6b74e (empty) (no description set)
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be deleted from the underlying Git repo on the next `jj git export`.
    "###);

    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    "###);
}

#[test]
fn test_bookmark_forget_export() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    foo: rlvkpnrz 65b6b74e (empty) (no description set)
    "###);

    // Exporting the bookmark to git creates a local-git tracking bookmark
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Forgot 1 bookmarks.
    "###);
    // Forgetting a bookmark deletes local and remote-tracking bookmarks including
    // the corresponding git-tracking bookmark.
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r=foo", "--no-graph"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "foo" doesn't exist
    "###);

    // `jj git export` will delete the bookmark from git. In a colocated repo,
    // this will happen automatically immediately after a `jj bookmark forget`.
    // This is demonstrated in `test_git_colocated_bookmark_forget` in
    // test_git_colocated.rs
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");
}

#[test]
fn test_bookmark_forget_fetched_bookmark() {
    // Much of this test is borrowed from `test_git_fetch_remote_only_bookmark` in
    // test_git_fetch.rs

    // Set up a git repo with a bookmark and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    // Create a commit and a bookmark in the git repo
    let first_git_repo_commit = git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();

    // Fetch normally
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 1: with export-import
    // Forget the bookmark
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "feature1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");

    // At this point `jj git export && jj git import` does *not* recreate the
    // bookmark. This behavior is important in colocated repos, as otherwise a
    // forgotten bookmark would be immediately resurrected.
    //
    // Technically, this is because `jj bookmark forget` preserved
    // the ref in jj view's `git_refs` tracking the local git repo's remote-tracking
    // bookmark.
    // TODO: Show that jj git push is also a no-op
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "import"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");

    // We can fetch feature1 again.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 2: No export/import (otherwise the same as test 1)
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "feature1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");
    // Fetch works even without the export-import
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: mzyxwzks 9f01a0e0 message
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // TEST 3: fetch bookmark that was moved & forgotten

    // Move the bookmark in the git repo.
    git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "another message",
            &tree,
            &[&git_repo.find_commit(first_git_repo_commit).unwrap()],
        )
        .unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "feature1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Forgot 1 bookmarks.
    "###);

    // Fetching a moved bookmark does not create a conflict
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [new] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: ooosovrs 38aefb17 (empty) another message
      @origin: ooosovrs 38aefb17 (empty) another message
    "###);
}

#[test]
fn test_bookmark_forget_deleted_or_nonexistent_bookmark() {
    // Much of this test is borrowed from `test_git_fetch_remote_only_bookmark` in
    // test_git_fetch.rs

    // ======== Beginning of test setup ========
    // Set up a git repo with a bookmark and a jj repo that has it as a remote.
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init_bare(git_repo_path).unwrap();
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    // Create a commit and a bookmark in the git repo
    git_repo
        .commit(
            Some("refs/heads/feature1"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();

    // Fetch and then delete the bookmark
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch", "--remote=origin"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "feature1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1 (deleted)
      @origin: mzyxwzks 9f01a0e0 message
    "###);

    // ============ End of test setup ============

    // We can forget a deleted bookmark
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "forget", "feature1"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @"");

    // Can't forget a non-existent bookmark
    let stderr = test_env.jj_cmd_failure(&repo_path, &["bookmark", "forget", "i_do_not_exist"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such bookmark: i_do_not_exist
    "###);
}

#[test]
fn test_bookmark_track_untrack() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up remote
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    let create_remote_commit = |message: &str, data: &[u8], ref_names: &[&str]| {
        let signature =
            git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0))
                .unwrap();
        let mut tree_builder = git_repo.treebuilder(None).unwrap();
        let file_oid = git_repo.blob(data).unwrap();
        tree_builder
            .insert("file", file_oid, git2::FileMode::Blob.into())
            .unwrap();
        let tree_oid = tree_builder.write().unwrap();
        let tree = git_repo.find_tree(tree_oid).unwrap();
        // Create commit and bookmarks in the remote
        let git_commit_oid = git_repo
            .commit(None, &signature, &signature, message, &tree, &[])
            .unwrap();
        for name in ref_names {
            git_repo.reference(name, git_commit_oid, true, "").unwrap();
        }
    };

    // Fetch new commit without auto tracking. No local bookmarks should be
    // created.
    create_remote_commit(
        "commit 1",
        b"content 1",
        &[
            "refs/heads/main",
            "refs/heads/feature1",
            "refs/heads/feature2",
        ],
    );
    test_env.add_config("git.auto-local-bookmark = false");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [new] untracked
    bookmark: feature2@origin [new] untracked
    bookmark: main@origin     [new] untracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1@origin: sptzoqmo 7b33f629 commit 1
    feature2@origin: sptzoqmo 7b33f629 commit 1
    main@origin: sptzoqmo 7b33f629 commit 1
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @   230dd059e1b0
    │ ◆  feature1@origin feature2@origin main@origin 7b33f6295eda
    ├─╯
    ◆   000000000000
    "#);

    // Track new bookmark. Local bookmark should be created.
    test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "track", "feature1@origin", "main@origin"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    feature2@origin: sptzoqmo 7b33f629 commit 1
    main: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    "###);

    // Track existing bookmark. Local bookmark should result in conflict.
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "feature2"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "track", "feature2@origin"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    feature2 (conflicted):
      + qpvuntsm 230dd059 (empty) (no description set)
      + sptzoqmo 7b33f629 commit 1
      @origin (behind by 1 commits): sptzoqmo 7b33f629 commit 1
    main: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    "###);

    // Untrack existing and locally-deleted bookmarks. Bookmark targets should be
    // unchanged
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "delete", "feature2"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "untrack", "feature1@origin", "feature2@origin"],
    );
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
    feature1@origin: sptzoqmo 7b33f629 commit 1
    feature2@origin: sptzoqmo 7b33f629 commit 1
    main: sptzoqmo 7b33f629 commit 1
      @origin: sptzoqmo 7b33f629 commit 1
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @   230dd059e1b0
    │ ◆  feature1 feature1@origin feature2@origin main 7b33f6295eda
    ├─╯
    ◆   000000000000
    "#);

    // Fetch new commit. Only tracking bookmark "main" should be merged.
    create_remote_commit(
        "commit 2",
        b"content 2",
        &[
            "refs/heads/main",
            "refs/heads/feature1",
            "refs/heads/feature2",
        ],
    );
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [updated] untracked
    bookmark: feature2@origin [updated] untracked
    bookmark: main@origin     [updated] tracked
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
    feature1@origin: mmqqkyyt 40dabdaf commit 2
    feature2@origin: mmqqkyyt 40dabdaf commit 2
    main: mmqqkyyt 40dabdaf commit 2
      @origin: mmqqkyyt 40dabdaf commit 2
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @   230dd059e1b0
    │ ◆  feature1@origin feature2@origin main 40dabdaf4abe
    ├─╯
    │ ○  feature1 7b33f6295eda
    ├─╯
    ◆   000000000000
    "#);

    // Fetch new commit with auto tracking. Tracking bookmark "main" and new
    // bookmark "feature3" should be merged.
    create_remote_commit(
        "commit 3",
        b"content 3",
        &[
            "refs/heads/main",
            "refs/heads/feature1",
            "refs/heads/feature2",
            "refs/heads/feature3",
        ],
    );
    test_env.add_config("git.auto-local-bookmark = true");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [updated] untracked
    bookmark: feature2@origin [updated] untracked
    bookmark: feature3@origin [new] tracked
    bookmark: main@origin     [updated] tracked
    Abandoned 1 commits that are no longer reachable.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: sptzoqmo 7b33f629 commit 1
    feature1@origin: wwnpyzpo 3f0f86fa commit 3
    feature2@origin: wwnpyzpo 3f0f86fa commit 3
    feature3: wwnpyzpo 3f0f86fa commit 3
      @origin: wwnpyzpo 3f0f86fa commit 3
    main: wwnpyzpo 3f0f86fa commit 3
      @origin: wwnpyzpo 3f0f86fa commit 3
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r#"
    @   230dd059e1b0
    │ ◆  feature1@origin feature2@origin feature3 main 3f0f86fa0e57
    ├─╯
    │ ○  feature1 7b33f6295eda
    ├─╯
    ◆   000000000000
    "#);
}

#[test]
fn test_bookmark_track_conflict() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let git_repo_path = test_env.env_root().join("git-repo");
    git2::Repository::init_bare(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "push", "--allow-new", "-b", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "untrack", "main@origin"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "-m", "b", "-r", "main", "--ignore-immutable"],
    );
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "track", "main@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Started tracking 1 remote bookmarks.
    main (conflicted):
      + qpvuntsm e802c4f8 (empty) b
      + qpvuntsm hidden 427890ea (empty) a
      @origin (behind by 1 commits): qpvuntsm hidden 427890ea (empty) a
    "###);
}

#[test]
fn test_bookmark_track_untrack_patterns() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Set up remote
    let git_repo_path = test_env.env_root().join("git-repo");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "remote", "add", "origin", "../git-repo"],
    );

    // Create remote commit
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    // Create commit and bookmarks in the remote
    let git_commit_oid = git_repo
        .commit(None, &signature, &signature, "commit", &tree, &[])
        .unwrap();
    for name in ["refs/heads/feature1", "refs/heads/feature2"] {
        git_repo.reference(name, git_commit_oid, true, "").unwrap();
    }

    // Fetch new commit without auto tracking
    test_env.add_config("git.auto-local-bookmark = false");
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);
    insta::assert_snapshot!(stderr, @r###"
    bookmark: feature1@origin [new] untracked
    bookmark: feature2@origin [new] untracked
    "###);

    // Track local bookmark
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "main"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_cli_error(&repo_path, &["bookmark", "track", "main"]), @r###"
    error: invalid value 'main' for '<BOOKMARK@REMOTE>...': remote bookmark must be specified in bookmark@remote form

    For more information, try '--help'.
    "###);

    // Track/untrack unknown bookmark
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["bookmark", "track", "main@origin"]), @r###"
    Error: No such remote bookmark: main@origin
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["bookmark", "untrack", "main@origin"]), @r###"
    Error: No such remote bookmark: main@origin
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["bookmark", "track", "glob:maine@*"]), @r###"
    Error: No matching remote bookmarks for patterns: maine@*
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(
            &repo_path,
            &["bookmark", "untrack", "main@origin", "glob:main@o*"],
        ), @r###"
    Error: No matching remote bookmarks for patterns: main@origin, main@o*
    "###);

    // Track already tracked bookmark
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "track", "feature1@origin"]);
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "track", "feature1@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Remote bookmark already tracked: feature1@origin
    Nothing changed.
    "###);

    // Untrack non-tracking bookmark
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "untrack", "feature2@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Remote bookmark not tracked yet: feature2@origin
    Nothing changed.
    "###);

    // Untrack Git-tracking bookmark
    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "untrack", "main@git"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Git-tracking bookmark cannot be untracked: main@git
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: omvolwpu 1336caed commit
      @git: omvolwpu 1336caed commit
      @origin: omvolwpu 1336caed commit
    feature2@origin: omvolwpu 1336caed commit
    main: qpvuntsm 230dd059 (empty) (no description set)
      @git: qpvuntsm 230dd059 (empty) (no description set)
    "###);

    // Untrack by pattern
    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "untrack", "glob:*@*"]);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Git-tracking bookmark cannot be untracked: feature1@git
    Warning: Remote bookmark not tracked yet: feature2@origin
    Warning: Git-tracking bookmark cannot be untracked: main@git
    Stopped tracking 1 remote bookmarks.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: omvolwpu 1336caed commit
      @git: omvolwpu 1336caed commit
    feature1@origin: omvolwpu 1336caed commit
    feature2@origin: omvolwpu 1336caed commit
    main: qpvuntsm 230dd059 (empty) (no description set)
      @git: qpvuntsm 230dd059 (empty) (no description set)
    "###);

    // Track by pattern
    let (_, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "track", "glob:feature?@origin"]);
    insta::assert_snapshot!(stderr, @r###"
    Started tracking 2 remote bookmarks.
    "###);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    feature1: omvolwpu 1336caed commit
      @git: omvolwpu 1336caed commit
      @origin: omvolwpu 1336caed commit
    feature2: omvolwpu 1336caed commit
      @origin: omvolwpu 1336caed commit
    main: qpvuntsm 230dd059 (empty) (no description set)
      @git: qpvuntsm 230dd059 (empty) (no description set)
    "###);
}

#[test]
fn test_bookmark_list() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");

    // Initialize remote refs
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "remote"]);
    let remote_path = test_env.env_root().join("remote");
    for bookmark in [
        "remote-sync",
        "remote-unsync",
        "remote-untrack",
        "remote-delete",
    ] {
        test_env.jj_cmd_ok(&remote_path, &["new", "root()", "-m", bookmark]);
        test_env.jj_cmd_ok(&remote_path, &["bookmark", "create", bookmark]);
    }
    test_env.jj_cmd_ok(&remote_path, &["new"]);
    test_env.jj_cmd_ok(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = remote_path;
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", remote_git_path.to_str().unwrap(), "local"],
    );
    let local_path = test_env.env_root().join("local");
    test_env.jj_cmd_ok(&local_path, &["new", "root()", "-m", "local-only"]);
    test_env.jj_cmd_ok(&local_path, &["bookmark", "create", "local-only"]);

    // Mutate refs in local repository
    test_env.jj_cmd_ok(&local_path, &["bookmark", "delete", "remote-delete"]);
    test_env.jj_cmd_ok(&local_path, &["bookmark", "delete", "remote-untrack"]);
    test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "untrack", "remote-untrack@origin"],
    );
    test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "set", "--allow-backwards", "remote-unsync"],
    );

    // Synchronized tracking remotes and non-tracking remotes aren't listed by
    // default
    let (stdout, stderr) = test_env.jj_cmd_ok(&local_path, &["bookmark", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    local-only: wqnwkozp 4e887f78 (empty) local-only
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: wqnwkozp 4e887f78 (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&local_path, &["bookmark", "list", "--all-remotes"]);
    insta::assert_snapshot!(stdout, @r###"
    local-only: wqnwkozp 4e887f78 (empty) local-only
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
      @origin: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: wqnwkozp 4e887f78 (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    remote-untrack@origin: vmortlor 71a16b05 (empty) remote-untrack
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    let template = r#"
    concat(
      "[" ++ name ++ if(remote, "@" ++ remote) ++ "]\n",
      separate(" ", "present:", present) ++ "\n",
      separate(" ", "conflict:", conflict) ++ "\n",
      separate(" ", "normal_target:", normal_target.description().first_line()) ++ "\n",
      separate(" ", "removed_targets:", removed_targets.map(|c| c.description().first_line())) ++ "\n",
      separate(" ", "added_targets:", added_targets.map(|c| c.description().first_line())) ++ "\n",
      separate(" ", "tracked:", tracked) ++ "\n",
      separate(" ", "tracking_present:", tracking_present) ++ "\n",
      separate(" ", "tracking_ahead_count:", tracking_ahead_count.lower()) ++ "\n",
      separate(" ", "tracking_behind_count:", tracking_behind_count.lower()) ++ "\n",
    )
    "#;
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "list", "--all-remotes", "-T", template],
    );
    insta::assert_snapshot!(stdout, @r###"
    [local-only]
    present: true
    conflict: false
    normal_target: local-only
    removed_targets:
    added_targets: local-only
    tracked: false
    tracking_present: false
    tracking_ahead_count: <Error: Not a tracked remote ref>
    tracking_behind_count: <Error: Not a tracked remote ref>
    [remote-delete]
    present: false
    conflict: false
    normal_target: <Error: No Commit available>
    removed_targets:
    added_targets:
    tracked: false
    tracking_present: false
    tracking_ahead_count: <Error: Not a tracked remote ref>
    tracking_behind_count: <Error: Not a tracked remote ref>
    [remote-delete@origin]
    present: true
    conflict: false
    normal_target: remote-delete
    removed_targets:
    added_targets: remote-delete
    tracked: true
    tracking_present: false
    tracking_ahead_count: 2
    tracking_behind_count: 0
    [remote-sync]
    present: true
    conflict: false
    normal_target: remote-sync
    removed_targets:
    added_targets: remote-sync
    tracked: false
    tracking_present: false
    tracking_ahead_count: <Error: Not a tracked remote ref>
    tracking_behind_count: <Error: Not a tracked remote ref>
    [remote-sync@origin]
    present: true
    conflict: false
    normal_target: remote-sync
    removed_targets:
    added_targets: remote-sync
    tracked: true
    tracking_present: true
    tracking_ahead_count: 0
    tracking_behind_count: 0
    [remote-unsync]
    present: true
    conflict: false
    normal_target: local-only
    removed_targets:
    added_targets: local-only
    tracked: false
    tracking_present: false
    tracking_ahead_count: <Error: Not a tracked remote ref>
    tracking_behind_count: <Error: Not a tracked remote ref>
    [remote-unsync@origin]
    present: true
    conflict: false
    normal_target: remote-unsync
    removed_targets:
    added_targets: remote-unsync
    tracked: true
    tracking_present: true
    tracking_ahead_count: 1
    tracking_behind_count: 1
    [remote-untrack@origin]
    present: true
    conflict: false
    normal_target: remote-untrack
    removed_targets:
    added_targets: remote-untrack
    tracked: false
    tracking_present: false
    tracking_ahead_count: <Error: Not a tracked remote ref>
    tracking_behind_count: <Error: Not a tracked remote ref>
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);
}

#[test]
fn test_bookmark_list_filtered() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);

    // Initialize remote refs
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "remote"]);
    let remote_path = test_env.env_root().join("remote");
    for bookmark in ["remote-keep", "remote-delete", "remote-rewrite"] {
        test_env.jj_cmd_ok(&remote_path, &["new", "root()", "-m", bookmark]);
        test_env.jj_cmd_ok(&remote_path, &["bookmark", "create", bookmark]);
    }
    test_env.jj_cmd_ok(&remote_path, &["new"]);
    test_env.jj_cmd_ok(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = remote_path;
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", remote_git_path.to_str().unwrap(), "local"],
    );
    let local_path = test_env.env_root().join("local");
    test_env.jj_cmd_ok(&local_path, &["new", "root()", "-m", "local-keep"]);
    test_env.jj_cmd_ok(&local_path, &["bookmark", "create", "local-keep"]);

    // Mutate refs in local repository
    test_env.jj_cmd_ok(&local_path, &["bookmark", "delete", "remote-delete"]);
    test_env.jj_cmd_ok(&local_path, &["describe", "-mrewritten", "remote-rewrite"]);

    let template = r#"separate(" ", commit_id.short(), bookmarks, if(hidden, "(hidden)"))"#;
    insta::assert_snapshot!(
        test_env.jj_cmd_success(
            &local_path,
            &["log", "-r::(bookmarks() | remote_bookmarks())", "-T", template],
        ),
        @r#"
    @  c7b4c09cd77c local-keep
    │ ○  e31634b64294 remote-rewrite*
    ├─╯
    │ ○  3e9a5af6ef15 remote-rewrite@origin (hidden)
    ├─╯
    │ ○  dad5f298ca57 remote-delete@origin
    ├─╯
    │ ○  911e912015fb remote-keep
    ├─╯
    ◆  000000000000
    "#);

    // All bookmarks are listed by default.
    let (stdout, stderr) = test_env.jj_cmd_ok(&local_path, &["bookmark", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    let query =
        |args: &[&str]| test_env.jj_cmd_ok(&local_path, &[&["bookmark", "list"], args].concat());
    let query_error = |args: &[&str]| {
        test_env.jj_cmd_failure(&local_path, &[&["bookmark", "list"], args].concat())
    };

    // "all()" doesn't include deleted bookmarks since they have no local targets.
    // So "all()" is identical to "bookmarks()".
    let (stdout, stderr) = query(&["-rall()"]);
    insta::assert_snapshot!(stdout, @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Exclude remote-only bookmarks. "remote-rewrite@origin" is included since
    // local "remote-rewrite" target matches.
    let (stdout, stderr) = query(&["-rbookmarks()"]);
    insta::assert_snapshot!(stdout, @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Select bookmarks by name.
    let (stdout, stderr) = query(&["remote-rewrite"]);
    insta::assert_snapshot!(stdout, @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = query(&["-rbookmarks(remote-rewrite)"]);
    insta::assert_snapshot!(stdout, @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Select bookmarks by name, combined with --all-remotes
    test_env.jj_cmd_ok(&local_path, &["git", "export"]);
    let (stdout, stderr) = query(&["--all-remotes", "remote-rewrite"]);
    insta::assert_snapshot!(stdout, @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @git: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = query(&["--all-remotes", "-rbookmarks(remote-rewrite)"]);
    insta::assert_snapshot!(stdout, @r###"
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @git: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Select bookmarks with --remote
    let (stdout, stderr) = query(&["--remote", "origin"]);
    insta::assert_snapshot!(stdout, @r#"
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
      @origin: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "#);
    insta::assert_snapshot!(stderr, @r#"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "#);
    let (stdout, stderr) = query(&["--remote", "glob:gi?"]);
    insta::assert_snapshot!(stdout, @r#"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
      @git: kpqxywon c7b4c09c (empty) local-keep
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
      @git: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @git: xyxluytn e31634b6 (empty) rewritten
    "#);
    insta::assert_snapshot!(stderr, @"");
    let (stdout, stderr) = query(&["--remote", "origin", "--remote", "git"]);
    insta::assert_snapshot!(stdout, @r#"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
      @git: kpqxywon c7b4c09c (empty) local-keep
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
      @git: nlwprzpn 911e9120 (empty) remote-keep
      @origin: nlwprzpn 911e9120 (empty) remote-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @git: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "#);
    insta::assert_snapshot!(stderr, @r#"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "#);

    // Can select deleted bookmark by name pattern, but not by revset.
    let (stdout, stderr) = query(&["remote-delete"]);
    insta::assert_snapshot!(stdout, @r###"
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);
    let (stdout, stderr) = query(&["-rbookmarks(remote-delete)"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
    insta::assert_snapshot!(query_error(&["-rremote-delete"]), @r###"
    Error: Revision "remote-delete" doesn't exist
    Hint: Did you mean "remote-delete@origin", "remote-keep", "remote-rewrite", "remote-rewrite@origin"?
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Name patterns are OR-ed.
    let (stdout, stderr) = query(&["glob:*-keep", "remote-delete"]);
    insta::assert_snapshot!(stdout, @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-delete (deleted)
      @origin: yxusvupt dad5f298 (empty) remote-delete
    remote-keep: nlwprzpn 911e9120 (empty) remote-keep
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    // Unmatched name pattern shouldn't be an error. A warning can be added later.
    let (stdout, stderr) = query(&["local-keep", "glob:push-*"]);
    insta::assert_snapshot!(stdout, @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Name pattern and revset are OR-ed.
    let (stdout, stderr) = query(&["local-keep", "-rbookmarks(remote-rewrite)"]);
    insta::assert_snapshot!(stdout, @r###"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @origin (ahead by 1 commits, behind by 1 commits): xyxluytn hidden 3e9a5af6 (empty) remote-rewrite
    "###);
    insta::assert_snapshot!(stderr, @"");

    // … but still filtered by --remote
    let (stdout, stderr) = query(&[
        "local-keep",
        "-rbookmarks(remote-rewrite)",
        "--remote",
        "git",
    ]);
    insta::assert_snapshot!(stdout, @r#"
    local-keep: kpqxywon c7b4c09c (empty) local-keep
      @git: kpqxywon c7b4c09c (empty) local-keep
    remote-rewrite: xyxluytn e31634b6 (empty) rewritten
      @git: xyxluytn e31634b6 (empty) rewritten
    "#);
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_bookmark_list_much_remote_divergence() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");

    // Initialize remote refs
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "remote"]);
    let remote_path = test_env.env_root().join("remote");
    test_env.jj_cmd_ok(&remote_path, &["new", "root()", "-m", "remote-unsync"]);
    for _ in 0..15 {
        test_env.jj_cmd_ok(&remote_path, &["new", "-m", "remote-unsync"]);
    }
    test_env.jj_cmd_ok(&remote_path, &["bookmark", "create", "remote-unsync"]);
    test_env.jj_cmd_ok(&remote_path, &["new"]);
    test_env.jj_cmd_ok(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = remote_path;
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "clone", remote_git_path.to_str().unwrap(), "local"],
    );
    let local_path = test_env.env_root().join("local");
    test_env.jj_cmd_ok(&local_path, &["new", "root()", "-m", "local-only"]);
    for _ in 0..15 {
        test_env.jj_cmd_ok(&local_path, &["new", "-m", "local-only"]);
    }
    test_env.jj_cmd_ok(&local_path, &["bookmark", "create", "local-only"]);

    // Mutate refs in local repository
    test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "set", "--allow-backwards", "remote-unsync"],
    );

    let (stdout, stderr) = test_env.jj_cmd_ok(&local_path, &["bookmark", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    local-only: zkyosouw 4ab3f751 (empty) local-only
    remote-unsync: zkyosouw 4ab3f751 (empty) local-only
      @origin (ahead by at least 10 commits, behind by at least 10 commits): lxyktnks 19582022 (empty) remote-unsync
    "###);
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_bookmark_list_tracked() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-bookmark = true");

    // Initialize remote refs
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "remote"]);
    let remote_path = test_env.env_root().join("remote");
    for bookmark in [
        "remote-sync",
        "remote-unsync",
        "remote-untrack",
        "remote-delete",
    ] {
        test_env.jj_cmd_ok(&remote_path, &["new", "root()", "-m", bookmark]);
        test_env.jj_cmd_ok(&remote_path, &["bookmark", "create", bookmark]);
    }
    test_env.jj_cmd_ok(&remote_path, &["new"]);
    test_env.jj_cmd_ok(&remote_path, &["git", "export"]);

    // Initialize local refs
    let mut remote_git_path = test_env.env_root().join("remote");
    remote_git_path.extend([".jj", "repo", "store", "git"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &[
            "git",
            "clone",
            "--colocate",
            remote_git_path.to_str().unwrap(),
            "local",
        ],
    );

    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "upstream"]);

    // Initialize a second remote
    let mut upstream_git_path = test_env.env_root().join("upstream");
    test_env.jj_cmd_ok(
        &upstream_git_path,
        &["new", "root()", "-m", "upstream-sync"],
    );
    test_env.jj_cmd_ok(&upstream_git_path, &["bookmark", "create", "upstream-sync"]);
    test_env.jj_cmd_ok(&upstream_git_path, &["new"]);
    test_env.jj_cmd_ok(&upstream_git_path, &["git", "export"]);

    upstream_git_path.extend([".jj", "repo", "store", "git"]);

    let local_path = test_env.env_root().join("local");

    test_env.jj_cmd_ok(
        &local_path,
        &[
            "git",
            "remote",
            "add",
            "upstream",
            upstream_git_path.to_str().unwrap(),
        ],
    );
    test_env.jj_cmd_ok(&local_path, &["git", "fetch", "--all-remotes"]);

    test_env.jj_cmd_ok(&local_path, &["new", "root()", "-m", "local-only"]);
    test_env.jj_cmd_ok(&local_path, &["bookmark", "create", "local-only"]);

    // Mutate refs in local repository
    test_env.jj_cmd_ok(&local_path, &["bookmark", "delete", "remote-delete"]);
    test_env.jj_cmd_ok(&local_path, &["bookmark", "delete", "remote-untrack"]);
    test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "untrack", "remote-untrack@origin"],
    );
    test_env.jj_cmd_ok(
        &local_path,
        &[
            "git",
            "push",
            "--allow-new",
            "--remote",
            "upstream",
            "--bookmark",
            "remote-unsync",
        ],
    );
    test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "set", "--allow-backwards", "remote-unsync"],
    );

    let (stdout, stderr) = test_env.jj_cmd_ok(&local_path, &["bookmark", "list", "--all-remotes"]);
    insta::assert_snapshot!(stdout, @r###"
    local-only: nmzmmopx e1da745b (empty) local-only
      @git: nmzmmopx e1da745b (empty) local-only
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
      @git: zwtyzrop c761c7ea (empty) remote-sync
      @origin: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: nmzmmopx e1da745b (empty) local-only
      @git: nmzmmopx e1da745b (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
      @upstream (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    remote-untrack@origin: vmortlor 71a16b05 (empty) remote-untrack
    upstream-sync: lolpmnqw 32fa6da0 (empty) upstream-sync
      @git: lolpmnqw 32fa6da0 (empty) upstream-sync
      @upstream: lolpmnqw 32fa6da0 (empty) upstream-sync
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&local_path, &["bookmark", "list", "--tracked"]);
    insta::assert_snapshot!(stdout, @r###"
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
      @origin: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: nmzmmopx e1da745b (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
      @upstream (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    upstream-sync: lolpmnqw 32fa6da0 (empty) upstream-sync
      @upstream: lolpmnqw 32fa6da0 (empty) upstream-sync
    "###
    );
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "list", "--tracked", "--remote", "origin"],
    );
    insta::assert_snapshot!(stdout, @r#"
    remote-delete (deleted)
      @origin: mnmymoky 203e60eb (empty) remote-delete
    remote-sync: zwtyzrop c761c7ea (empty) remote-sync
      @origin: zwtyzrop c761c7ea (empty) remote-sync
    remote-unsync: nmzmmopx e1da745b (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    "#
    );
    insta::assert_snapshot!(stderr, @r###"
    Hint: Bookmarks marked as deleted will be *deleted permanently* on the remote on the next `jj git push`. Use `jj bookmark forget` to prevent this.
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "list", "--tracked", "remote-unsync"],
    );
    insta::assert_snapshot!(stdout, @r###"
    remote-unsync: nmzmmopx e1da745b (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
      @upstream (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    "###);
    insta::assert_snapshot!(stderr, @"");

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "list", "--tracked", "remote-untrack"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");

    test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "untrack", "remote-unsync@upstream"],
    );

    let (stdout, stderr) = test_env.jj_cmd_ok(
        &local_path,
        &["bookmark", "list", "--tracked", "remote-unsync"],
    );
    insta::assert_snapshot!(stdout, @r###"
    remote-unsync: nmzmmopx e1da745b (empty) local-only
      @origin (ahead by 1 commits, behind by 1 commits): qpsqxpyq 38ef8af7 (empty) remote-unsync
    "###);
    insta::assert_snapshot!(stderr, @"");
}

#[test]
fn test_bookmark_list_conflicted() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Track existing bookmark. Local bookmark should result in conflict.
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["bookmark", "create", "foo", "-r", "description(a)"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "bookmark",
            "create",
            "foo",
            "-r",
            "description(b)",
            "--at-op=@-",
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["status"]);
    insta::assert_snapshot!(get_bookmark_output(&test_env, &repo_path), @r###"
    bar: kkmpptxz 06a973bc (empty) b
    foo (conflicted):
      + rlvkpnrz d8d5f980 (empty) a
      + kkmpptxz 06a973bc (empty) b
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["bookmark", "list", "--conflicted"]), @r###"
    foo (conflicted):
      + rlvkpnrz d8d5f980 (empty) a
      + kkmpptxz 06a973bc (empty) b
    "###);
}

#[test]
fn test_bookmark_create_onto_hidden_unhides() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("a.txt"), "AA").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "A"]);
    // Emulate a simple commit change, where we want to recover the initial version.
    std::fs::write(repo_path.join("b.txt"), "BB").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["debug", "snapshot"]);
    std::fs::write(repo_path.join("b.txt"), "Art").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "B"]);
    // Create our bookmark onto the hidden commit.
    let (stdout, _) = test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "back"]);
    insta::assert_snapshot!(stdout, r#""#);
}

#[test]
fn test_bookmark_move_onto_hidden_unhides() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("a.txt"), "AA").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "A"]);
    // Create our bookmark on the first commit. It will be moved to a predecessor of
    // the second one.
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "back"]);
    // Emulate a simple commit change, where we want to recover the initial version.
    std::fs::write(repo_path.join("b.txt"), "BB").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["debug", "snapshot"]);
    std::fs::write(repo_path.join("b.txt"), "Art").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "B"]);

    insta::assert_snapshot!(get_evolog_output(&test_env, &repo_path), r#""#);

    let (stdout, _) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "move", "back", "-r", "<old-id>"]);
    insta::assert_snapshot!(stdout, r#""#);
}

#[test]
fn test_bookmark_set_onto_hidden_unhides() {
    // TODO: write
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("a.txt"), "AA").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "A"]);
    // Emulate a simple commit change, where we want to recover the initial version.
    std::fs::write(repo_path.join("b.txt"), "BB").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["debug", "snapshot"]);
    std::fs::write(repo_path.join("b.txt"), "Art").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "B"]);
    insta::assert_snapshot!(get_evolog_output(&test_env, &repo_path), r#""#);
    let (stdout, _) =
        test_env.jj_cmd_ok(&repo_path, &["bookmark", "set", "back", "-r", "<old-id>"]);
    insta::assert_snapshot!(stdout, r#""#);
}

fn get_evolog_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"change_id ++ " " ++ commit_id"#;
    test_env.jj_cmd_success(cwd, &["evolog", "-T", template])
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"bookmarks ++ " " ++ commit_id.short()"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}

fn get_bookmark_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    // --quiet to suppress deleted bookmarks hint
    test_env.jj_cmd_success(repo_path, &["bookmark", "list", "--all-remotes", "--quiet"])
}
