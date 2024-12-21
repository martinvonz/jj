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

use std::path::PathBuf;

use indoc::indoc;
use itertools::Itertools;
use regex::Regex;

use crate::common::fake_editor_path;
use crate::common::to_toml_value;
use crate::common::TestEnvironment;

#[test]
fn test_config_list_single() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    [test-table]
    somekey = "some value"
    "#,
    );

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["config", "list", "test-table.somekey"],
    );
    insta::assert_snapshot!(stdout, @r###"
    test-table.somekey = "some value"
    "###);

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["config", "list", r#"-Tname ++ "\n""#, "test-table.somekey"],
    );
    insta::assert_snapshot!(stdout, @r###"
    test-table.somekey
    "###);
}

#[test]
fn test_config_list_nonexistent() {
    let test_env = TestEnvironment::default();
    let (stdout, stderr) = test_env.jj_cmd_ok(
        test_env.env_root(),
        &["config", "list", "nonexistent-test-key"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No matching config key for nonexistent-test-key
    "###);
}

#[test]
fn test_config_list_table() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    [test-table]
    x = true
    y.foo = "abc"
    y.bar = 123
    "z"."with space"."function()" = 5
    "#,
    );
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "test-table"]);
    insta::assert_snapshot!(
        stdout,
        @r###"
    test-table.x = true
    test-table.y.foo = "abc"
    test-table.y.bar = 123
    test-table.z."with space"."function()" = 5
    "###);
}

#[test]
fn test_config_list_inline_table() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    test-table = { x = true, y = 1 }
    "#,
    );
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "test-table"]);
    insta::assert_snapshot!(stdout, @"test-table = { x = true, y = 1 }");
    // Inner value cannot be addressed by a dotted name path
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["config", "list", "test-table.x"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"Warning: No matching config key for test-table.x");
}

#[test]
fn test_config_list_array() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    test-array = [1, "b", 3.4]
    "#,
    );
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "test-array"]);
    insta::assert_snapshot!(stdout, @r###"
    test-array = [1, "b", 3.4]
    "###);
}

#[test]
fn test_config_list_array_of_tables() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
        [[test-table]]
        x = 1
        [[test-table]]
        y = ["z"]
        z."key=with whitespace" = []
    "#,
    );
    // TODO: Perhaps, each value should be listed separately, but there's no
    // path notation like "test-table[0].x".
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "test-table"]);
    insta::assert_snapshot!(stdout, @r###"
    test-table = [{ x = 1 }, { y = ["z"], z = { "key=with whitespace" = [] } }]
    "###);
}

#[test]
fn test_config_list_all() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    test-val = [1, 2, 3]
    [test-table]
    x = true
    y.foo = "abc"
    y.bar = 123
    "#,
    );

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list"]);
    insta::assert_snapshot!(
        find_stdout_lines(r"(test-val|test-table\b[^=]*)", &stdout),
        @r###"
    test-val = [1, 2, 3]
    test-table.x = true
    test-table.y.foo = "abc"
    test-table.y.bar = 123
    "###);
}

#[test]
fn test_config_list_multiline_string() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    multiline = '''
foo
bar
'''
    "#,
    );

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "multiline"]);
    insta::assert_snapshot!(stdout, @r"
    multiline = '''
    foo
    bar
    '''
    ");

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "config",
            "list",
            "multiline",
            "--include-overridden",
            "--config=multiline='single'",
        ],
    );
    insta::assert_snapshot!(stdout, @r"
    # multiline = '''
    # foo
    # bar
    # '''
    multiline = 'single'
    ");
}

