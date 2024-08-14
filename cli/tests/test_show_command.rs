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

use itertools::Itertools;
use regex::Regex;

use crate::common::TestEnvironment;

#[test]
fn test_show() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["show"]);
    let stdout = stdout.lines().skip(2).join("\n");

    insta::assert_snapshot!(stdout, @r###"
    Author: Test User <test.user@example.com> (2001-02-03 08:05:07)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:07)

        (no description set)
    "###);
}

#[test]
fn test_show_basic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\nbaz qux\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "foo\nbar\nbaz quux\n").unwrap();
    std::fs::write(repo_path.join("file3"), "foo\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["show"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    Removed regular file file1:
       1     : foo
    Modified regular file file2:
       1    1: foo
            2: bar
       2    3: baz quxquux
    Modified regular file file3 (file1 => file3):
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--context=0"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    Removed regular file file1:
       1     : foo
    Modified regular file file2:
       1    1: foo
            2: bar
       2    3: baz quxquux
    Modified regular file file3 (file1 => file3):
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--color=debug"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: [38;5;4m<<commit_id::e34f04317a81edc6ba41fef239c0d0180f10656f>>[39m
    Change ID: [38;5;5m<<change_id::rlvkpnrzqnoowoytxnquwvuryrwnrmlp>>[39m
    Author: <<author name::Test User>> <[38;5;3m<<author email::test.user@example.com>>[39m> ([38;5;6m<<author timestamp local format::2001-02-03 08:05:09>>[39m)
    Committer: <<committer name::Test User>> <[38;5;3m<<committer email::test.user@example.com>>[39m> ([38;5;6m<<committer timestamp local format::2001-02-03 08:05:09>>[39m)

    [38;5;3m<<description placeholder::    (no description set)>>[39m

    [38;5;3m<<diff header::Removed regular file file1:>>[39m
    [38;5;1m<<diff removed line_number::   1>>[39m<<diff::     : >>[4m[38;5;1m<<diff removed token::foo>>[24m[39m
    [38;5;3m<<diff header::Modified regular file file2:>>[39m
    [38;5;1m<<diff removed line_number::   1>>[39m<<diff:: >>[38;5;2m<<diff added line_number::   1>>[39m<<diff::: foo>>
    <<diff::     >>[38;5;2m<<diff added line_number::   2>>[39m<<diff::: >>[4m[38;5;2m<<diff added token::bar>>[24m[39m
    [38;5;1m<<diff removed line_number::   2>>[39m<<diff:: >>[38;5;2m<<diff added line_number::   3>>[39m<<diff::: baz >>[4m[38;5;1m<<diff removed token::qux>>[38;5;2m<<diff added token::quux>>[24m[39m<<diff::>>
    [38;5;3m<<diff header::Modified regular file file3 (file1 => file3):>>[39m
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    M file2
    R {file1 => file3}
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--types"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    FF file2
    FF {file1 => file3}
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    diff --git a/file2 b/file2
    index 523a4a9de8..485b56a572 100644
    --- a/file2
    +++ b/file2
    @@ -1,2 +1,3 @@
     foo
    -baz qux
    +bar
    +baz quux
    diff --git a/file1 b/file3
    rename from file1
    rename to file3
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--git", "--context=0"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    diff --git a/file2 b/file2
    index 523a4a9de8..485b56a572 100644
    --- a/file2
    +++ b/file2
    @@ -2,1 +2,2 @@
    -baz qux
    +bar
    +baz quux
    diff --git a/file1 b/file3
    rename from file1
    rename to file3
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--git", "--color=debug"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: [38;5;4m<<commit_id::e34f04317a81edc6ba41fef239c0d0180f10656f>>[39m
    Change ID: [38;5;5m<<change_id::rlvkpnrzqnoowoytxnquwvuryrwnrmlp>>[39m
    Author: <<author name::Test User>> <[38;5;3m<<author email::test.user@example.com>>[39m> ([38;5;6m<<author timestamp local format::2001-02-03 08:05:09>>[39m)
    Committer: <<committer name::Test User>> <[38;5;3m<<committer email::test.user@example.com>>[39m> ([38;5;6m<<committer timestamp local format::2001-02-03 08:05:09>>[39m)

    [38;5;3m<<description placeholder::    (no description set)>>[39m

    [1m<<diff file_header::diff --git a/file2 b/file2>>[0m
    [1m<<diff file_header::index 523a4a9de8..485b56a572 100644>>[0m
    [1m<<diff file_header::--- a/file2>>[0m
    [1m<<diff file_header::+++ b/file2>>[0m
    [38;5;6m<<diff hunk_header::@@ -1,2 +1,3 @@>>[39m
    <<diff context:: foo>>
    [38;5;1m<<diff removed::-baz >>[4m<<diff removed token::qux>>[24m<<diff removed::>>[39m
    [38;5;2m<<diff added::+>>[4m<<diff added token::bar>>[24m[39m
    [38;5;2m<<diff added::+baz >>[4m<<diff added token::quux>>[24m<<diff added::>>[39m
    [1m<<diff file_header::diff --git a/file1 b/file3>>[0m
    [1m<<diff file_header::rename from file1>>[0m
    [1m<<diff file_header::rename to file3>>[0m
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "-s", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    M file2
    R {file1 => file3}
    diff --git a/file2 b/file2
    index 523a4a9de8..485b56a572 100644
    --- a/file2
    +++ b/file2
    @@ -1,2 +1,3 @@
     foo
    -baz qux
    +bar
    +baz quux
    diff --git a/file1 b/file3
    rename from file1
    rename to file3
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: e34f04317a81edc6ba41fef239c0d0180f10656f
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:09)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    file2            | 3 ++-
    {file1 => file3} | 0
    2 files changed, 2 insertions(+), 1 deletion(-)
    "###);
}

#[test]
fn test_show_with_template() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "a new commit"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "-T", "description"]);

    insta::assert_snapshot!(stdout, @r###"
    a new commit
    "###);
}

#[test]
fn test_show_with_no_template() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["show", "-T"]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '--template <TEMPLATE>' but none was supplied

    For more information, try '--help'.
    Hint: The following template aliases are defined:
    - builtin_log_comfortable
    - builtin_log_compact
    - builtin_log_detailed
    - builtin_log_node
    - builtin_log_node_ascii
    - builtin_log_oneline
    - builtin_op_log_comfortable
    - builtin_op_log_compact
    - builtin_op_log_node
    - builtin_op_log_node_ascii
    - commit_summary_separator
    - description_placeholder
    - email_placeholder
    - name_placeholder
    "###);
}

#[test]
fn test_show_relative_timestamps() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
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
