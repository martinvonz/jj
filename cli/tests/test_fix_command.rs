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

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use itertools::Itertools;
use jj_lib::file_util::try_symlink;

use crate::common::TestEnvironment;

/// Set up a repo where the `jj fix` command uses the fake editor with the given
/// flags. Returns a function that redacts the formatter executable's path from
/// a given string for test determinism.
fn init_with_fake_formatter(args: &[&str]) -> (TestEnvironment, PathBuf, impl Fn(&str) -> String) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    // The deprecated configuration syntax is still used by tests where it doesn't
    // make a meaningful difference in coverage. Otherwise, we would have to add
    // dedicated test coverage for the deprecated syntax until it is removed. We use
    // single quotes here to avoid escaping issues when running the test on Windows.
    test_env.add_config(&format!(
        r#"fix.tool-command = ['{}']"#,
        [formatter_path.to_str().unwrap()]
            .iter()
            .chain(args)
            .join(r#"', '"#)
    ));
    (test_env, repo_path, move |snapshot: &str| {
        // When the test runs on Windows, backslashes in the path complicate things by
        // changing the double quotes to single quotes in the serialized TOML.
        snapshot.replace(
            &if cfg!(windows) {
                format!(r#"'{}'"#, formatter_path.to_str().unwrap())
            } else {
                format!(r#""{}""#, formatter_path.to_str().unwrap())
            },
            "<redacted formatter path>",
        )
    })
}

#[test]
fn test_config_no_tools() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "content\n").unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: At least one entry of `fix.tools` or `fix.tool-command` is required.
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content\n");
}

#[test]
fn test_config_both_legacy_and_table_tools() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix]
        tool-command = ["{formatter}", "--append", "legacy change"]

        [fix.tools.tool-1]
        command = ["{formatter}", "--append", "tables change"]
        patterns = ["tables-file"]
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("legacy-file"), "legacy content\n").unwrap();
    std::fs::write(repo_path.join("tables-file"), "tables content\n").unwrap();

    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "legacy-file", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    legacy content
    legacy change
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "tables-file", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    tables content
    legacy change
    tables change
    "###);
}

#[test]
fn test_config_multiple_tools() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix.tools.tool-1]
        command = ["{formatter}", "--uppercase"]
        patterns = ["foo"]

        [fix.tools.tool-2]
        command = ["{formatter}", "--lowercase"]
        patterns = ["bar"]
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("foo"), "Foo\n").unwrap();
    std::fs::write(repo_path.join("bar"), "Bar\n").unwrap();
    std::fs::write(repo_path.join("baz"), "Baz\n").unwrap();

    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @"FOO\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "bar", "-r", "@"]);
    insta::assert_snapshot!(content, @"bar\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "baz", "-r", "@"]);
    insta::assert_snapshot!(content, @"Baz\n");
}

#[test]
fn test_config_multiple_tools_with_same_name() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");

    // Multiple definitions with the same `name` are not allowed, because it is
    // likely to be a mistake, and mistakes are risky when they rewrite files.
    test_env.add_config(&format!(
        r###"
        [fix.tools.my-tool]
        command = ["{formatter}", "--uppercase"]
        patterns = ["foo"]

        [fix.tools.my-tool]
        command = ["{formatter}", "--lowercase"]
        patterns = ["bar"]
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("foo"), "Foo\n").unwrap();
    std::fs::write(repo_path.join("bar"), "Bar\n").unwrap();

    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix"]);
    #[cfg(unix)]
    insta::assert_snapshot!(stderr, @r###"
    Config error: redefinition of table `fix.tools.my-tool` for key `fix.tools.my-tool` at line 6 column 9 in ../config/config0002.toml
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);
    #[cfg(windows)]
    insta::assert_snapshot!(stderr, @r###"
    Config error: redefinition of table `fix.tools.my-tool` for key `fix.tools.my-tool` at line 6 column 9 in ..\config\config0002.toml
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

    test_env.set_config_path("/dev/null".into());
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @"Foo\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "bar", "-r", "@"]);
    insta::assert_snapshot!(content, @"Bar\n");
}

#[test]
fn test_config_disabled_tools() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix.tools.tool-1]
        # default is enabled
        command = ["{formatter}", "--uppercase"]
        patterns = ["foo"]

        [fix.tools.tool-2]
        enabled = true
        command = ["{formatter}", "--lowercase"]
        patterns = ["bar"]

        [fix.tools.tool-3]
        enabled = false
        command = ["{formatter}", "--lowercase"]
        patterns = ["baz"]
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("foo"), "Foo\n").unwrap();
    std::fs::write(repo_path.join("bar"), "Bar\n").unwrap();
    std::fs::write(repo_path.join("baz"), "Baz\n").unwrap();

    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @"FOO\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "bar", "-r", "@"]);
    insta::assert_snapshot!(content, @"bar\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "baz", "-r", "@"]);
    insta::assert_snapshot!(content, @"Baz\n");
}

