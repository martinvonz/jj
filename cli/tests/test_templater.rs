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

    let template = r#"commit_id.short() ++ " " ++ branches"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ◉  b1bb3766d584 branch3??
    │ ◉  21c33875443e branch1*
    ├─╯
    │ @  a5b4d15489cc branch2* new-branch
    │ ◉  8476341eb395 branch2@origin
    ├─╯
    ◉  000000000000
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
    insta::assert_snapshot!(render(r#"  commit_id.short()  ++ empty "#), @"000000000000true");

    // Parenthesized single term
    insta::assert_snapshot!(render(r#"(commit_id.short())"#), @"000000000000");

    // Parenthesized multiple terms and concatenation
    insta::assert_snapshot!(render(r#"(commit_id.short() ++ " ") ++ empty"#), @"000000000000 true");

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
    Error: Failed to parse template:  --> 1:13
      |
    1 | description ()
      |             ^---
      |
      = expected EOI
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
    insta::assert_snapshot!(render_err(r#"(-empty)"#), @r###"
    Error: Failed to parse template:  --> 1:2
      |
    1 | (-empty)
      |  ^---
      |
      = expected template
    "###);

    insta::assert_snapshot!(render_err(r#"("foo" ++ "bar").baz()"#), @r###"
    Error: Failed to parse template:  --> 1:18
      |
    1 | ("foo" ++ "bar").baz()
      |                  ^-^
      |
      = Method "baz" doesn't exist for type "Template"
    "###);

    insta::assert_snapshot!(render_err(r#"description.contains()"#), @r###"
    Error: Failed to parse template:  --> 1:22
      |
    1 | description.contains()
      |                      ^
      |
      = Function "contains": Expected 1 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"description.first_line("foo")"#), @r###"
    Error: Failed to parse template:  --> 1:24
      |
    1 | description.first_line("foo")
      |                        ^---^
      |
      = Function "first_line": Expected 0 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"label()"#), @r###"
    Error: Failed to parse template:  --> 1:7
      |
    1 | label()
      |       ^
      |
      = Function "label": Expected 2 arguments
    "###);
    insta::assert_snapshot!(render_err(r#"label("foo", "bar", "baz")"#), @r###"
    Error: Failed to parse template:  --> 1:7
      |
    1 | label("foo", "bar", "baz")
      |       ^-----------------^
      |
      = Function "label": Expected 2 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"if()"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | if()
      |    ^
      |
      = Function "if": Expected 2 to 3 arguments
    "###);
    insta::assert_snapshot!(render_err(r#"if("foo", "bar", "baz", "quux")"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | if("foo", "bar", "baz", "quux")
      |    ^-------------------------^
      |
      = Function "if": Expected 2 to 3 arguments
    "###);

    insta::assert_snapshot!(render_err(r#"if(label("foo", "bar"), "baz")"#), @r###"
    Error: Failed to parse template:  --> 1:4
      |
    1 | if(label("foo", "bar"), "baz")
      |    ^-----------------^
      |
      = Expected expression of type "Boolean"
    "###);

    insta::assert_snapshot!(render_err(r#"|x| description"#), @r###"
    Error: Failed to parse template:  --> 1:1
      |
    1 | |x| description
      | ^-------------^
      |
      = Lambda cannot be defined here
    "###);
}

#[test]
fn test_templater_list_method() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_template_output(&test_env, &repo_path, "@-", template);
    let render_err = |template| test_env.jj_cmd_failure(&repo_path, &["log", "-T", template]);

    test_env.add_config(
        r###"
    [template-aliases]
    'identity' = '|x| x'
    'too_many_params' = '|x, y| x'
    "###,
    );

    insta::assert_snapshot!(render(r#""".lines().join("|")"#), @"");
    insta::assert_snapshot!(render(r#""a\nb\nc".lines().join("|")"#), @"a|b|c");
    // Keyword as separator
    insta::assert_snapshot!(render(r#""a\nb\nc".lines().join(commit_id.short(2))"#), @"a00b00c");

    insta::assert_snapshot!(render(r#""a\nb\nc".lines().map(|s| s ++ s)"#), @"aa bb cc");
    // Global keyword in item template
    insta::assert_snapshot!(
        render(r#""a\nb\nc".lines().map(|s| s ++ empty)"#), @"atrue btrue ctrue");
    // Override global keyword 'empty'
    insta::assert_snapshot!(
        render(r#""a\nb\nc".lines().map(|empty| empty)"#), @"a b c");
    // Nested map operations
    insta::assert_snapshot!(
        render(r#""a\nb\nc".lines().map(|s| "x\ny".lines().map(|t| s ++ t))"#),
        @"ax ay bx by cx cy");
    // Nested map/join operations
    insta::assert_snapshot!(
        render(r#""a\nb\nc".lines().map(|s| "x\ny".lines().map(|t| s ++ t).join(",")).join(";")"#),
        @"ax,ay;bx,by;cx,cy");
    // Nested string operations
    insta::assert_snapshot!(render(r#""!a\n!b\nc\nend".remove_suffix("end").lines().map(|s| s.remove_prefix("!"))"#), @"a b c");

    // Lambda expression in alias
    insta::assert_snapshot!(render(r#""a\nb\nc".lines().map(identity)"#), @"a b c");

    // Not a lambda expression
    insta::assert_snapshot!(render_err(r#""a".lines().map(empty)"#), @r###"
    Error: Failed to parse template:  --> 1:17
      |
    1 | "a".lines().map(empty)
      |                 ^---^
      |
      = Expected lambda expression
    "###);
    // Bad lambda parameter count
    insta::assert_snapshot!(render_err(r#""a".lines().map(|| "")"#), @r###"
    Error: Failed to parse template:  --> 1:18
      |
    1 | "a".lines().map(|| "")
      |                  ^
      |
      = Expected 1 lambda parameters
    "###);
    insta::assert_snapshot!(render_err(r#""a".lines().map(|a, b| "")"#), @r###"
    Error: Failed to parse template:  --> 1:18
      |
    1 | "a".lines().map(|a, b| "")
      |                  ^--^
      |
      = Expected 1 lambda parameters
    "###);
    // Error in lambda expression
    insta::assert_snapshot!(render_err(r#""a".lines().map(|s| s.unknown())"#), @r###"
    Error: Failed to parse template:  --> 1:23
      |
    1 | "a".lines().map(|s| s.unknown())
      |                       ^-----^
      |
      = Method "unknown" doesn't exist for type "String"
    "###);
    // Error in lambda alias
    insta::assert_snapshot!(render_err(r#""a".lines().map(too_many_params)"#), @r###"
    Error: Failed to parse template:  --> 1:17
      |
    1 | "a".lines().map(too_many_params)
      |                 ^-------------^
      |
      = Alias "too_many_params" cannot be expanded
     --> 1:2
      |
    1 | |x, y| x
      |  ^--^
      |
      = Expected 1 lambda parameters
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

    insta::assert_snapshot!(render(r#""".lines()"#), @"");
    insta::assert_snapshot!(render(r#""a\nb\nc\n".lines()"#), @"a b c");

    insta::assert_snapshot!(render(r#""".starts_with("")"#), @"true");
    insta::assert_snapshot!(render(r#""everything".starts_with("")"#), @"true");
    insta::assert_snapshot!(render(r#""".starts_with("foo")"#), @"false");
    insta::assert_snapshot!(render(r#""foo".starts_with("foo")"#), @"true");
    insta::assert_snapshot!(render(r#""foobar".starts_with("foo")"#), @"true");
    insta::assert_snapshot!(render(r#""foobar".starts_with("bar")"#), @"false");

    insta::assert_snapshot!(render(r#""".ends_with("")"#), @"true");
    insta::assert_snapshot!(render(r#""everything".ends_with("")"#), @"true");
    insta::assert_snapshot!(render(r#""".ends_with("foo")"#), @"false");
    insta::assert_snapshot!(render(r#""foo".ends_with("foo")"#), @"true");
    insta::assert_snapshot!(render(r#""foobar".ends_with("foo")"#), @"false");
    insta::assert_snapshot!(render(r#""foobar".ends_with("bar")"#), @"true");

    insta::assert_snapshot!(render(r#""".remove_prefix("wip: ")"#), @"");
    insta::assert_snapshot!(render(r#""wip: testing".remove_prefix("wip: ")"#), @"testing");

    insta::assert_snapshot!(render(r#""bar@my.example.com".remove_suffix("@other.example.com")"#), @"bar@my.example.com");
    insta::assert_snapshot!(render(r#""bar@other.example.com".remove_suffix("@other.example.com")"#), @"bar");

    insta::assert_snapshot!(render(r#""foo".substr(0, 0)"#), @"");
    insta::assert_snapshot!(render(r#""foo".substr(0, 1)"#), @"f");
    insta::assert_snapshot!(render(r#""foo".substr(0, 99)"#), @"foo");
    insta::assert_snapshot!(render(r#""abcdef".substr(2, -1)"#), @"cde");
    insta::assert_snapshot!(render(r#""abcdef".substr(-3, 99)"#), @"def");
    insta::assert_snapshot!(render(r#""abc💩".substr(2, -1)"#), @"c");
    insta::assert_snapshot!(render(r#""abc💩".substr(-1, 99)"#), @"💩");

    // ranges with end > start are empty
    insta::assert_snapshot!(render(r#""abcdef".substr(4, 2)"#), @"");
    insta::assert_snapshot!(render(r#""abcdef".substr(-2, -4)"#), @"");
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

    test_env.jj_cmd_ok(&repo_path, &["--config-toml=user.name=''", "new"]);

    insta::assert_snapshot!(render(r#"author"#), @"<test.user@example.com>");
    insta::assert_snapshot!(render(r#"author.name()"#), @"");
    insta::assert_snapshot!(render(r#"author.email()"#), @"test.user@example.com");
    insta::assert_snapshot!(render(r#"author.username()"#), @"test.user");

    test_env.jj_cmd_ok(&repo_path, &["--config-toml=user.email=''", "new"]);

    insta::assert_snapshot!(render(r#"author"#), @"Test User");
    insta::assert_snapshot!(render(r#"author.name()"#), @"Test User");
    insta::assert_snapshot!(render(r#"author.email()"#), @"");
    insta::assert_snapshot!(render(r#"author.username()"#), @"");

    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--config-toml=user.email=''",
            "--config-toml=user.name=''",
            "new",
        ],
    );

    insta::assert_snapshot!(render(r#"author"#), @"");
    insta::assert_snapshot!(render(r#"author.name()"#), @"");
    insta::assert_snapshot!(render(r#"author.email()"#), @"");
    insta::assert_snapshot!(render(r#"author.username()"#), @"");
}

#[test]
fn test_templater_timestamp_method() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_template_output(&test_env, &repo_path, "@-", template);
    let render_err = |template| test_env.jj_cmd_failure(&repo_path, &["log", "-T", template]);

    test_env.add_config(
        r#"
    [template-aliases]
    'time_format' = '"%Y-%m-%d"'
    'bad_time_format' = '"%_"'
    "#,
    );

    insta::assert_snapshot!(
        render(r#"author.timestamp().format("%Y%m%d %H:%M:%S")"#), @"19700101 00:00:00");

    // Invalid format string
    insta::assert_snapshot!(render_err(r#"author.timestamp().format("%_")"#), @r###"
    Error: Failed to parse template:  --> 1:27
      |
    1 | author.timestamp().format("%_")
      |                           ^--^
      |
      = Invalid time format
    "###);

    // Invalid type
    insta::assert_snapshot!(render_err(r#"author.timestamp().format(0)"#), @r###"
    Error: Failed to parse template:  --> 1:27
      |
    1 | author.timestamp().format(0)
      |                           ^
      |
      = Expected string literal
    "###);

    // Dynamic string isn't supported yet
    insta::assert_snapshot!(render_err(r#"author.timestamp().format("%Y" ++ "%m")"#), @r###"
    Error: Failed to parse template:  --> 1:27
      |
    1 | author.timestamp().format("%Y" ++ "%m")
      |                           ^----------^
      |
      = Expected string literal
    "###);

    // Literal alias expansion
    insta::assert_snapshot!(render(r#"author.timestamp().format(time_format)"#), @"1970-01-01");
    insta::assert_snapshot!(render_err(r#"author.timestamp().format(bad_time_format)"#), @r###"
    Error: Failed to parse template:  --> 1:27
      |
    1 | author.timestamp().format(bad_time_format)
      |                           ^-------------^
      |
      = Alias "bad_time_format" cannot be expanded
     --> 1:1
      |
    1 | "%_"
      | ^--^
      |
      = Invalid time format
    "###);
}

#[test]
fn test_templater_fill_function() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    insta::assert_snapshot!(
        render(r#"fill(20, "The quick fox jumps over the " ++
                       label("error", "lazy") ++ " dog\n")"#),
        @r###"
    The quick fox jumps
    over the [38;5;1mlazy[39m dog
    "###);

    // A low value will not chop words, but can chop a label by words
    insta::assert_snapshot!(
        render(r#"fill(9, "Longlonglongword an some short words " ++
                       label("error", "longlonglongword and short words") ++ " back out\n")"#),
        @r###"
    Longlonglongword
    an some
    short
    words
    [38;5;1mlonglonglongword[39m
    [38;5;1mand short[39m
    [38;5;1mwords[39m
    back out
    "###);

    // Filling to 0 means breaking at every word
    insta::assert_snapshot!(
        render(r#"fill(0, "The quick fox jumps over the " ++
                       label("error", "lazy") ++ " dog\n")"#),
        @r###"
    The
    quick
    fox
    jumps
    over
    the
    [38;5;1mlazy[39m
    dog
    "###);

    // Filling to -0 is the same as 0
    insta::assert_snapshot!(
        render(r#"fill(-0, "The quick fox jumps over the " ++
                       label("error", "lazy") ++ " dog\n")"#),
        @r###"
    The
    quick
    fox
    jumps
    over
    the
    [38;5;1mlazy[39m
    dog
    "###);

    // Filling to negatives are clamped to the same as zero
    insta::assert_snapshot!(
        render(r#"fill(-10, "The quick fox jumps over the " ++
                       label("error", "lazy") ++ " dog\n")"#),
        @r###"
    The
    quick
    fox
    jumps
    over
    the
    [38;5;1mlazy[39m
    dog
    "###);

    // Word-wrap, then indent
    insta::assert_snapshot!(
        render(r#""START marker to help insta\n" ++
                  indent("    ", fill(20, "The quick fox jumps over the " ++
                                      label("error", "lazy") ++ " dog\n"))"#),
        @r###"
    START marker to help insta
        The quick fox jumps
        over the [38;5;1mlazy[39m dog
    "###);

    // Word-wrap indented (no special handling for leading spaces)
    insta::assert_snapshot!(
        render(r#""START marker to help insta\n" ++
                  fill(20, indent("    ", "The quick fox jumps over the " ++
                                  label("error", "lazy") ++ " dog\n"))"#),
        @r###"
    START marker to help insta
        The quick fox
    jumps over the [38;5;1mlazy[39m
    dog
    "###);
}

#[test]
fn test_templater_indent_function() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    // Empty line shouldn't be indented. Not using insta here because we test
    // whitespace existence.
    assert_eq!(render(r#"indent("__", "")"#), "");
    assert_eq!(render(r#"indent("__", "\n")"#), "\n");
    assert_eq!(render(r#"indent("__", "a\n\nb")"#), "__a\n\n__b");

    // "\n" at end of labeled text
    insta::assert_snapshot!(
        render(r#"indent("__", label("error", "a\n") ++ label("warning", "b\n"))"#),
        @r###"
    [38;5;1m__a[39m
    [38;5;3m__b[39m
    "###);

    // "\n" in labeled text
    insta::assert_snapshot!(
        render(r#"indent("__", label("error", "a") ++ label("warning", "b\nc"))"#),
        @r###"
    [38;5;1m__a[39m[38;5;3mb[39m
    [38;5;3m__c[39m
    "###);

    // Labeled prefix + unlabeled content
    insta::assert_snapshot!(
        render(r#"indent(label("error", "XX"), "a\nb\n")"#),
        @r###"
    [38;5;1mXX[39ma
    [38;5;1mXX[39mb
    "###);

    // Nested indent, silly but works
    insta::assert_snapshot!(
        render(r#"indent(label("hint", "A"),
                         label("warning", indent(label("hint", "B"),
                                                 label("error", "x\n") ++ "y")))"#),
        @r###"
    [38;5;6mAB[38;5;1mx[39m
    [38;5;6mAB[38;5;3my[39m
    "###);
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
fn test_templater_concat_function() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| get_colored_template_output(&test_env, &repo_path, "@-", template);

    insta::assert_snapshot!(render(r#"concat()"#), @"");
    insta::assert_snapshot!(render(r#"concat(hidden, empty)"#), @"false[38;5;2mtrue[39m");
    insta::assert_snapshot!(
        render(r#"concat(label("error", ""), label("warning", "a"), "b")"#),
        @"[38;5;3ma[39mb");
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
    insta::assert_snapshot!(render(r#"separate(" ", "a", ("" ++ ""))"#), @"a");
    insta::assert_snapshot!(render(r#"separate(" ", "a", ("" ++ "b"))"#), @"a b");

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
        render(r#"separate(" ", hidden, description, empty)"#), @"false [38;5;2mtrue[39m");

    // Keyword as separator
    insta::assert_snapshot!(
        render(r#"separate(hidden, "X", "Y", "Z")"#), @"XfalseYfalseZ");
}

#[test]
fn test_templater_upper_lower() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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

    insta::assert_snapshot!(render_err("commit_id ++ syntax_error"), @r###"
    Error: Failed to parse template:  --> 1:14
      |
    1 | commit_id ++ syntax_error
      |              ^----------^
      |
      = Alias "syntax_error" cannot be expanded
     --> 1:5
      |
    1 | foo.
      |     ^---
      |
      = expected identifier
    "###);

    insta::assert_snapshot!(render_err("commit_id ++ name_error"), @r###"
    Error: Failed to parse template:  --> 1:14
      |
    1 | commit_id ++ name_error
      |              ^--------^
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
      = Expected expression of type "Integer"
    "###);

    insta::assert_snapshot!(render_err("commit_id ++ recurse"), @r###"
    Error: Failed to parse template:  --> 1:14
      |
    1 | commit_id ++ recurse
      |              ^-----^
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
      = Function "identity": Expected 1 arguments
    "###);
    insta::assert_snapshot!(render_err("identity(commit_id, commit_id)"), @r###"
    Error: Failed to parse template:  --> 1:10
      |
    1 | identity(commit_id, commit_id)
      |          ^------------------^
      |
      = Function "identity": Expected 1 arguments
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
      = Expected expression of type "Boolean"
    "###);
}

#[test]
fn test_templater_alias_override() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["log", "--no-graph", "-r@-", "-Tmy_commit_id"]);
    insta::assert_snapshot!(stdout, @"000000000000");
    insta::assert_snapshot!(stderr, @r###"
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