#[test]
fn test_config_list_layer() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    // Test with fresh new config file
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(&user_config_path);
    let repo_path = test_env.env_root().join("repo");

    // User
    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-key", "test-val"],
    );

    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "config",
            "set",
            "--user",
            "test-layered-key",
            "test-original-val",
        ],
    );

    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", "--user"]);
    insta::assert_snapshot!(stdout, @r###"
    test-key = "test-val"
    test-layered-key = "test-original-val"
    "###);

    // Repo
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "config",
            "set",
            "--repo",
            "test-layered-key",
            "test-layered-val",
        ],
    );

    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", "--user"]);
    insta::assert_snapshot!(stdout, @r###"
    test-key = "test-val"
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", "--repo"]);
    insta::assert_snapshot!(stdout, @r###"
    test-layered-key = "test-layered-val"
    "###);
}

#[test]
fn test_config_layer_override_default() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let config_key = "merge-tools.vimdiff.program";

    // Default
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["config", "list", config_key, "--include-defaults"],
    );
    insta::assert_snapshot!(stdout, @r###"
    merge-tools.vimdiff.program = "vim"
    "###);

    // User
    test_env.add_config(format!(
        "{config_key} = {value}\n",
        value = to_toml_value("user")
    ));
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    merge-tools.vimdiff.program = "user"
    "###);

    // Repo
    std::fs::write(
        repo_path.join(".jj/repo/config.toml"),
        format!("{config_key} = {value}\n", value = to_toml_value("repo")),
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    merge-tools.vimdiff.program = "repo"
    "###);

    // Command argument
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "config",
            "list",
            config_key,
            "--config",
            &format!("{config_key}={value}", value = to_toml_value("command-arg")),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    merge-tools.vimdiff.program = "command-arg"
    "###);

    // Allow printing overridden values
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "config",
            "list",
            config_key,
            "--include-overridden",
            "--config",
            &format!("{config_key}={value}", value = to_toml_value("command-arg")),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    # merge-tools.vimdiff.program = "user"
    # merge-tools.vimdiff.program = "repo"
    merge-tools.vimdiff.program = "command-arg"
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "config",
            "list",
            "--color=always",
            config_key,
            "--include-overridden",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    [38;5;8m# merge-tools.vimdiff.program = "user"[39m
    [38;5;2mmerge-tools.vimdiff.program[39m = [38;5;3m"repo"[39m
    "###);
}

#[test]
fn test_config_layer_override_env() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let config_key = "ui.editor";

    // Environment base
    test_env.add_env_var("EDITOR", "env-base");
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "env-base"
    "###);

    // User
    test_env.add_config(format!(
        "{config_key} = {value}\n",
        value = to_toml_value("user")
    ));
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "user"
    "###);

    // Repo
    std::fs::write(
        repo_path.join(".jj/repo/config.toml"),
        format!("{config_key} = {value}\n", value = to_toml_value("repo")),
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "repo"
    "###);

    // Environment override
    test_env.add_env_var("JJ_EDITOR", "env-override");
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "env-override"
    "###);

    // Command argument
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "config",
            "list",
            config_key,
            "--config",
            &format!("{config_key}={value}", value = to_toml_value("command-arg")),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "command-arg"
    "###);

    // Allow printing overridden values
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "config",
            "list",
            config_key,
            "--include-overridden",
            "--config",
            &format!("{config_key}={value}", value = to_toml_value("command-arg")),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    # ui.editor = "env-base"
    # ui.editor = "user"
    # ui.editor = "repo"
    # ui.editor = "env-override"
    ui.editor = "command-arg"
    "###);
}

#[test]
fn test_config_layer_workspace() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "main"]);
    let main_path = test_env.env_root().join("main");
    let secondary_path = test_env.env_root().join("secondary");
    let config_key = "ui.editor";

    std::fs::write(main_path.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&main_path, &["new"]);
    test_env.jj_cmd_ok(
        &main_path,
        &["workspace", "add", "--name", "second", "../secondary"],
    );

    // Repo
    std::fs::write(
        main_path.join(".jj/repo/config.toml"),
        format!(
            "{config_key} = {value}\n",
            value = to_toml_value("main-repo")
        ),
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&main_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "main-repo"
    "###);
    let stdout = test_env.jj_cmd_success(&secondary_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "main-repo"
    "###);
}

#[test]
fn test_config_set_bad_opts() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["config", "set"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      <--user|--repo>
      <NAME>
      <VALUE>

    Usage: jj config set <--user|--repo> <NAME> <VALUE>

    For more information, try '--help'.
    "###);

    let stderr =
        test_env.jj_cmd_cli_error(test_env.env_root(), &["config", "set", "--user", "", "x"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value '' for '<NAME>': TOML parse error at line 1, column 1
      |
    1 | 
      | ^
    invalid key


    For more information, try '--help'.
    "###);

    let stderr = test_env.jj_cmd_cli_error(
        test_env.env_root(),
        &["config", "set", "--user", "x", "['typo'}"],
    );
    insta::assert_snapshot!(stderr, @r"
    error: invalid value '['typo'}' for '<VALUE>': TOML parse error at line 1, column 8
      |
    1 | ['typo'}
      |        ^
    invalid array
    expected `]`


    For more information, try '--help'.
    ");
}

#[test]
fn test_config_set_for_user() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    // Test with fresh new config file
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(&user_config_path);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-key", "test-val"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-table.foo", "true"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-table.'bar()'", "0"],
    );

    // Ensure test-key successfully written to user config.
    let user_config_toml = std::fs::read_to_string(&user_config_path)
        .unwrap_or_else(|_| panic!("Failed to read file {}", user_config_path.display()));
    insta::assert_snapshot!(user_config_toml, @r###"
    test-key = "test-val"

    [test-table]
    foo = true
    "bar()" = 0
    "###);
}

#[test]
fn test_config_set_for_user_directory() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["config", "set", "--user", "test-key", "test-val"],
    );
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.last_config_file_path()).unwrap(),
        @r#"
    test-key = "test-val"

    [template-aliases]
    'format_time_range(time_range)' = 'time_range.start() ++ " - " ++ time_range.end()'
    "#);

    // Add one more config file to the directory
    test_env.add_config("");
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["config", "set", "--user", "test-key", "test-val"],
    );
    insta::assert_snapshot!(stderr, @r"
    Error: Cannot determine config file to edit:
      $TEST_ENV/config/config0001.toml
      $TEST_ENV/config/config0002.toml
    ");
}

#[test]
fn test_config_set_for_repo() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--repo", "test-key", "test-val"],
    );
    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--repo", "test-table.foo", "true"],
    );
    // Ensure test-key successfully written to user config.
    let expected_repo_config_path = repo_path.join(".jj/repo/config.toml");
    let repo_config_toml =
        std::fs::read_to_string(&expected_repo_config_path).unwrap_or_else(|_| {
            panic!(
                "Failed to read file {}",
                expected_repo_config_path.display()
            )
        });
    insta::assert_snapshot!(repo_config_toml, @r###"
    test-key = "test-val"

    [test-table]
    foo = true
    "###);
}

#[test]
fn test_config_set_toml_types() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    // Test with fresh new config file
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(&user_config_path);
    let repo_path = test_env.env_root().join("repo");

    let set_value = |key, value| {
        test_env.jj_cmd_success(&repo_path, &["config", "set", "--user", key, value]);
    };
    set_value("test-table.integer", "42");
    set_value("test-table.float", "3.14");
    set_value("test-table.array", r#"["one", "two"]"#);
    set_value("test-table.boolean", "true");
    set_value("test-table.string", r#""foo""#);
    set_value("test-table.invalid", r"a + b");
    insta::assert_snapshot!(std::fs::read_to_string(&user_config_path).unwrap(), @r###"
    [test-table]
    integer = 42
    float = 3.14
    array = ["one", "two"]
    boolean = true
    string = "foo"
    invalid = "a + b"
    "###);
}

#[test]
fn test_config_set_type_mismatch() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-table.foo", "test-val"],
    );
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["config", "set", "--user", "test-table", "not-a-table"],
    );
    insta::assert_snapshot!(stderr, @r"
    Error: Failed to set test-table
    Caused by: Would overwrite entire table test-table
    ");

    // But it's fine to overwrite arrays and inline tables
    test_env.jj_cmd_success(
        &repo_path,
        &["config", "set", "--user", "test-table.array", "[1,2,3]"],
    );
    test_env.jj_cmd_success(
        &repo_path,
        &["config", "set", "--user", "test-table.array", "[4,5,6]"],
    );
    test_env.jj_cmd_success(
        &repo_path,
        &["config", "set", "--user", "test-table.inline", "{ x = 42}"],
    );
    test_env.jj_cmd_success(
        &repo_path,
        &["config", "set", "--user", "test-table.inline", "42"],
    );
}

#[test]
fn test_config_set_nontable_parent() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-nontable", "test-val"],
    );
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["config", "set", "--user", "test-nontable.foo", "test-val"],
    );
    insta::assert_snapshot!(stderr, @r"
    Error: Failed to set test-nontable.foo
    Caused by: Would overwrite non-table value with parent table test-nontable
    ");
}