#[test]
fn test_config_tables_overlapping_patterns() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");

    test_env.add_config(&format!(
        r###"
        [fix.tools.tool-1]
        command = ["{formatter}", "--append", "tool-1"]
        patterns = ["foo", "bar"]

        [fix.tools.tool-2]
        command = ["{formatter}", "--append", "tool-2"]
        patterns = ["bar", "baz"]
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("foo"), "foo\n").unwrap();
    std::fs::write(repo_path.join("bar"), "bar\n").unwrap();
    std::fs::write(repo_path.join("baz"), "baz\n").unwrap();

    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    foo
    tool-1
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "bar", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    bar
    tool-1
    tool-2
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "baz", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    baz
    tool-2
    "###);
}

#[test]
fn test_config_tables_all_commands_missing() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(
        r###"
        [fix.tools.my-tool-missing-command-1]
        patterns = ["foo"]

        [fix.tools.my-tool-missing-command-2]
        patterns = ['glob:"ba*"']
        "###,
    );

    std::fs::write(repo_path.join("foo"), "foo\n").unwrap();

    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: missing field `command`
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @"foo\n");
}

#[test]
fn test_config_tables_some_commands_missing() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix.tools.tool-1]
        command = ["{formatter}", "--uppercase"]
        patterns = ["foo"]

        [fix.tools.my-tool-missing-command]
        patterns = ['bar']
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("foo"), "foo\n").unwrap();

    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix"]);
    insta::assert_snapshot!(stderr, @r###"
    Config error: missing field `command`
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @"foo\n");
}

#[test]
fn test_config_tables_empty_patterns_list() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix.tools.my-tool-empty-patterns]
        command = ["{formatter}", "--uppercase"]
        patterns = []
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("foo"), "foo\n").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
      Fixed 0 commits of 1 checked.
      Nothing changed.
      "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo", "-r", "@"]);
    insta::assert_snapshot!(content, @"foo\n");
}

#[test]
fn test_config_filesets() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix.tools.my-tool-match-one]
        command = ["{formatter}", "--uppercase"]
        patterns = ['glob:"a*"']

        [fix.tools.my-tool-match-two]
        command = ["{formatter}", "--reverse"]
        patterns = ['glob:"b*"']

        [fix.tools.my-tool-match-none]
        command = ["{formatter}", "--append", "SHOULD NOT APPEAR"]
        patterns = ['glob:"this-doesnt-match-anything-*"']
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::write(repo_path.join("a1"), "a1\n").unwrap();
    std::fs::write(repo_path.join("b1"), "b1\n").unwrap();
    std::fs::write(repo_path.join("b2"), "b2\n").unwrap();

    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "a1", "-r", "@"]);
    insta::assert_snapshot!(content, @"A1\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "b1", "-r", "@"]);
    insta::assert_snapshot!(content, @"1b\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "b2", "-r", "@"]);
    insta::assert_snapshot!(content, @"2b\n");
}

#[test]
fn test_relative_paths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");
    test_env.add_config(&format!(
        r###"
        [fix.tools.tool]
        command = ["{formatter}", "--stdout", "Fixed!"]
        patterns = ['glob:"foo*"']
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::create_dir(repo_path.join("dir")).unwrap();
    std::fs::write(repo_path.join("foo1"), "unfixed\n").unwrap();
    std::fs::write(repo_path.join("foo2"), "unfixed\n").unwrap();
    std::fs::write(repo_path.join("dir/foo3"), "unfixed\n").unwrap();

    // Positional arguments are cwd-relative, but the configured patterns are
    // repo-relative, so this command fixes the empty intersection of those
    // filesets.
    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path.join("dir"), &["fix", "foo3"]);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo1", "-r", "@"]);
    insta::assert_snapshot!(content, @"unfixed\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo2", "-r", "@"]);
    insta::assert_snapshot!(content, @"unfixed\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "dir/foo3", "-r", "@"]);
    insta::assert_snapshot!(content, @"unfixed\n");

    // Positional arguments can specify a subset of the configured fileset.
    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path.join("dir"), &["fix", "../foo1"]);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo1", "-r", "@"]);
    insta::assert_snapshot!(content, @"Fixed!\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo2", "-r", "@"]);
    insta::assert_snapshot!(content, @"unfixed\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "dir/foo3", "-r", "@"]);
    insta::assert_snapshot!(content, @"unfixed\n");

    // The current directory does not change the interpretation of the config, so
    // foo2 is fixed but not dir/foo3.
    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path.join("dir"), &["fix"]);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo1", "-r", "@"]);
    insta::assert_snapshot!(content, @"Fixed!\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "foo2", "-r", "@"]);
    insta::assert_snapshot!(content, @"Fixed!\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "dir/foo3", "-r", "@"]);
    insta::assert_snapshot!(content, @"unfixed\n");
}

#[test]
fn test_fix_empty_commit() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
}

