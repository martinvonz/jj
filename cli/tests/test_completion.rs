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
fn test_bookmark_rename() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "aaa"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bbb"]);

    let mut test_env = test_env;
    // Every shell hook is a little different, e.g. the zsh hooks add some
    // additional environment variables. But this is irrelevant for the purpose
    // of testing our own logic, so it's fine to test a single shell only.
    test_env.add_env_var("COMPLETE", "fish");
    let test_env = test_env;

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "bookmark", "rename", ""]);
    insta::assert_snapshot!(stdout, @r"
    aaa
    bbb
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
    insta::assert_snapshot!(stdout, @"aaa");
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
    insta::assert_snapshot!(stdout, @"aaa");
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
    insta::assert_snapshot!(stdout, @"aaa");

    let stdout = test_env.jj_cmd_success(&repo_path, &["--", "jj", "b2", "rename", "a"]);
    insta::assert_snapshot!(stdout, @"aaa");
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
