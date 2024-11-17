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

use itertools::Itertools as _;

use crate::common::get_stdout_string;
use crate::common::TestEnvironment;

#[test]
fn test_bookmark_names() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "aaa-local"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bbb-local"]);

    // add various remote branches
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "git",
            "remote",
            "add",
            "origin",
            origin_git_repo_path.to_str().unwrap(),
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "aaa-tracked"]);
    test_env.jj_cmd_ok(&repo_path, &["desc", "-r", "aaa-tracked", "-m", "x"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bbb-tracked"]);
    test_env.jj_cmd_ok(&repo_path, &["desc", "-r", "bbb-tracked", "-m", "x"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &["git", "push", "--allow-new", "--bookmark", "glob:*-tracked"],
    );

    test_env.jj_cmd_ok(&origin_path, &["bookmark", "create", "aaa-untracked"]);
    test_env.jj_cmd_ok(&origin_path, &["desc", "-r", "aaa-untracked", "-m", "x"]);
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "create", "bbb-untracked"]);
    test_env.jj_cmd_ok(&origin_path, &["desc", "-r", "bbb-untracked", "-m", "x"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);

    let mut test_env = test_env;
    // Every shell hook is a little different, e.g. the zsh hooks add some
    // additional environment variables. But this is irrelevant for the purpose
    // of testing our own logic, so it's fine to test a single shell only.
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "rename", ""]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    bbb-local	x
    bbb-tracked	x
    --repository	Path to repository to operate on
    --ignore-working-copy	Don't snapshot the working copy, and don't update it
    --ignore-immutable	Allow rewriting immutable commits
    --at-operation	Operation to load the repo at
    --debug	Enable debug logging
    --color	When to colorize output (always, never, debug, auto)
    --quiet	Silence non-primary command output
    --no-pager	Disable the pager
    --config-toml	Additional configuration options (can be repeated)
    --help	Print help (see more with '--help')
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "rename", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "delete", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "forget", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    aaa-untracked
    ");

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["--", "jj", "bookmark", "list", "--bookmark", "a"],
    );
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    aaa-untracked
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "move", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "set", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "track", "a"]);
    insta::assert_snapshot!(stdout, @"aaa-untracked@origin	x");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "untrack", "a"]);
    insta::assert_snapshot!(stdout, @"aaa-tracked@origin	x");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "git", "push", "-b", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    ");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "git", "fetch", "-b", "a"]);
    insta::assert_snapshot!(stdout, @r"
    aaa-local	x
    aaa-tracked	x
    aaa-untracked
    ");
}

#[test]
fn test_global_arg_repository_is_respected() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "aaa"]);

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "--",
            "jj",
            "--repository",
            "repo",
            "bookmark",
            "rename",
            "a",
        ],
    );
    insta::assert_snapshot!(stdout, @"aaa	(no description set)");
}

#[test]
fn test_aliases_are_resolved() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "aaa"]);

    // user config alias
    test_env.add_config(r#"aliases.b = ["bookmark"]"#);
    // repo config alias
    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--repo", "aliases.b2", "['bookmark']"],
    );

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "b", "rename", "a"]);
    insta::assert_snapshot!(stdout, @"aaa	(no description set)");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "b2", "rename", "a"]);
    insta::assert_snapshot!(stdout, @"aaa	(no description set)");
}

#[test]
fn test_completions_are_generated() {
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("COMPLETE", "fish");
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &[]);
    // cannot use assert_snapshot!, output contains path to binary that depends
    // on environment
    assert!(stdout.starts_with("complete --keep-order --exclusive --command jj --arguments"));
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["--"]);
    assert!(stdout.starts_with("complete --keep-order --exclusive --command jj --arguments"));
}