#[test]
fn test_config_unset_non_existent_key() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["config", "unset", "--user", "nonexistent"]);
    insta::assert_snapshot!(stderr, @r#"Error: "nonexistent" doesn't exist"#);
}

#[test]
fn test_config_unset_inline_table_key() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "inline-table", "{ foo = true }"],
    );
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["config", "unset", "--user", "inline-table.foo"],
    );

    insta::assert_snapshot!(stderr, @r#"Error: "inline-table.foo" doesn't exist"#);
}

#[test]
fn test_config_unset_table_like() {
    let mut test_env = TestEnvironment::default();
    // Test with fresh new config file
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(&user_config_path);

    std::fs::write(
        &user_config_path,
        indoc! {b"
            inline-table = { foo = true }
            [non-inline-table]
            foo = true
        "},
    )
    .unwrap();

    // Inline table is a "value", so it can be deleted.
    test_env.jj_cmd_success(
        test_env.env_root(),
        &["config", "unset", "--user", "inline-table"],
    );
    // Non-inline table cannot be deleted.
    let stderr = test_env.jj_cmd_failure(
        test_env.env_root(),
        &["config", "unset", "--user", "non-inline-table"],
    );
    insta::assert_snapshot!(stderr, @r"
    Error: Failed to unset non-inline-table
    Caused by: Would delete entire table non-inline-table
    ");

    let user_config_toml = std::fs::read_to_string(&user_config_path).unwrap();
    insta::assert_snapshot!(user_config_toml, @r"
    [non-inline-table]
    foo = true
    ");
}

#[test]
fn test_config_unset_for_user() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    // Test with fresh new config file
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(&user_config_path);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["config", "set", "--user", "foo", "true"]);
    test_env.jj_cmd_ok(&repo_path, &["config", "unset", "--user", "foo"]);

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "table.foo", "true"],
    );
    test_env.jj_cmd_ok(&repo_path, &["config", "unset", "--user", "table.foo"]);

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "table.inline", "{ foo = true }"],
    );
    test_env.jj_cmd_ok(&repo_path, &["config", "unset", "--user", "table.inline"]);

    let user_config_toml = std::fs::read_to_string(&user_config_path).unwrap();
    insta::assert_snapshot!(user_config_toml, @r#"
        [table]
        "#);
}

