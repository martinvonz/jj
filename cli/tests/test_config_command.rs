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

use insta::assert_snapshot;
use itertools::Itertools;
use regex::Regex;

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
    test-table.y.bar = 123
    test-table.y.foo = "abc"
    test-table.z."with space"."function()" = 5
    "###);
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
fn test_config_list_inline_table() {
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
    test-table.x = true
    test-table.y.bar = 123
    test-table.y.foo = "abc"
    test-val = [1, 2, 3]
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
    insta::assert_snapshot!(stdout, @r###"
    multiline = """
    foo
    bar
    """
    "###);

    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "config",
            "list",
            "multiline",
            "--include-overridden",
            "--config-toml=multiline='single'",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    # multiline = """
    # foo
    # bar
    # """
    multiline = "single"
    "###);
}

#[test]
fn test_config_list_layer() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path.to_owned());
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
    test_env.add_config(&format!("{config_key} = {value:?}\n", value = "user"));
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    merge-tools.vimdiff.program = "user"
    "###);

    // Repo
    std::fs::write(
        repo_path.join(".jj/repo/config.toml"),
        format!("{config_key} = {value:?}\n", value = "repo"),
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
            "--config-toml",
            &format!("{config_key}={value:?}", value = "command-arg"),
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
            "--config-toml",
            &format!("{config_key}={value:?}", value = "command-arg"),
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
    test_env.add_config(&format!("{config_key} = {value:?}\n", value = "user"));
    let stdout = test_env.jj_cmd_success(&repo_path, &["config", "list", config_key]);
    insta::assert_snapshot!(stdout, @r###"
    ui.editor = "user"
    "###);

    // Repo
    std::fs::write(
        repo_path.join(".jj/repo/config.toml"),
        format!("{config_key} = {value:?}\n", value = "repo"),
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
            "--config-toml",
            &format!("{config_key}={value:?}", value = "command-arg"),
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
            "--config-toml",
            &format!("{config_key}={value:?}", value = "command-arg"),
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
        format!("{config_key} = {value:?}\n", value = "main-repo"),
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
}

#[test]
fn test_config_set_for_user() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    // Point to a config file since `config set` can't handle directories.
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path.to_owned());
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
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path.clone());
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
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-table.foo", "test-val"],
    );
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["config", "set", "--user", "test-table", "not-a-table"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to set test-table: would overwrite entire table
    "###);

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
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "test-nontable", "test-val"],
    );
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["config", "set", "--user", "test-nontable.foo", "test-val"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to set test-nontable.foo: would overwrite non-table value with parent table
    "###);
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
    let edit_script = test_env.set_up_fake_editor();

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["config", "edit", "--user"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    assert_eq!(edited_path, dunce::simplified(test_env.config_path()));
}

#[test]
fn test_config_edit_repo() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let edit_script = test_env.set_up_fake_editor();

    std::fs::write(edit_script, "dump-path path").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["config", "edit", "--repo"]);

    let edited_path =
        PathBuf::from(std::fs::read_to_string(test_env.env_root().join("path")).unwrap());
    assert_eq!(
        edited_path,
        dunce::simplified(&repo_path.join(".jj/repo/config.toml"))
    );
}

#[test]
fn test_config_path() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    assert_snapshot!(
      test_env.jj_cmd_success(&repo_path, &["config", "path", "--user"]),
      @r###"
      $TEST_ENV/config
      "###
    );
    assert_snapshot!(
      test_env.jj_cmd_success(&repo_path, &["config", "path", "--repo"]),
      @r###"
      $TEST_ENV/repo/.jj/repo/config.toml
      "###
    );
}

#[test]
fn test_config_edit_repo_outside_repo() {
    let test_env = TestEnvironment::default();
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["config", "edit", "--repo"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: There is no jj repo in "."
    "###);
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
    insta::assert_snapshot!(stdout, @r###"
    Config error: configuration property "nonexistent" not found
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "table.string"]);
    insta::assert_snapshot!(stdout, @r###"
    some value 1
    "###);

    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["config", "get", "table.int"]);
    insta::assert_snapshot!(stdout, @r###"
    123
    "###);

    let stdout = test_env.jj_cmd_failure(test_env.env_root(), &["config", "get", "table.list"]);
    insta::assert_snapshot!(stdout, @r###"
    Config error: invalid type: sequence, expected a value convertible to a string
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

    let stdout = test_env.jj_cmd_failure(test_env.env_root(), &["config", "get", "table"]);
    insta::assert_snapshot!(stdout, @r###"
    Config error: invalid type: map, expected a value convertible to a string
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

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
    insta::assert_snapshot!(stderr, @r###"
    Config error: configuration property "a.'b()'.x" not found
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

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
fn test_config_show_paths() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path.to_owned());
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(
        &repo_path,
        &["config", "set", "--user", "ui.paginate", ":builtin"],
    );
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["st"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: Invalid `ui.paginate`
    Caused by: enum PaginationChoice does not have variant constructor :builtin
    Hint: Check the following config files:
    - $TEST_ENV/config/config.toml
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);
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
    let mut test_env = TestEnvironment::default();
    let user_config_path = test_env.config_path().join("config.toml");
    test_env.set_config_path(user_config_path);
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