#[test]
fn test_fix_leaf_commit() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "unaffected").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "affected").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: rlvkpnrz 85ce8924 (no description set)
    Parent commit      : qpvuntsm b2ca2bc5 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@-"]);
    insta::assert_snapshot!(content, @"unaffected");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"AFFECTED");
}

#[test]
fn test_fix_parent_commit() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    // Using one file name for all commits adds coverage of some possible bugs.
    std::fs::write(repo_path.join("file"), "parent").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "parent"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "child1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "child1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "parent"]);
    std::fs::write(repo_path.join("file"), "child2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "child2"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "parent"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 3 commits of 3 checked.
    Working copy now at: mzvwutvl d30c8ae2 child2 | (no description set)
    Parent commit      : qpvuntsm 70a4dae2 parent | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "parent"]);
    insta::assert_snapshot!(content, @"PARENT");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child1"]);
    insta::assert_snapshot!(content, @"CHILD1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child2"]);
    insta::assert_snapshot!(content, @"CHILD2");
}

#[test]
fn test_fix_sibling_commit() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "parent").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "parent"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "child1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "child1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "parent"]);
    std::fs::write(repo_path.join("file"), "child2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "child2"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "child1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "parent"]);
    insta::assert_snapshot!(content, @"parent");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child1"]);
    insta::assert_snapshot!(content, @"CHILD1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "child2"]);
    insta::assert_snapshot!(content, @"child2");
}

#[test]
fn test_default_revset() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "trunk1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "trunk1"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "trunk2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "trunk2"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "trunk1"]);
    std::fs::write(repo_path.join("file"), "foo").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "trunk1"]);
    std::fs::write(repo_path.join("file"), "bar1").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar1"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar2").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar2"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar3").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar3"]);
    test_env.jj_cmd_ok(&repo_path, &["edit", "bar2"]);

    // With no args and no revset configuration, we fix `reachable(@, mutable())`,
    // which includes bar{1,2,3} and excludes trunk{1,2} (which is immutable) and
    // foo (which is mutable but not reachable).
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "trunk2""#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 3 commits of 3 checked.
    Working copy now at: yostqsxw dabc47b2 bar2 | (no description set)
    Parent commit      : yqosqzyt 984b5924 bar1 | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "trunk1"]);
    insta::assert_snapshot!(content, @"trunk1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "trunk2"]);
    insta::assert_snapshot!(content, @"trunk2");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "foo"]);
    insta::assert_snapshot!(content, @"foo");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar1"]);
    insta::assert_snapshot!(content, @"BAR1");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar2"]);
    insta::assert_snapshot!(content, @"BAR2");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar3"]);
    insta::assert_snapshot!(content, @"BAR3");
}