#[test]
fn test_config_unset_for_repo() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--repo", "test-key", "test-val"],
    );
    test_env.jj_cmd_ok(&repo_path, &["config", "unset", "--repo", "test-key"]);

    let repo_config_path = repo_path.join(".jj/repo/config.toml");
    let repo_config_toml = std::fs::read_to_string(repo_config_path).unwrap();
    insta::assert_snapshot!(repo_config_toml, @"");
}

#[test]
fn test_config_edit_missing_opt() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["config", "edit"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      <--user|--repo>

    Usage: jj config edit <--user|--repo>

    For more information, try '--help'.
    "###);
}

#[test]
fn test_config_edit_user() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    // Remove one of the config file to disambiguate
    std::fs::remove_file(test_env.last_config_file_path()).unwrap();
    let edit_script = test_env.set_up_fake_editor();

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["config", "edit", "--user"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    assert_eq!(
        edited_path,
        dunce::simplified(&test_env.last_config_file_path())
    );
}

#[test]
fn test_config_edit_user_new_file() {
    let mut test_env = TestEnvironment::default();
    let user_config_path = test_env.config_path().join("config").join("file.toml");
    test_env.set_up_fake_editor(); // set $EDIT_SCRIPT, but added configuration is ignored
    test_env.add_env_var("EDITOR", fake_editor_path());
    test_env.set_config_path(&user_config_path);
    assert!(!user_config_path.exists());

    test_env.jj_cmd_ok(test_env.env_root(), &["config", "edit", "--user"]);
    assert!(
        user_config_path.exists(),
        "new file and directory should be created"
    );
}

