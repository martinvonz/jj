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
    test_env.jj_cmd_ok(&repo_path, &["git", "push", "--bookmark", "glob:*-tracked"]);

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