#[test]
fn test_zsh_completion() {
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("COMPLETE", "zsh");

    // ["--", "jj"]
    //        ^^^^ index = 0
    let complete_at = |index: usize, args: &[&str]| {
        let assert = test_env
            .jj_cmd(test_env.env_root(), args)
            .env("_CLAP_COMPLETE_INDEX", index.to_string())
            .assert()
            .success();
        get_stdout_string(&assert)
    };

    // Command names should be suggested. If the default command were expanded,
    // only "log" would be listed.
    let stdout = complete_at(1, &["--", "jj"]);
    insta::assert_snapshot!(stdout.lines().take(2).join("\n"), @r"
    abandon:Abandon a revision
    absorb:Move changes from a revision into the stack of mutable revisions
    ");
    let stdout = complete_at(2, &["--", "jj", "--no-pager"]);
    insta::assert_snapshot!(stdout.lines().take(2).join("\n"), @r"
    abandon:Abandon a revision
    absorb:Move changes from a revision into the stack of mutable revisions
    ");

    let stdout = complete_at(1, &["--", "jj", "b"]);
    insta::assert_snapshot!(stdout, @"bookmark:Manage bookmarks [default alias: b]");
}

#[test]
fn test_remote_names() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init"]);

    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["git", "remote", "add", "origin", "git@git.local:user/repo"],
    );

    test_env.add_env_var("COMPLETE", "fish");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["--", "jj", "git", "remote", "remove", "o"],
    );
    insta::assert_snapshot!(stdout, @r"origin");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["--", "jj", "git", "remote", "rename", "o"],
    );
    insta::assert_snapshot!(stdout, @r"origin");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["--", "jj", "git", "remote", "set-url", "o"],
    );
    insta::assert_snapshot!(stdout, @r"origin");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["--", "jj", "git", "push", "--remote", "o"],
    );
    insta::assert_snapshot!(stdout, @r"origin");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["--", "jj", "git", "fetch", "--remote", "o"],
    );
    insta::assert_snapshot!(stdout, @r"origin");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["--", "jj", "bookmark", "list", "--remote", "o"],
    );
    insta::assert_snapshot!(stdout, @r"origin");
}

#[test]
fn test_aliases_are_completed() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // user config alias
    test_env.add_config(r#"aliases.user-alias = ["bookmark"]"#);
    // repo config alias
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "config",
            "set",
            "--repo",
            "aliases.repo-alias",
            "['bookmark']",
        ],
    );

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "user-al"]);
    insta::assert_snapshot!(stdout, @"user-alias");

    // make sure --repository flag is respected
    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "--",
            "jj",
            "--repository",
            repo_path.to_str().unwrap(),
            "repo-al",
        ],
    );
    insta::assert_snapshot!(stdout, @"repo-alias");

    // cannot load aliases from --config-toml flag
    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "--",
            "jj",
            "--config-toml",
            "aliases.cli-alias = ['bookmark']",
            "cli-al",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
}

#[test]
fn test_revisions() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // create remote to test remote branches
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "git",
            "remote",
            "add",
            "origin",
            origin_git_repo_path.to_str().unwrap(),
        ],
    );
    test_env.jj_cmd_ok(&origin_path, &["b", "c", "remote_bookmark"]);
    test_env.jj_cmd_ok(&origin_path, &["commit", "-m", "remote_commit"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "fetch"]);

    test_env.jj_cmd_ok(&repo_path, &["b", "c", "immutable_bookmark"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "immutable"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "immutable_bookmark""#);

    test_env.jj_cmd_ok(&repo_path, &["b", "c", "mutable_bookmark"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "mutable"]);

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "working_copy"]);

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    // There are _a lot_ of commands and arguments accepting revisions.
    // Let's not test all of them. Having at least one test per variation of
    // completion function should be sufficient.

    // complete all revisions
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "diff", "--from", ""]);
    insta::assert_snapshot!(stdout, @r"
    immutable_bookmark	immutable
    mutable_bookmark	mutable
    k	working_copy
    y	mutable
    q	immutable
    zq	remote_commit
    zz	(no description set)
    remote_bookmark@origin	remote_commit
    ");

    // complete only mutable revisions
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "squash", "--into", ""]);
    insta::assert_snapshot!(stdout, @r"
    mutable_bookmark	mutable
    k	working_copy
    y	mutable
    zq	remote_commit
    ");

    // complete args of the default command
    test_env.add_config("ui.default-command = 'log'");
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "-r", ""]);
    insta::assert_snapshot!(stdout, @r"
    immutable_bookmark	immutable
    mutable_bookmark	mutable
    k	working_copy
    y	mutable
    q	immutable
    zq	remote_commit
    zz	(no description set)
    remote_bookmark@origin	remote_commit
    ");
}