#[test]
fn test_config_edit_repo() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let repo_config_path = repo_path.join(PathBuf::from_iter([".jj", "repo", "config.toml"]));
    let edit_script = test_env.set_up_fake_editor();
    assert!(!repo_config_path.exists());

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["config", "edit", "--repo"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    assert_eq!(edited_path, dunce::simplified(&repo_config_path));
    assert!(repo_config_path.exists(), "new file should be created");
}

#[test]
fn test_config_path() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let user_config_path = test_env.env_root().join("config.toml");
    let repo_config_path = repo_path.join(PathBuf::from_iter([".jj", "repo", "config.toml"]));
    test_env.set_config_path(&user_config_path);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["config", "path", "--user"]),
        @"$TEST_ENV/config.toml");
    assert!(
        !user_config_path.exists(),
        "jj config path shouldn't create new file"
    );

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["config", "path", "--repo"]),
        @"$TEST_ENV/repo/.jj/repo/config.toml");
    assert!(
        !repo_config_path.exists(),
        "jj config path shouldn't create new file"
    );

    insta::assert_snapshot!(
        test_env.jj_cmd_failure(test_env.env_root(), &["config", "path", "--repo"]),
        @"Error: No repo config path found");
}

#[test]
fn test_config_edit_repo_outside_repo() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["config", "edit", "--repo"]);
    insta::assert_snapshot!(stderr, @"Error: No repo config path found to edit");
}

#[test]
fn test_config_get() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    [table]
    string = "some value 1"
    int = 123
    list = ["list", "value"]
    overridden = "foo"
    "#,
    );
    test_env.add_config(
        r#"
    [table]
    overridden = "bar"
    "#,
    );

    let stdout = test_env.jj_cmd_failure(test_env.env_root(), &["config", "get", "nonexistent"]);
    insta::assert_snapshot!(stdout, @r"
    Config error: Value not found for nonexistent
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "table.string"]);
    insta::assert_snapshot!(stdout, @r###"
    some value 1
    "###);

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "table.int"]);
    insta::assert_snapshot!(stdout, @r###"
    123
    "###);

    let stdout = test_env.jj_cmd_failure(test_env.env_root(), &["config", "get", "table.list"]);
    insta::assert_snapshot!(stdout, @r"
    Config error: Invalid type or value for table.list
    Caused by: Expected a value convertible to a string, but is an array
    Hint: Check the config file: $TEST_ENV/config/config0002.toml
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");

    let stdout = test_env.jj_cmd_failure(test_env.env_root(), &["config", "get", "table"]);
    insta::assert_snapshot!(stdout, @r"
    Config error: Invalid type or value for table
    Caused by: Expected a value convertible to a string, but is a table
    Hint: Check the config file: $TEST_ENV/config/config0003.toml
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");

    let stdout =
        test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "table.overridden"]);
    insta::assert_snapshot!(stdout, @"bar");
}

