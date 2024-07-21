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

use crate::common::TestEnvironment;

#[test]
fn test_syntax_error() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", ":x"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: ':' is not a prefix operator
    Caused by:  --> 1:1
      |
    1 | :x
      | ^
      |
      = ':' is not a prefix operator
    Hint: Did you mean '::' for ancestors?
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "x &"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Syntax error
    Caused by:  --> 1:4
      |
    1 | x &
      |    ^---
      |
      = expected `::`, `..`, `~`, or <primary>
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "x - y"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: '-' is not an infix operator
    Caused by:  --> 1:3
      |
    1 | x - y
      |   ^
      |
      = '-' is not an infix operator
    Hint: Did you mean '~' for difference?
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "HEAD^"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: '^' is not a postfix operator
    Caused by:  --> 1:5
      |
    1 | HEAD^
      |     ^
      |
      = '^' is not a postfix operator
    Hint: Did you mean '-' for parents?
    "###);
}

#[test]
fn test_bad_function_call() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "all(or::nothing)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "all": Expected 0 arguments
    Caused by:  --> 1:5
      |
    1 | all(or::nothing)
      |     ^---------^
      |
      = Function "all": Expected 0 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "parents()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "parents": Expected 1 arguments
    Caused by:  --> 1:9
      |
    1 | parents()
      |         ^
      |
      = Function "parents": Expected 1 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "parents(foo, bar)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "parents": Expected 1 arguments
    Caused by:  --> 1:9
      |
    1 | parents(foo, bar)
      |         ^------^
      |
      = Function "parents": Expected 1 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "heads(foo, bar)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "heads": Expected 1 arguments
    Caused by:  --> 1:7
      |
    1 | heads(foo, bar)
      |       ^------^
      |
      = Function "heads": Expected 1 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "latest(a, not_an_integer)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Expected expression of type integer
    Caused by:  --> 1:11
      |
    1 | latest(a, not_an_integer)
      |           ^------------^
      |
      = Expected expression of type integer
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "file()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "file": Expected at least 1 arguments
    Caused by:  --> 1:6
      |
    1 | file()
      |      ^
      |
      = Function "file": Expected at least 1 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "file(not::a-fileset)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Invalid fileset expression
    Caused by:
    1:  --> 1:6
      |
    1 | file(not::a-fileset)
      |      ^------------^
      |
      = Invalid fileset expression
    2:  --> 1:5
      |
    1 | not::a-fileset
      |     ^---
      |
      = expected <identifier>, <string_literal>, or <raw_string_literal>
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", r#"file(foo:"bar")"#]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Invalid fileset expression
    Caused by:
    1:  --> 1:6
      |
    1 | file(foo:"bar")
      |      ^-------^
      |
      = Invalid fileset expression
    2:  --> 1:1
      |
    1 | foo:"bar"
      | ^-------^
      |
      = Invalid file pattern
    3: Invalid file pattern kind "foo:"
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", r#"file(a, "../out")"#]);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Error: Failed to parse revset: Invalid fileset expression
    Caused by:
    1:  --> 1:9
      |
    1 | file(a, "../out")
      |         ^------^
      |
      = Invalid fileset expression
    2:  --> 1:1
      |
    1 | "../out"
      | ^------^
      |
      = Invalid file pattern
    3: Path "../out" is not in the repo "."
    4: Invalid component ".." in repo-relative path "../out"
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "branches(bad:pattern)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Invalid string pattern
    Caused by:
    1:  --> 1:10
      |
    1 | branches(bad:pattern)
      |          ^---------^
      |
      = Invalid string pattern
    2: Invalid string pattern kind "bad:"
    Hint: Try prefixing with one of `exact:`, `glob:`, `regex:`, or `substring:`
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "branches(regex:'(')"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Invalid string pattern
    Caused by:
    1:  --> 1:10
      |
    1 | branches(regex:'(')
      |          ^-------^
      |
      = Invalid string pattern
    2: regex parse error:
        (
        ^
    error: unclosed group
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "root()::whatever()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "whatever" doesn't exist
    Caused by:  --> 1:9
      |
    1 | root()::whatever()
      |         ^------^
      |
      = Function "whatever" doesn't exist
    "###);

    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["log", "-r", "remote_branches(a, b, remote=c)"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "remote_branches": Got multiple values for keyword "remote"
    Caused by:  --> 1:23
      |
    1 | remote_branches(a, b, remote=c)
      |                       ^------^
      |
      = Function "remote_branches": Got multiple values for keyword "remote"
    "###);

    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["log", "-r", "remote_branches(remote=a, b)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "remote_branches": Positional argument follows keyword argument
    Caused by:  --> 1:27
      |
    1 | remote_branches(remote=a, b)
      |                           ^
      |
      = Function "remote_branches": Positional argument follows keyword argument
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "remote_branches(=foo)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Syntax error
    Caused by:  --> 1:17
      |
    1 | remote_branches(=foo)
      |                 ^---
      |
      = expected <identifier> or <expression>
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "remote_branches(remote=)"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Syntax error
    Caused by:  --> 1:24
      |
    1 | remote_branches(remote=)
      |                        ^---
      |
      = expected <expression>
    "###);
}

#[test]
fn test_function_name_hint() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let evaluate_err = |expr| test_env.jj_cmd_failure(&repo_path, &["log", "-r", expr]);

    test_env.add_config(
        r###"
    [revset-aliases]
    'branches(x)' = 'x' # override builtin function
    'my_author(x)' = 'author(x)' # similar name to builtin function
    'author_sym' = 'x' # not a function alias
    'my_branches' = 'branch()' # typo in alias
    "###,
    );

    // The suggestion "branches" shouldn't be duplicated
    insta::assert_snapshot!(evaluate_err("branch()"), @r###"
    Error: Failed to parse revset: Function "branch" doesn't exist
    Caused by:  --> 1:1
      |
    1 | branch()
      | ^----^
      |
      = Function "branch" doesn't exist
    Hint: Did you mean "branches", "reachable"?
    "###);

    // Both builtin function and function alias should be suggested
    insta::assert_snapshot!(evaluate_err("author_()"), @r###"
    Error: Failed to parse revset: Function "author_" doesn't exist
    Caused by:  --> 1:1
      |
    1 | author_()
      | ^-----^
      |
      = Function "author_" doesn't exist
    Hint: Did you mean "author", "my_author"?
    "###);

    insta::assert_snapshot!(evaluate_err("my_branches"), @r###"
    Error: Failed to parse revset: Alias "my_branches" cannot be expanded
    Caused by:
    1:  --> 1:1
      |
    1 | my_branches
      | ^---------^
      |
      = Alias "my_branches" cannot be expanded
    2:  --> 1:1
      |
    1 | branch()
      | ^----^
      |
      = Function "branch" doesn't exist
    Hint: Did you mean "branches", "reachable"?
    "###);
}

#[test]
fn test_alias() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r###"
    [revset-aliases]
    'my-root' = 'root()'
    'syntax-error' = 'whatever &'
    'recurse' = 'recurse1'
    'recurse1' = 'recurse2()'
    'recurse2()' = 'recurse'
    'identity(x)' = 'x'
    'my_author(x)' = 'author(x)'
    "###,
    );

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "my-root"]);
    insta::assert_snapshot!(stdout, @r###"
    ◆  zzzzzzzz root() 00000000
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r", "identity(my-root)"]);
    insta::assert_snapshot!(stdout, @r###"
    ◆  zzzzzzzz root() 00000000
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "root() & syntax-error"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Alias "syntax-error" cannot be expanded
    Caused by:
    1:  --> 1:10
      |
    1 | root() & syntax-error
      |          ^----------^
      |
      = Alias "syntax-error" cannot be expanded
    2:  --> 1:11
      |
    1 | whatever &
      |           ^---
      |
      = expected `::`, `..`, `~`, or <primary>
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "identity()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Function "identity": Expected 1 arguments
    Caused by:  --> 1:10
      |
    1 | identity()
      |          ^
      |
      = Function "identity": Expected 1 arguments
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "my_author(none())"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Alias "my_author(x)" cannot be expanded
    Caused by:
    1:  --> 1:1
      |
    1 | my_author(none())
      | ^---------------^
      |
      = Alias "my_author(x)" cannot be expanded
    2:  --> 1:8
      |
    1 | author(x)
      |        ^
      |
      = Function parameter "x" cannot be expanded
    3:  --> 1:11
      |
    1 | my_author(none())
      |           ^----^
      |
      = Expected expression of string pattern
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r", "root() & recurse"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Alias "recurse" cannot be expanded
    Caused by:
    1:  --> 1:10
      |
    1 | root() & recurse
      |          ^-----^
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
}

#[test]
fn test_alias_override() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r###"
    [revset-aliases]
    'f(x)' = 'user'
    "###,
    );

    // 'f(x)' should be overridden by --config-toml 'f(a)'. If aliases were sorted
    // purely by name, 'f(a)' would come first.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &[
            "log",
            "-r",
            "f(_)",
            "--config-toml",
            "revset-aliases.'f(a)' = 'arg'",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "arg" doesn't exist
    "###);
}