#[test]
fn test_custom_default_revset() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);

    std::fs::write(repo_path.join("file"), "foo").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "foo"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "bar"]);

    // Check out a different commit so that the schema default `reachable(@,
    // mutable())` would behave differently from our customized default.
    test_env.jj_cmd_ok(&repo_path, &["new", "-r", "foo"]);
    test_env.add_config(r#"revsets.fix = "bar""#);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "foo"]);
    insta::assert_snapshot!(content, @"foo");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "bar"]);
    insta::assert_snapshot!(content, @"BAR");
}

#[test]
fn test_fix_immutable_commit() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "immutable").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "immutable"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "mutable").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "mutable"]);
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "immutable""#);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["fix", "-s", "immutable"]);
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Error: Commit e4b41a3ce243 is immutable
    Hint: Pass `--ignore-immutable` or configure the set of immutable commits via `revset-aliases.immutable_heads()`.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "immutable"]);
    insta::assert_snapshot!(content, @"immutable");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "mutable"]);
    insta::assert_snapshot!(content, @"mutable");
}

#[test]
fn test_fix_empty_file() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"");
}

#[test]
fn test_fix_some_paths() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    std::fs::write(repo_path.join("file2"), "bar").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 54a90d2b (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file1"]);
    insta::assert_snapshot!(content, @r###"
    FOO
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file2"]);
    insta::assert_snapshot!(content, @"bar");
}

#[test]
fn test_fix_cyclic() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--reverse"]);
    std::fs::write(repo_path.join("file"), "content\n").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--reverse"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm bf5e6a5a (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"tnetnoc\n");

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--reverse"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 0e2d20d6 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content\n");
}

#[test]
fn test_deduplication() {
    // Append all fixed content to a log file. This assumes we're running the tool
    // in the root directory of the repo, which is worth reconsidering if we
    // establish a contract regarding cwd.
    let (test_env, repo_path, redact) =
        init_with_fake_formatter(&["--uppercase", "--tee", "$path-fixlog"]);

    // There are at least two interesting cases: the content is repeated immediately
    // in the child commit, or later in another descendant.
    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "bar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "c"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "d"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "a"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase", "--tee", "$path-fixlog"]
                patterns = ["all()"]
                
    Fixed 4 commits of 4 checked.
    Working copy now at: yqosqzyt cf770245 d | (no description set)
    Parent commit      : mzvwutvl 370615a5 c | (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "a"]);
    insta::assert_snapshot!(content, @"FOO\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "b"]);
    insta::assert_snapshot!(content, @"BAR\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "c"]);
    insta::assert_snapshot!(content, @"BAR\n");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "d"]);
    insta::assert_snapshot!(content, @"FOO\n");

    // Each new content string only appears once in the log, because all the other
    // inputs (like file name) were identical, and so the results were re-used. We
    // sort the log because the order of execution inside `jj fix` is undefined.
    insta::assert_snapshot!(sorted_lines(repo_path.join("file-fixlog")), @"BAR\nFOO\n");
}

fn sorted_lines(path: PathBuf) -> String {
    let mut log: Vec<_> = std::fs::read_to_string(path.as_os_str())
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    log.sort();
    log.join("\n")
}

#[test]
fn test_executed_but_nothing_changed() {
    // Show that the tool ran by causing a side effect with --tee, and test that we
    // do the right thing when the tool's output is exactly equal to its input.
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--tee", "$path-copy"]);
    std::fs::write(repo_path.join("file"), "content\n").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--tee", "$path-copy"]
                patterns = ["all()"]
                
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content\n");
    let copy_content = std::fs::read_to_string(repo_path.join("file-copy").as_os_str()).unwrap();
    insta::assert_snapshot!(copy_content, @"content\n");
}

#[test]
fn test_failure() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--fail"]);
    std::fs::write(repo_path.join("file"), "content").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--fail"]
                patterns = ["all()"]
                
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"content");
}

#[test]
fn test_stderr_success() {
    let (test_env, repo_path, redact) =
        init_with_fake_formatter(&["--stderr", "error", "--stdout", "new content"]);
    std::fs::write(repo_path.join("file"), "old content").unwrap();

    // TODO: Associate the stderr lines with the relevant tool/file/commit instead
    // of passing it through directly.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--stderr", "error", "--stdout", "new content"]
                patterns = ["all()"]
                
    errorFixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 487808ba (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"new content");
}