#[test]
fn test_operations() {
    let test_env = TestEnvironment::default();

    // suppress warnings on stderr of completions for invalid args
    test_env.add_config("ui.default-command = 'log'");

    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 0"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 1"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 2"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 3"]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 4"]);

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "show", ""]);
    let add_workspace_id = stdout.lines().nth(5).unwrap().split('\t').next().unwrap();
    insta::assert_snapshot!(add_workspace_id, @"eac759b9ab75");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "show", "5"]);
    insta::assert_snapshot!(stdout, @r"
    5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710
    518b588abbc6	(2001-02-03 08:05:09) describe commit 19611c995a342c01f525583e5fcafdd211f6d009
    ");
    // make sure global --at-op flag is respected
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["--", "jj", "--at-op", "518b588abbc6", "op", "show", "5"],
    );
    insta::assert_snapshot!(stdout, @"518b588abbc6	(2001-02-03 08:05:09) describe commit 19611c995a342c01f525583e5fcafdd211f6d009");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "--at-op", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "abandon", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "diff", "--op", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "diff", "--from", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "diff", "--to", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "restore", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "op", "undo", "5b"]);
    insta::assert_snapshot!(stdout, @"5bbb4ca536a8	(2001-02-03 08:05:12) describe commit 968261075dddabf4b0e333c1cc9a49ce26a3f710");
}

#[test]
fn test_workspaces() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["describe", "-m", "initial"]);

    test_env.jj_cmd_ok(
        &main_path,
        // same prefix as "default" workspace
        &["workspace", "add", "--name", "def-second", "../secondary"],
    );

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&main_path, &["--", "jj", "workspace", "forget", "def"]);
    insta::assert_snapshot!(stdout, @r"
    def-second	(no description set)
    default	initial
    ");
}