#[test]
fn test_bad_alias_decl() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r#"
    [revset-aliases]
    'my-root' = 'root()'
    '"bad"' = 'root()'
    'badfn(a, a)' = 'root()'
    "#,
    );

    // Invalid declaration should be warned and ignored.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-r", "my-root"]);
    insta::assert_snapshot!(stdout, @r###"
    ◆  zzzzzzzz root() 00000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Failed to load "revset-aliases."bad"":  --> 1:1
      |
    1 | "bad"
      | ^---
      |
      = expected <identifier> or <function_name>
    Warning: Failed to load "revset-aliases.badfn(a, a)":  --> 1:7
      |
    1 | badfn(a, a)
      |       ^--^
      |
      = Redefinition of function parameter
    "###);
}

#[test]
fn test_all_modifier() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // Command that accepts single revision by default
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "all()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "all()" resolved to more than one revision
    Hint: The revset "all()" resolved to these revisions:
      qpvuntsm 230dd059 (empty) (no description set)
      zzzzzzzz 00000000 (empty) (no description set)
    Hint: Prefix the expression with 'all:' to allow any number of revisions (i.e. 'all:all()').
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "all:all()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: The Git backend does not support creating merge commits with the root commit as one of the parents.
    "###);

    // Command that accepts multiple revisions by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-rall:all()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 08:05:07 230dd059
    │  (empty) (no description set)
    ◆  zzzzzzzz root() 00000000
    "###);

    // Command that accepts only single revision
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["branch", "create", "-rall:@", "x"]);
    insta::assert_snapshot!(stderr, @r###"
    Created 1 branches pointing to qpvuntsm 230dd059 x | (empty) (no description set)
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["branch", "set", "-rall:all()", "x"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revset "all:all()" resolved to more than one revision
    Hint: The revset "all:all()" resolved to these revisions:
      qpvuntsm 230dd059 x | (empty) (no description set)
      zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Template expression that accepts multiple revisions by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-Tself.contained_in('all:all()')"]);
    insta::assert_snapshot!(stdout, @r###"
    @  true
    ◆  true
    "###);

    // Typo
    let stderr = test_env.jj_cmd_failure(&repo_path, &["new", "ale:x"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Modifier "ale" doesn't exist
    Caused by:  --> 1:1
      |
    1 | ale:x
      | ^-^
      |
      = Modifier "ale" doesn't exist
    "###);

    // Modifier shouldn't be allowed in sub expression
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["new", "x..", "--config-toml=revset-aliases.x='all:@'"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse revset: Alias "x" cannot be expanded
    Caused by:
    1:  --> 1:1
      |
    1 | x..
      | ^
      |
      = Alias "x" cannot be expanded
    2:  --> 1:1
      |
    1 | all:@
      | ^-^
      |
      = Modifier "all:" is not allowed in sub expression
    "###);

    // immutable_heads() alias may be parsed as a top-level expression, but
    // still, modifier shouldn't be allowed there.
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &[
            "new",
            "--config-toml=revset-aliases.'immutable_heads()'='all:@'",
            "--config-toml=revsets.short-prefixes='none()'",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    Config error: Invalid `revset-aliases.immutable_heads()`
    Caused by:  --> 1:1
      |
    1 | all:@
      | ^-^
      |
      = Modifier "all:" is not allowed in sub expression
    For help, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
    "###);
}