#[test]
fn test_stderr_failure() {
    let (test_env, repo_path, redact) =
        init_with_fake_formatter(&["--stderr", "error", "--stdout", "new content", "--fail"]);
    std::fs::write(repo_path.join("file"), "old content").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--stderr", "error", "--stdout", "new content", "--fail"]
                patterns = ["all()"]
                
    errorFixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"old content");
}

#[test]
fn test_missing_command() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(r#"fix.tool-command = ["this_executable_shouldnt_exist"]"#);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    // TODO: We should display a warning about invalid tool configurations. When we
    // support multiple tools, we should also keep going to see if any of the other
    // executions succeed.
    insta::assert_snapshot!(stderr, @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = ["this_executable_shouldnt_exist"]
                patterns = ["all()"]
                
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
}

#[test]
fn test_fix_file_types() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "content").unwrap();
    std::fs::create_dir(repo_path.join("dir")).unwrap();
    try_symlink("file", repo_path.join("link")).unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm 6836a9e4 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"CONTENT");
}

#[cfg(unix)]
#[test]
fn test_fix_executable() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    let path = repo_path.join("file");
    std::fs::write(&path, "content").unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(&path, permissions).unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: qpvuntsm fee78e99 (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @"CONTENT");
    let executable = std::fs::metadata(&path).unwrap().permissions().mode() & 0o111;
    assert_eq!(executable, 0o111);
}

#[test]
fn test_fix_trivial_merge_commit() {
    // All the changes are attributable to a parent, so none are fixed (in the same
    // way that none would be shown in `jj diff -r @`).
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file_a"), "content a").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file_b"), "content b").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 0 commits of 1 checked.
    Nothing changed.
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_a", "-r", "@"]);
    insta::assert_snapshot!(content, @"content a");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_b", "-r", "@"]);
    insta::assert_snapshot!(content, @"content b");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_c", "-r", "@"]);
    insta::assert_snapshot!(content, @"content c");
}

#[test]
fn test_fix_adding_merge_commit() {
    // None of the changes are attributable to a parent, so they are all fixed (in
    // the same way that they would be shown in `jj diff -r @`).
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file_a"), "content a").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file_b"), "content b").unwrap();
    std::fs::write(repo_path.join("file_c"), "content c").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);
    std::fs::write(repo_path.join("file_a"), "change a").unwrap();
    std::fs::write(repo_path.join("file_b"), "change b").unwrap();
    std::fs::write(repo_path.join("file_c"), "change c").unwrap();
    std::fs::write(repo_path.join("file_d"), "change d").unwrap();

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 1 commits of 1 checked.
    Working copy now at: mzvwutvl f93eb5a9 (no description set)
    Parent commit      : qpvuntsm 6e64e7a7 a | (no description set)
    Parent commit      : kkmpptxz c536f264 b | (no description set)
    Added 0 files, modified 4 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_a", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE A");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_b", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE B");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_c", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE C");
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file_d", "-r", "@"]);
    insta::assert_snapshot!(content, @"CHANGE D");
}

#[test]
fn test_fix_both_sides_of_conflict() {
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "content a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file"), "content b\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);

    // The conflicts are not different from the merged parent, so they would not be
    // fixed if we didn't fix the parents also.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "a", "-s", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 3 commits of 3 checked.
    Working copy now at: mzvwutvl 88866235 (conflict) (empty) (no description set)
    Parent commit      : qpvuntsm 8e8aad69 a | (no description set)
    Parent commit      : kkmpptxz 91f9b284 b | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    There are unresolved conflicts at these paths:
    file    2-sided conflict
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "a"]);
    insta::assert_snapshot!(content, @r###"
    CONTENT A
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "b"]);
    insta::assert_snapshot!(content, @r###"
    CONTENT B
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    +CONTENT A
    +++++++ Contents of side #2
    CONTENT B
    >>>>>>> Conflict 1 of 1 ends
    "###);
}

