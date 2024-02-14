// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use itertools::Itertools;
use regex::Regex;

use crate::common::TestEnvironment;

#[test]
fn test_show() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["show"]);
    let stdout = stdout.lines().skip(2).join("\n");

    insta::assert_snapshot!(stdout, @r###"
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)

        (no description set)
    "###);
}

#[test]
fn test_show_with_template() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "a new commit"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "-T", "description"]);

    insta::assert_snapshot!(stdout, @r###"
    a new commit
    "###);
}

#[test]
fn test_show_relative_timestamps() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(
        r#"
        [template-aliases]
        'format_timestamp(timestamp)' = 'timestamp.ago()'
        "#,
    );

    let stdout = test_env.jj_cmd_success(&repo_path, &["show"]);
    let timestamp_re = Regex::new(r"\([0-9]+ years ago\)").unwrap();
    let stdout = stdout
        .lines()
        .skip(2)
        .map(|x| timestamp_re.replace_all(x, "(...timestamp...)"))
        .join("\n");

    insta::assert_snapshot!(stdout, @r###"
    Author: Test User <test.user@example.com> (...timestamp...)
    Committer: Test User <test.user@example.com> (...timestamp...)

        (no description set)
    "###);
}