#[test]
fn test_config() {
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("COMPLETE", "fish");
    let dir = test_env.env_root();

    let stdout = test_env.jj_cmd_success(dir, &["--", "jj", "config", "get", "c"]);
    insta::assert_snapshot!(stdout, @r"
    core.fsmonitor	Whether to use an external filesystem monitor, useful for large repos
    core.watchman.register_snapshot_trigger	Whether to use triggers to monitor for changes in the background.
    ");

    let stdout = test_env.jj_cmd_success(dir, &["--", "jj", "config", "list", "c"]);
    insta::assert_snapshot!(stdout, @r"
    colors	Mapping from jj formatter labels to colors
    core
    core.fsmonitor	Whether to use an external filesystem monitor, useful for large repos
    core.watchman
    core.watchman.register_snapshot_trigger	Whether to use triggers to monitor for changes in the background.
    ");
}

fn create_commit(
    test_env: &TestEnvironment,
    repo_path: &std::path::Path,
    name: &str,
    parents: &[&str],
    files: &[(&str, Option<&str>)],
) {
    if parents.is_empty() {
        test_env.jj_cmd_ok(repo_path, &["new", "root()", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_ok(repo_path, &args);
    }
    for (name, content) in files {
        if let Some((dir, _)) = name.rsplit_once('/') {
            std::fs::create_dir_all(repo_path.join(dir)).unwrap();
        }
        match content {
            Some(content) => std::fs::write(repo_path.join(name), content).unwrap(),
            None => std::fs::remove_file(repo_path.join(name)).unwrap(),
        }
    }
    test_env.jj_cmd_ok(repo_path, &["bookmark", "create", name]);
}

#[test]
fn test_files() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Completions for files use filesets internally.
    // Ensure they still work if the user has them disabled.
    test_env.add_config("ui.allow-filesets = false");

    create_commit(
        &test_env,
        &repo_path,
        "first",
        &[],
        &[
            ("f_unchanged", Some("unchanged\n")),
            ("f_modified", Some("not_yet_modified\n")),
            ("f_not_yet_renamed", Some("renamed\n")),
            ("f_deleted", Some("not_yet_deleted\n")),
            // not yet: "added" file
        ],
    );
    create_commit(
        &test_env,
        &repo_path,
        "second",
        &["first"],
        &[
            // "unchanged" file
            ("f_modified", Some("modified\n")),
            ("f_renamed", Some("renamed\n")),
            ("f_deleted", None),
            ("f_added", Some("added\n")),
            ("f_dir/dir_file_1", Some("foo\n")),
            ("f_dir/dir_file_2", Some("foo\n")),
            ("f_dir/dir_file_3", Some("foo\n")),
        ],
    );

    // create a conflicted commit to check the completions of `jj restore`
    create_commit(
        &test_env,
        &repo_path,
        "conflicted",
        &["second"],
        &[
            ("f_modified", Some("modified_again\n")),
            ("f_added_2", Some("added_2\n")),
            ("f_dir/dir_file_1", Some("bar\n")),
            ("f_dir/dir_file_2", Some("bar\n")),
            ("f_dir/dir_file_3", Some("bar\n")),
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["rebase", "-r=@", "-d=first"]);

    // two commits that are similar but not identical, for `jj interdiff`
    create_commit(
        &test_env,
        &repo_path,
        "interdiff_from",
        &[],
        &[
            ("f_interdiff_same", Some("same in both commits\n")),
            (("f_interdiff_only_from"), Some("only from\n")),
        ],
    );
    create_commit(
        &test_env,
        &repo_path,
        "interdiff_to",
        &[],
        &[
            ("f_interdiff_same", Some("same in both commits\n")),
            (("f_interdiff_only_to"), Some("only to\n")),
        ],
    );

    // "dirty worktree"
    create_commit(
        &test_env,
        &repo_path,
        "working_copy",
        &["second"],
        &[
            ("f_modified", Some("modified_again\n")),
            ("f_added_2", Some("added_2\n")),
        ],
    );

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "--summary"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    @  wqnwkozp test.user@example.com 2001-02-03 08:05:20 working_copy 45c3a621
    │  working_copy
    │  A f_added_2
    │  M f_modified
    ○  zsuskuln test.user@example.com 2001-02-03 08:05:11 second 77a99380
    │  second
    │  A f_added
    │  D f_deleted
    │  A f_dir/dir_file_1
    │  A f_dir/dir_file_2
    │  A f_dir/dir_file_3
    │  M f_modified
    │  A f_renamed
    │ ×  royxmykx test.user@example.com 2001-02-03 08:05:14 conflicted 23eb154d conflict
    ├─╯  conflicted
    │    A f_added_2
    │    A f_dir/dir_file_1
    │    A f_dir/dir_file_2
    │    A f_dir/dir_file_3
    │    M f_modified
    ○  rlvkpnrz test.user@example.com 2001-02-03 08:05:09 first 2a2f433c
    │  first
    │  A f_deleted
    │  A f_modified
    │  A f_not_yet_renamed
    │  A f_unchanged
    │ ○  kpqxywon test.user@example.com 2001-02-03 08:05:18 interdiff_to 302c4041
    ├─╯  interdiff_to
    │    A f_interdiff_only_to
    │    A f_interdiff_same
    │ ○  yostqsxw test.user@example.com 2001-02-03 08:05:16 interdiff_from 083d1cc6
    ├─╯  interdiff_from
    │    A f_interdiff_only_from
    │    A f_interdiff_same
    ◆  zzzzzzzz root() 00000000
    ");

    let mut test_env = test_env;
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "file", "show", "f_"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_added
    f_added_2
    f_dir/
    f_modified
    f_not_yet_renamed
    f_renamed
    f_unchanged
    ");

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["--", "jj", "file", "annotate", "-r@-", "f_"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_added
    f_dir/
    f_modified
    f_not_yet_renamed
    f_renamed
    f_unchanged
    ");
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "diff", "-r", "@-", "f_"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_added	Added
    f_deleted	Deleted
    f_dir/
    f_modified	Modified
    f_renamed	Added
    ");

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "--",
            "jj",
            "diff",
            "-r",
            "@-",
            &format!("f_dir{}", std::path::MAIN_SEPARATOR),
        ],
    );
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_dir/dir_file_1	Added
    f_dir/dir_file_2	Added
    f_dir/dir_file_3	Added
    ");

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["--", "jj", "diff", "--from", "root()", "--to", "@-", "f_"],
    );
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_added	Added
    f_dir/
    f_modified	Added
    f_not_yet_renamed	Added
    f_renamed	Added
    f_unchanged	Added
    ");

    // interdiff has a different behavior with --from and --to flags
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "--",
            "jj",
            "interdiff",
            "--to=interdiff_to",
            "--from=interdiff_from",
            "f_",
        ],
    );
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_interdiff_only_from	Added
    f_interdiff_same	Added
    f_interdiff_only_to	Added
    f_interdiff_same	Added
    ");

    // squash has a different behavior with --from and --to flags
    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "squash", "-f=first", "f_"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_deleted	Added
    f_modified	Added
    f_not_yet_renamed	Added
    f_unchanged	Added
    ");

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["--", "jj", "resolve", "-r=conflicted", "f_"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r"
    f_dir/
    f_modified
    ");
}