#[test]
fn test_config_path_syntax() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"
    a.'b()' = 0
    'b c'.d = 1
    'b c'.e.'f[]' = 2
    - = 3
    _ = 4
    '.' = 5
    "#,
    );

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "a.'b()'"]);
    insta::assert_snapshot!(stdout, @r###"
    a.'b()' = 0
    "###);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "'b c'"]);
    insta::assert_snapshot!(stdout, @r###"
    'b c'.d = 1
    'b c'.e."f[]" = 2
    "###);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "'b c'.d"]);
    insta::assert_snapshot!(stdout, @r###"
    'b c'.d = 1
    "###);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "'b c'.e.'f[]'"]);
    insta::assert_snapshot!(stdout, @r###"
    'b c'.e.'f[]' = 2
    "###);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "'b c'.e.'f[]'"]);
    insta::assert_snapshot!(stdout, @r###"
    2
    "###);

    // Not a table
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["config", "list", "a.'b()'.x"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: No matching config key for a.'b()'.x
    "###);
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["config", "get", "a.'b()'.x"]);
    insta::assert_snapshot!(stderr, @r"
    Config error: Value not found for a.'b()'.x
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");

    // "-" and "_" are valid TOML keys
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "-"]);
    insta::assert_snapshot!(stdout, @r###"
    - = 3
    "###);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "_"]);
    insta::assert_snapshot!(stdout, @r###"
    _ = 4
    "###);

    // "." requires quoting
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "'.'"]);
    insta::assert_snapshot!(stdout, @r###"
    '.' = 5
    "###);
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "'.'"]);
    insta::assert_snapshot!(stdout, @r###"
    5
    "###);
    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["config", "get", "."]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value '.' for '<NAME>': TOML parse error at line 1, column 1
      |
    1 | .
      | ^
    invalid key


    For more information, try '--help'.
    "###);

    // Invalid TOML keys
    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["config", "list", "b c"]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value 'b c' for '[NAME]': TOML parse error at line 1, column 3
      |
    1 | b c
      |   ^



    For more information, try '--help'.
    "###);
    let stderr = test_env.jj_cmd_cli_error(test_env.env_root(), &["config", "list", ""]);
    insta::assert_snapshot!(stderr, @r###"
    error: invalid value '' for '[NAME]': TOML parse error at line 1, column 1
      |
    1 | 
      | ^
    invalid key


    For more information, try '--help'.
    "###);
}

#[test]
#[cfg_attr(windows, ignore = "dirs::home_dir() can't be overridden by $HOME")] // TODO
fn test_config_conditional() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.home_dir(), &["git", "init", "repo1"]);
    test_env.jj_cmd_ok(test_env.home_dir(), &["git", "init", "repo2"]);
    let repo1_path = test_env.home_dir().join("repo1");
    let repo2_path = test_env.home_dir().join("repo2");
    // Test with fresh new config file
    let user_config_path = test_env.env_root().join("config.toml");
    test_env.set_config_path(&user_config_path);
    std::fs::write(
        &user_config_path,
        indoc! {"
            foo = 'global'
            [[--scope]]
            --when.repositories = ['~/repo1']
            foo = 'repo1'
            [[--scope]]
            --when.repositories = ['~/repo2']
            foo = 'repo2'
        "},
    )
    .unwrap();

    // get and list should refer to the resolved config
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "foo"]);
    insta::assert_snapshot!(stdout, @"global");
    let stdout = test_env.jj_cmd_success(&repo1_path, &["config", "get", "foo"]);
    insta::assert_snapshot!(stdout, @"repo1");
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "list", "--user"]);
    insta::assert_snapshot!(stdout, @"foo = 'global'");
    let stdout = test_env.jj_cmd_success(&repo1_path, &["config", "list", "--user"]);
    insta::assert_snapshot!(stdout, @"foo = 'repo1'");
    let stdout = test_env.jj_cmd_success(&repo2_path, &["config", "list", "--user"]);
    insta::assert_snapshot!(stdout, @"foo = 'repo2'");

    // relative workspace path
    let stdout = test_env.jj_cmd_success(&repo2_path, &["config", "list", "--user", "-R../repo1"]);
    insta::assert_snapshot!(stdout, @"foo = 'repo1'");

    // set and unset should refer to the source config
    // (there's no option to update scoped table right now.)
    let (_stdout, stderr) = test_env.jj_cmd_ok(
        test_env.env_root(),
        &["config", "set", "--user", "bar", "new value"],
    );
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(std::fs::read_to_string(&user_config_path).unwrap(), @r#"
    foo = 'global'
    bar = "new value"
    [[--scope]]
    --when.repositories = ['~/repo1']
    foo = 'repo1'
    [[--scope]]
    --when.repositories = ['~/repo2']
    foo = 'repo2'
    "#);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo1_path, &["config", "unset", "--user", "foo"]);
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(std::fs::read_to_string(&user_config_path).unwrap(), @r#"
    bar = "new value"
    [[--scope]]
    --when.repositories = ['~/repo1']
    foo = 'repo1'
    [[--scope]]
    --when.repositories = ['~/repo2']
    foo = 'repo2'
    "#);
}

// Minimal test for Windows where the home directory can't be switched.
// (Can be removed if test_config_conditional() is enabled on Windows.)
#[test]
fn test_config_conditional_without_home_dir() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    // Test with fresh new config file
    let user_config_path = test_env.env_root().join("config.toml");
    test_env.set_config_path(&user_config_path);
    std::fs::write(
        &user_config_path,
        format!(
            indoc! {"
                foo = 'global'
                [[--scope]]
                --when.repositories = [{repo_path}]
                foo = 'repo'
            "},
            // "\\?\" paths shouldn't be required on Windows
            repo_path = to_toml_value(dunce::simplified(&repo_path).to_str().unwrap())
        ),
    )
    .unwrap();

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "foo"]);
    insta::assert_snapshot!(stdout, @"global");
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "get", "foo"]);
    insta::assert_snapshot!(stdout, @"repo");
}