#[test]
fn test_fix_resolve_conflict() {
    // If both sides of the conflict look the same after being fixed, the conflict
    // will be resolved.
    let (test_env, repo_path, redact) = init_with_fake_formatter(&["--uppercase"]);
    std::fs::write(repo_path.join("file"), "Content\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "a"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    std::fs::write(repo_path.join("file"), "cOnTeNt\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "b"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "a", "b"]);

    // The conflicts are not different from the merged parent, so they would not be
    // fixed if we didn't fix the parents also.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "-s", "a", "-s", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(redact(&stderr), @r###"
    Warning: The `fix.tool-command` config option is deprecated and will be removed in a future version.
    Hint: Replace it with the following:
                [fix.tools.legacy-tool-command]
                command = [<redacted formatter path>, "--uppercase"]
                patterns = ["all()"]
                
    Fixed 3 commits of 3 checked.
    Working copy now at: mzvwutvl 50fd048d (empty) (no description set)
    Parent commit      : qpvuntsm dd2721f1 a | (no description set)
    Parent commit      : kkmpptxz 07c27a8e b | (no description set)
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "file", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    CONTENT
    "###);
}

#[test]
fn test_all_files() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let formatter_path = assert_cmd::cargo::cargo_bin("fake-formatter");
    assert!(formatter_path.is_file());
    let escaped_formatter_path = formatter_path.to_str().unwrap().replace('\\', r"\\");

    // Consider a few cases:
    // File A:     in patterns,     changed in child
    // File B:     in patterns, NOT changed in child
    // File C: NOT in patterns, NOT changed in child
    // File D: NOT in patterns,     changed in child
    // Some files will be in subdirectories to make sure we're covering that aspect
    // of matching.
    test_env.add_config(&format!(
        r###"
        [fix.tools.tool]
        command = ["{formatter}", "--append", "fixed"]
        patterns = ["a/a", "b/b"]
        "###,
        formatter = escaped_formatter_path.as_str()
    ));

    std::fs::create_dir(repo_path.join("a")).unwrap();
    std::fs::create_dir(repo_path.join("b")).unwrap();
    std::fs::create_dir(repo_path.join("c")).unwrap();
    std::fs::write(repo_path.join("a/a"), "parent aaa\n").unwrap();
    std::fs::write(repo_path.join("b/b"), "parent bbb\n").unwrap();
    std::fs::write(repo_path.join("c/c"), "parent ccc\n").unwrap();
    std::fs::write(repo_path.join("ddd"), "parent ddd\n").unwrap();
    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "parent"]);

    std::fs::write(repo_path.join("a/a"), "child aaa\n").unwrap();
    std::fs::write(repo_path.join("ddd"), "child ddd\n").unwrap();
    let (_stdout, _stderr) = test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "child"]);

    // Specifying files means exactly those files will be fixed in each revision,
    // although some like file C won't have any tools configured to make changes to
    // them. Specified but unfixed files are silently skipped, whether they lack
    // configuration, are ignored, don't exist, aren't normal files, etc.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &[
            "fix",
            "--include-unchanged-files",
            "b/b",
            "c/c",
            "does_not.exist",
        ],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 2 commits of 2 checked.
    Working copy now at: rlvkpnrz c098d165 child
    Parent commit      : qpvuntsm 0bb31627 parent
    Added 0 files, modified 1 files, removed 0 files
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "a/a", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent aaa
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "b/b", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent bbb
    fixed
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "c/c", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent ccc
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "ddd", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent ddd
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "a/a", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    child aaa
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "b/b", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    parent bbb
    fixed
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "c/c", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    parent ccc
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "ddd", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    child ddd
    "###);

    // Not specifying files means all files will be fixed in each revision.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["fix", "--include-unchanged-files"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Fixed 2 commits of 2 checked.
    Working copy now at: rlvkpnrz c5d0aa1d child
    Parent commit      : qpvuntsm b4d02ca9 parent
    Added 0 files, modified 2 files, removed 0 files
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "a/a", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent aaa
    fixed
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "b/b", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent bbb
    fixed
    fixed
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "c/c", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent ccc
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "ddd", "-r", "@-"]);
    insta::assert_snapshot!(content, @r###"
    parent ddd
    "###);

    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "a/a", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    child aaa
    fixed
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "b/b", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    parent bbb
    fixed
    fixed
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "c/c", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    parent ccc
    "###);
    let content = test_env.jj_cmd_success(&repo_path, &["file", "show", "ddd", "-r", "@"]);
    insta::assert_snapshot!(content, @r###"
    child ddd
    "###);
}
