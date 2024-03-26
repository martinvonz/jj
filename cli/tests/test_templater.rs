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
fn test_templater_parse_error() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render_err = |template| test_env.jj_cmd_failure(&repo_path, &["log", "-T", template]);

    insta::assert_snapshot!(render_err(r#"description ()"#), @r###"
    Error: Failed to parse template: Syntax error
    Caused by:  --> 1:13
      |
    1 | description ()
      |             ^---
      |
      = expected <EOI>, `++`, `||`, or `&&`
    "###);

    // Typo
    test_env.add_config(
        r###"
    [template-aliases]
    'conflicting' = ''
    'shorted()' = ''
    'socat(x)' = 'x'
    'format_id(id)' = 'id.sort()'
    "###,
    );
    insta::assert_snapshot!(render_err(r#"conflicts"#), @r###"
    Error: Failed to parse template: Keyword "conflicts" doesn't exist
    Caused by:  --> 1:1
      |
    1 | conflicts
      | ^-------^
      |
      = Keyword "conflicts" doesn't exist
    Hint: Did you mean "conflict", "conflicting"?
    "###);
    insta::assert_snapshot!(render_err(r#"commit_id.shorter()"#), @r###"
    Error: Failed to parse template: Method "shorter" doesn't exist for type "CommitOrChangeId"
    Caused by:  --> 1:11
      |
    1 | commit_id.shorter()
      |           ^-----^
      |
      = Method "shorter" doesn't exist for type "CommitOrChangeId"
    Hint: Did you mean "short", "shortest"?
    "###);
    insta::assert_snapshot!(render_err(r#"oncat()"#), @r###"
    Error: Failed to parse template: Function "oncat" doesn't exist
    Caused by:  --> 1:1
      |
    1 | oncat()
      | ^---^
      |
      = Function "oncat" doesn't exist
    Hint: Did you mean "concat", "socat"?
    "###);
    insta::assert_snapshot!(render_err(r#""".lines().map(|s| se)"#), @r###"
    Error: Failed to parse template: Keyword "se" doesn't exist
    Caused by:  --> 1:20
      |
    1 | "".lines().map(|s| se)
      |                    ^^
      |
      = Keyword "se" doesn't exist
    Hint: Did you mean "s", "self"?
    "###);
    insta::assert_snapshot!(render_err(r#"format_id(commit_id)"#), @r###"
    Error: Failed to parse template: Alias "format_id()" cannot be expanded
    Caused by:
    1:  --> 1:1
      |
    1 | format_id(commit_id)
      | ^------------------^
      |
      = Alias "format_id()" cannot be expanded
    2:  --> 1:4
      |
    1 | id.sort()
      |    ^--^
      |
      = Method "sort" doesn't exist for type "CommitOrChangeId"
    Hint: Did you mean "short", "shortest"?
    "###);

    // -Tbuiltin shows the predefined builtin_* aliases. This isn't 100%
    // guaranteed, but is nice.
    insta::assert_snapshot!(render_err(r#"builtin"#), @r###"
    Error: Failed to parse template: Keyword "builtin" doesn't exist
    Caused by:  --> 1:1
      |
    1 | builtin
      | ^-----^
      |
      = Keyword "builtin" doesn't exist
    Hint: Did you mean "builtin_change_id_with_hidden_and_divergent_info", "builtin_log_comfortable", "builtin_log_compact", "builtin_log_detailed", "builtin_log_oneline", "builtin_op_log_comfortable", "builtin_op_log_compact"?
    "###);
}

#[test]
fn test_templater_upper_lower() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    insta::assert_snapshot!(
      render(r#"change_id.shortest(4).upper() ++ change_id.shortest(4).upper().lower()"#),
      @"[1m[38;5;5mZ[0m[38;5;8mZZZ[39m[1m[38;5;5mz[0m[38;5;8mzzz[39m");
    insta::assert_snapshot!(
      render(r#""Hello".upper() ++ "Hello".lower()"#), @"HELLOhello");
}

#[test]
fn test_templater_alias() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_template_output(&test_env, &repo_path, "@-", template);
    let render_err = |template| test_env.jj_cmd_failure(&repo_path, &["log", "-T", template]);

    test_env.add_config(
        r###"
    [template-aliases]
    'my_commit_id' = 'commit_id.short()'
    'syntax_error' = 'foo.'
    'name_error' = 'unknown_id'
    'recurse' = 'recurse1'
    'recurse1' = 'recurse2()'
    'recurse2()' = 'recurse'
    'identity(x)' = 'x'
    'coalesce(x, y)' = 'if(x, x, y)'
    "###,
    );

    insta::assert_snapshot!(render("my_commit_id"), @"000000000000");
    insta::assert_snapshot!(render("identity(my_commit_id)"), @"000000000000");

    insta::assert_snapshot!(render_err("commit_id ++ syntax_error"), @r###"
    Error: Failed to parse template: Alias "syntax_error" cannot be expanded
    Caused by:
    1:  --> 1:14
      |
    1 | commit_id ++ syntax_error
      |              ^----------^
      |
      = Alias "syntax_error" cannot be expanded
    2:  --> 1:5
      |
    1 | foo.
      |     ^---
      |
      = expected <identifier>
    "###);

    insta::assert_snapshot!(render_err("commit_id ++ name_error"), @r###"
    Error: Failed to parse template: Alias "name_error" cannot be expanded
    Caused by:
    1:  --> 1:14
      |
    1 | commit_id ++ name_error
      |              ^--------^
      |
      = Alias "name_error" cannot be expanded
    2:  --> 1:1
      |
    1 | unknown_id
      | ^--------^
      |
      = Keyword "unknown_id" doesn't exist
    "###);

    insta::assert_snapshot!(render_err(r#"identity(identity(commit_id.short("")))"#), @r###"
    Error: Failed to parse template: Alias "identity()" cannot be expanded
    Caused by:
    1:  --> 1:1
      |
    1 | identity(identity(commit_id.short("")))
      | ^-------------------------------------^
      |
      = Alias "identity()" cannot be expanded
    2:  --> 1:10
      |
    1 | identity(identity(commit_id.short("")))
      |          ^---------------------------^
      |
      = Alias "identity()" cannot be expanded
    3:  --> 1:35
      |
    1 | identity(identity(commit_id.short("")))
      |                                   ^^
      |
      = Expected expression of type "Integer"
    "###);

    insta::assert_snapshot!(render_err("commit_id ++ recurse"), @r###"
    Error: Failed to parse template: Alias "recurse" cannot be expanded
    Caused by:
    1:  --> 1:14
      |
    1 | commit_id ++ recurse
      |              ^-----^
      |
      = Alias "recurse" cannot be expanded
    2:  --> 1:1
      |
    1 | recurse1
      | ^------^
      |
      = Alias "recurse1" cannot be expanded
    3:  --> 1:1
      |
    1 | recurse2()
      | ^--------^
      |
      = Alias "recurse2()" cannot be expanded
    4:  --> 1:1
      |
    1 | recurse
      | ^-----^
      |
      = Alias "recurse" expanded recursively
    "###);

    insta::assert_snapshot!(render_err("identity()"), @r###"
    Error: Failed to parse template: Function "identity": Expected 1 arguments
    Caused by:  --> 1:10
      |
    1 | identity()
      |          ^
      |
      = Function "identity": Expected 1 arguments
    "###);
    insta::assert_snapshot!(render_err("identity(commit_id, commit_id)"), @r###"
    Error: Failed to parse template: Function "identity": Expected 1 arguments
    Caused by:  --> 1:10
      |
    1 | identity(commit_id, commit_id)
      |          ^------------------^
      |
      = Function "identity": Expected 1 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"coalesce(label("x", "not boolean"), "")"#), @r###"
    Error: Failed to parse template: Alias "coalesce()" cannot be expanded
    Caused by:
    1:  --> 1:1
      |
    1 | coalesce(label("x", "not boolean"), "")
      | ^-------------------------------------^
      |
      = Alias "coalesce()" cannot be expanded
    2:  --> 1:10
      |
    1 | coalesce(label("x", "not boolean"), "")
      |          ^-----------------------^
      |
      = Expected expression of type "Boolean"
    "###);
}

#[test]
fn test_templater_alias_override() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r#"
    [template-aliases]
    'f(x)' = '"user"'
    "#,
    );

    // 'f(x)' should be overridden by --config-toml 'f(a)'. If aliases were sorted
    // purely by name, 'f(a)' would come first.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--no-graph",
            "-r@",
            "-T",
            r#"f(_)"#,
            "--config-toml",
            r#"template-aliases.'f(a)' = '"arg"'"#,
        ],
    );
    insta::assert_snapshot!(stdout, @"arg");
}

#[test]
fn test_templater_bad_alias_decl() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r###"
    [template-aliases]
    'badfn(a, a)' = 'a'
    'my_commit_id' = 'commit_id.short()'
    "###,
    );

    // Invalid declaration should be warned and ignored.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["log", "--no-graph", "-r@-", "-Tmy_commit_id"]);
    insta::assert_snapshot!(stdout, @"000000000000");
    insta::assert_snapshot!(stderr, @r###"
    Warning: Failed to load "template-aliases.badfn(a, a)":  --> 1:7
      |
    1 | badfn(a, a)
      |       ^--^
      |
      = Redefinition of function parameter
    "###);
}

fn get_template_output(
    test_env: &TestEnvironment,
    repo_path: &Path,
    rev: &str,
    template: &str,
) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "--no-graph", "-r", rev, "-T", template])
}

fn get_colored_template_output(
    test_env: &TestEnvironment,
    repo_path: &Path,
    rev: &str,
    template: &str,
) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "--color=always",
            "--no-graph",
            "-r",
            rev,
            "-T",
            template,
        ],
    )
}
