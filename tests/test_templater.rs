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

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_templater_branches() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    // Created some branches on the remote
    test_env.jj_cmd_success(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch1"]);
    test_env.jj_cmd_success(&origin_path, &["new", "root", "-m=description 2"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch2"]);
    test_env.jj_cmd_success(&origin_path, &["new", "root", "-m=description 3"]);
    test_env.jj_cmd_success(&origin_path, &["branch", "create", "branch3"]);
    test_env.jj_cmd_success(&origin_path, &["git", "export"]);
    test_env.jj_cmd_success(
        test_env.env_root(),
        &[
            "git",
            "clone",
            origin_git_repo_path.to_str().unwrap(),
            "local",
        ],
    );
    let workspace_root = test_env.env_root().join("local");

    // Rewrite branch1, move branch2 forward, create conflict in branch3, add
    // new-branch
    test_env.jj_cmd_success(
        &workspace_root,
        &["describe", "branch1", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_success(&workspace_root, &["new", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "set", "branch2"]);
    test_env.jj_cmd_success(&workspace_root, &["branch", "create", "new-branch"]);
    test_env.jj_cmd_success(&workspace_root, &["describe", "branch3", "-m=local"]);
    test_env.jj_cmd_success(&origin_path, &["describe", "branch3", "-m=origin"]);
    test_env.jj_cmd_success(&origin_path, &["git", "export"]);
    test_env.jj_cmd_success(&workspace_root, &["git", "fetch"]);

    let template = r#"commit_id.short() " " branches"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    o  b1bb3766d584 branch3??
    │ @  a5b4d15489cc branch2* new-branch
    │ │ o  21c33875443e branch1*
    ├───╯
    │ o  8476341eb395 branch2@origin
    ├─╯
    o  000000000000
    "###);
}

#[test]
fn test_templater_parsed_tree() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_template_output(&test_env, &repo_path, "@-", template);

    // Empty
    insta::assert_snapshot!(render(r#"  "#), @"");

    // Single term with whitespace
    insta::assert_snapshot!(render(r#"  commit_id.short()  "#), @"000000000000");

    // Multiple terms
    insta::assert_snapshot!(render(r#"  commit_id.short()  empty "#), @"000000000000true");

    // Parenthesized single term
    insta::assert_snapshot!(render(r#"(commit_id.short())"#), @"000000000000");

    // Parenthesized multiple terms and concatenation
    insta::assert_snapshot!(render(r#"(commit_id.short() " ") empty"#), @"000000000000 true");

    // Parenthesized "if" condition
    insta::assert_snapshot!(render(r#"if((divergent), "t", "f")"#), @"f");

    // Parenthesized method chaining
    insta::assert_snapshot!(render(r#"(commit_id).short()"#), @"000000000000");
}

#[test]
fn test_templater_parse_error() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render_err = |template| test_env.jj_cmd_failure(&repo_path, &["log", "-T", template]);

    insta::assert_snapshot!(render_err(r#"description ()"#), @r###"
    Error: Failed to parse template:  --> 1:14
      |
    1 | description ()
      |              ^---
      |
      = expected template
    "###);

    insta::assert_snapshot!(render_err(r#"foo"#), @r###"
    Error: Failed to parse template:  --> 1:1
      |
    1 | foo
      | ^-^
      |
      = Keyword "foo" doesn't exist
    "###);

    insta::assert_snapshot!(render_err(r#"foo()"#), @r###"
    Error: Failed to parse template:  --> 1:1
      |
    1 | foo()
      | ^-^
      |
      = Function "foo" doesn't exist
    "###);

    insta::assert_snapshot!(render_err(r#"description.first_line().foo()"#), @r###"
    Error: Failed to parse template:  --> 1:26
      |
    1 | description.first_line().foo()
      |                          ^-^
      |
      = Method "foo" doesn't exist for type "String"
    "###);

    insta::assert_snapshot!(render_err(r#"10000000000000000000"#), @r###"
    Error: Failed to parse template:  --> 1:1
      |
    1 | 10000000000000000000
      | ^------------------^
      |
      = Invalid integer literal: number too large to fit in target type
    "###);
    insta::assert_snapshot!(render_err(r#"42.foo()"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | 42.foo()
      |    ^-^
      |
      = Method "foo" doesn't exist for type "Integer"
    "###);

    insta::assert_snapshot!(render_err(r#"("foo" "bar").baz()"#), @r###"
    Error: Failed to parse template:  --> 1:15
      |
    1 | ("foo" "bar").baz()
      |               ^-^
      |
      = Method "baz" doesn't exist for type "Template"
    "###);

    insta::assert_snapshot!(render_err(r#"description.contains()"#), @r###"
    Error: Failed to parse template:  --> 1:22
      |
    1 | description.contains()
      |                      ^
      |
      = Expected 1 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"description.first_line("foo")"#), @r###"
    Error: Failed to parse template:  --> 1:24
      |
    1 | description.first_line("foo")
      |                        ^---^
      |
      = Expected 0 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"label()"#), @r###"
    Error: Failed to parse template:  --> 1:7
      |
    1 | label()
      |       ^
      |
      = Expected 2 arguments
    "###);
    insta::assert_snapshot!(render_err(r#"label("foo", "bar", "baz")"#), @r###"
    Error: Failed to parse template:  --> 1:7
      |
    1 | label("foo", "bar", "baz")
      |       ^-----------------^
      |
      = Expected 2 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"if()"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | if()
      |    ^
      |
      = Expected 2 to 3 arguments
    "###);
    insta::assert_snapshot!(render_err(r#"if("foo", "bar", "baz", "quux")"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | if("foo", "bar", "baz", "quux")
      |    ^-------------------------^
      |
      = Expected 2 to 3 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"if(label("foo", "bar"), "baz")"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | if(label("foo", "bar"), "baz")
      |    ^-----------------^
      |
      = Expected argument of type "Boolean"
    "###);
}

#[test]
fn test_templater_string_method() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_success(&repo_path, &["commit", "-m=description 1"]);
    let render = |template| get_template_output(&test_env, &repo_path, "@-", template);

    insta::assert_snapshot!(render(r#""fooo".contains("foo")"#), @"true");
    insta::assert_snapshot!(render(r#""foo".contains("fooo")"#), @"false");
    insta::assert_snapshot!(render(r#"description.contains("description")"#), @"true");
    insta::assert_snapshot!(
        render(r#""description 123".contains(description.first_line())"#), @"true");

    insta::assert_snapshot!(render(r#""".first_line()"#), @"");
    insta::assert_snapshot!(render(r#""foo\nbar".first_line()"#), @"foo");
}

#[test]
fn test_templater_signature() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_template_output(&test_env, &repo_path, "@", template);

    test_env.jj_cmd_success(&repo_path, &["new"]);

    insta::assert_snapshot!(render(r#"author"#), @"Test User <test.user@example.com>");
    insta::assert_snapshot!(render(r#"author.name()"#), @"Test User");
    insta::assert_snapshot!(render(r#"author.email()"#), @"test.user@example.com");
    insta::assert_snapshot!(render(r#"author.username()"#), @"test.user");

    test_env.jj_cmd_success(
        &repo_path,
        &["--config-toml=user.name='Another Test User'", "new"],
    );

    insta::assert_snapshot!(render(r#"author"#), @"Another Test User <test.user@example.com>");
    insta::assert_snapshot!(render(r#"author.name()"#), @"Another Test User");
    insta::assert_snapshot!(render(r#"author.email()"#), @"test.user@example.com");
    insta::assert_snapshot!(render(r#"author.username()"#), @"test.user");

    test_env.jj_cmd_success(
        &repo_path,
        &[
            "--config-toml=user.email='test.user@invalid@example.com'",
            "new",
        ],
    );

    insta::assert_snapshot!(render(r#"author"#), @"Test User <test.user@invalid@example.com>");
    insta::assert_snapshot!(render(r#"author.name()"#), @"Test User");
    insta::assert_snapshot!(render(r#"author.email()"#), @"test.user@invalid@example.com");
    insta::assert_snapshot!(render(r#"author.username()"#), @"test.user");

    test_env.jj_cmd_success(&repo_path, &["--config-toml=user.email='test.user'", "new"]);

    insta::assert_snapshot!(render(r#"author"#), @"Test User <test.user>");
    insta::assert_snapshot!(render(r#"author.email()"#), @"test.user");
    insta::assert_snapshot!(render(r#"author.username()"#), @"test.user");

    test_env.jj_cmd_success(
        &repo_path,
        &[
            "--config-toml=user.email='test.user+tag@example.com'",
            "new",
        ],
    );

    insta::assert_snapshot!(render(r#"author"#), @"Test User <test.user+tag@example.com>");
    insta::assert_snapshot!(render(r#"author.email()"#), @"test.user+tag@example.com");
    insta::assert_snapshot!(render(r#"author.username()"#), @"test.user+tag");

    test_env.jj_cmd_success(&repo_path, &["--config-toml=user.email='x@y'", "new"]);

    insta::assert_snapshot!(render(r#"author"#), @"Test User <x@y>");
    insta::assert_snapshot!(render(r#"author.email()"#), @"x@y");
    insta::assert_snapshot!(render(r#"author.username()"#), @"x");
}

#[test]
fn test_templater_label_function() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    // Literal
    insta::assert_snapshot!(render(r#"label("error", "text")"#), @"[38;5;1mtext[39m");

    // Evaluated property
    insta::assert_snapshot!(
        render(r#"label("error".first_line(), "text")"#), @"[38;5;1mtext[39m");

    // Template
    insta::assert_snapshot!(
        render(r#"label(if(empty, "error", "warning"), "text")"#), @"[38;5;1mtext[39m");
}

#[test]
fn test_templater_separate_function() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    insta::assert_snapshot!(render(r#"separate(" ")"#), @"");
    insta::assert_snapshot!(render(r#"separate(" ", "")"#), @"");
    insta::assert_snapshot!(render(r#"separate(" ", "a")"#), @"a");
    insta::assert_snapshot!(render(r#"separate(" ", "a", "b")"#), @"a b");
    insta::assert_snapshot!(render(r#"separate(" ", "a", "", "b")"#), @"a b");
    insta::assert_snapshot!(render(r#"separate(" ", "a", "b", "")"#), @"a b");
    insta::assert_snapshot!(render(r#"separate(" ", "", "a", "b")"#), @"a b");

    // Labeled
    insta::assert_snapshot!(
        render(r#"separate(" ", label("error", ""), label("warning", "a"), "b")"#),
        @"[38;5;3ma[39m b");

    // List template
    insta::assert_snapshot!(render(r#"separate(" ", "a", ("" ""))"#), @"a");
    insta::assert_snapshot!(render(r#"separate(" ", "a", ("" "b"))"#), @"a b");

    // Nested separate
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", separate("|", "", ""))"#), @"a");
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", separate("|", "b", ""))"#), @"a b");
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", separate("|", "b", "c"))"#), @"a b|c");

    // Conditional template
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", if("t", ""))"#), @"a");
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", if("t", "", "f"))"#), @"a");
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", if("", "t", ""))"#), @"a");
    insta::assert_snapshot!(
        render(r#"separate(" ", "a", if("t", "t", "f"))"#), @"a t");

    // Separate keywords
    insta::assert_snapshot!(
        render(r#"separate(" ", author, description, empty)"#), @" <> [38;5;2mtrue[39m");

    // Keyword as separator
    insta::assert_snapshot!(
        render(r#"separate(author, "X", "Y", "Z")"#), @"X <>Y <>Z");
}

#[test]
fn test_templater_upper_lower() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    insta::assert_snapshot!(
      render(r#"change_id.shortest(4).upper() change_id.shortest(4).upper().lower()"#),
      @"[1m[38;5;5mZ[0m[38;5;8mZZZ[39m[1m[38;5;5mz[0m[38;5;8mzzz[39m");
    insta::assert_snapshot!(
      render(r#""Hello".upper() "Hello".lower()"#), @"HELLOhello");
}

#[test]
fn test_templater_alias() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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

    insta::assert_snapshot!(render_err("commit_id syntax_error"), @r###"
    Error: Failed to parse template:  --> 1:11
      |
    1 | commit_id syntax_error
      |           ^----------^
      |
      = Alias "syntax_error" cannot be expanded
     --> 1:5
      |
    1 | foo.
      |     ^---
      |
      = expected identifier
    "###);

    insta::assert_snapshot!(render_err("commit_id name_error"), @r###"
    Error: Failed to parse template:  --> 1:11
      |
    1 | commit_id name_error
      |           ^--------^
      |
      = Alias "name_error" cannot be expanded
     --> 1:1
      |
    1 | unknown_id
      | ^--------^
      |
      = Keyword "unknown_id" doesn't exist
    "###);

    insta::assert_snapshot!(render_err(r#"identity(identity(commit_id.short("")))"#), @r###"
    Error: Failed to parse template:  --> 1:1
      |
    1 | identity(identity(commit_id.short("")))
      | ^-------------------------------------^
      |
      = Alias "identity()" cannot be expanded
     --> 1:10
      |
    1 | identity(identity(commit_id.short("")))
      |          ^---------------------------^
      |
      = Alias "identity()" cannot be expanded
     --> 1:35
      |
    1 | identity(identity(commit_id.short("")))
      |                                   ^^
      |
      = Expected argument of type "Integer"
    "###);

    insta::assert_snapshot!(render_err("commit_id recurse"), @r###"
    Error: Failed to parse template:  --> 1:11
      |
    1 | commit_id recurse
      |           ^-----^
      |
      = Alias "recurse" cannot be expanded
     --> 1:1
      |
    1 | recurse1
      | ^------^
      |
      = Alias "recurse1" cannot be expanded
     --> 1:1
      |
    1 | recurse2()
      | ^--------^
      |
      = Alias "recurse2()" cannot be expanded
     --> 1:1
      |
    1 | recurse
      | ^-----^
      |
      = Alias "recurse" expanded recursively
    "###);

    insta::assert_snapshot!(render_err("identity()"), @r###"
    Error: Failed to parse template:  --> 1:10
      |
    1 | identity()
      |          ^
      |
      = Expected 1 arguments
    "###);
    insta::assert_snapshot!(render_err("identity(commit_id, commit_id)"), @r###"
    Error: Failed to parse template:  --> 1:10
      |
    1 | identity(commit_id, commit_id)
      |          ^------------------^
      |
      = Expected 1 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"coalesce(label("x", "not boolean"), "")"#), @r###"
    Error: Failed to parse template:  --> 1:1
      |
    1 | coalesce(label("x", "not boolean"), "")
      | ^-------------------------------------^
      |
      = Alias "coalesce()" cannot be expanded
     --> 1:10
      |
    1 | coalesce(label("x", "not boolean"), "")
      |          ^-----------------------^
      |
      = Expected argument of type "Boolean"
    "###);
}

#[test]
fn test_templater_bad_alias_decl() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r###"
    [template-aliases]
    'badfn(a, a)' = 'a'
    'my_commit_id' = 'commit_id.short()'
    "###,
    );

    // Invalid declaration should be warned and ignored.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "--no-graph", "-r@-", "-Tmy_commit_id"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"000000000000");
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Failed to load "template-aliases.badfn(a, a)":  --> 1:7
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