#[test]
fn test_config_show_paths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "ui.paginate", ":builtin"],
    );
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["st"]);
    insta::assert_snapshot!(stderr, @r"
    Config error: Invalid type or value for ui.paginate
    Caused by: unknown variant `:builtin`, expected `never` or `auto`

    Hint: Check the config file: $TEST_ENV/config/config0001.toml
    For help, see https://jj-vcs.github.io/jj/latest/config/.
    ");
}

#[test]
fn test_config_author_change_warning() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--repo", "user.email", "'Foo'"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: This setting will only impact future commits.
    The author of the working copy will stay "Test User <test.user@example.com>".
    To change the working copy author, use "jj describe --reset-author --no-edit"
    "###);

    // test_env.jj_cmd resets state for every invocation
    // for this test, the state (user.email) is needed
    let mut log_cmd = test_env.jj_cmd(&repo_path, &["describe", "--reset-author", "--no-edit"]);
    log_cmd.env_remove("JJ_EMAIL");
    log_cmd.assert().success();

    let (stdout, _) = test_env.jj_cmd_ok(&repo_path, &["log"]);
    assert!(stdout.contains("Foo"));
}

#[test]
fn test_config_author_change_warning_root_env() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(
        test_env.env_root(),
        &["config", "set", "--user", "user.email", "'Foo'"],
    );
}

fn find_stdout_lines(keyname_pattern: &str, stdout: &str) -> String {
    let key_line_re = Regex::new(&format!(r"(?m)^{keyname_pattern} = .*$")).unwrap();
    key_line_re
        .find_iter(stdout)
        .map(|m| m.as_str())
        .collect_vec()
        .join("\n")
}
