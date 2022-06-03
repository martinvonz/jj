// Copyright 2022 Google LLC
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

use common::TestEnvironment;
use regex::Regex;

pub mod common;

#[test]
fn test_obslog_with_or_without_diff() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "my description"]);
    std::fs::write(repo_path.join("file1"), "foo\nbar\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["rebase", "-r", "@", "-d", "root"]);
    std::fs::write(repo_path.join("file1"), "resolved\n").unwrap();

    let stdout = get_log_output(&test_env, &repo_path, &["obslog"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 1daafc17fefb test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    | my description
    o 813918f7b4e6 test.user@example.com 2001-02-03 04:05:08.000 +07:00    conflict
    | my description
    o 8f02f5470c55 test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    | my description
    o c8ceb219336b test.user@example.com 2001-02-03 04:05:08.000 +07:00   
      my description
    "###);

    // There should be no diff caused by the rebase because it was a pure rebase
    // (even even though it resulted in a conflict).
    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "-p"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 1daafc17fefb test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    | my description
    | Resolved conflict in file1:
    |    1    1: <<<<<<<resolved
    |    2     : -------
    |    3     : +++++++
    |    4     : +bar
    |    5     : >>>>>>>
    o 813918f7b4e6 test.user@example.com 2001-02-03 04:05:08.000 +07:00    conflict
    | my description
    o 8f02f5470c55 test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    | my description
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    | Added regular file file2:
    |         1: foo
    o c8ceb219336b test.user@example.com 2001-02-03 04:05:08.000 +07:00   
      my description
    "###);

    // Test `--no-graph`
    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    1daafc17fefb test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    my description
    813918f7b4e6 test.user@example.com 2001-02-03 04:05:08.000 +07:00    conflict
    my description
    8f02f5470c55 test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    my description
    c8ceb219336b test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    my description
    "###);

    // Test `--git` format, and that it implies `-p`
    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "--no-graph", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    1daafc17fefb test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    my description
    diff --git a/file1 b/file1
    index e155302a24...2ab19ae607 100644
    --- a/file1
    +++ b/file1
    @@ -1,5 +1,1 @@
    -<<<<<<<
    --------
    -+++++++
    -+bar
    ->>>>>>>
    +resolved
    813918f7b4e6 test.user@example.com 2001-02-03 04:05:08.000 +07:00    conflict
    my description
    8f02f5470c55 test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    my description
    diff --git a/file1 b/file1
    index 257cc5642c...3bd1f0e297 100644
    --- a/file1
    +++ b/file1
    @@ -1,1 +1,2 @@
     foo
    +bar
    diff --git a/file2 b/file2
    new file mode 100644
    index 0000000000..257cc5642c
    --- /dev/null
    +++ b/file2
    @@ -1,0 +1,1 @@
    +foo
    c8ceb219336b test.user@example.com 2001-02-03 04:05:08.000 +07:00   
    my description
    "###);
}

#[test]
fn test_obslog_squash() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);
    std::fs::write(repo_path.join("file1"), "foo\nbar\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["squash"]);

    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "-p", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    o   c36a0819516d test.user@example.com 2001-02-03 04:05:07.000 +07:00   
    |\  first
    | | Modified regular file file1:
    | |    1    1: foo
    | |         2: bar
    o | 803a7299cb1a test.user@example.com 2001-02-03 04:05:07.000 +07:00   
    | | first
    | | Added regular file file1:
    | |         1: foo
    o | 85a1e2839620 test.user@example.com 2001-02-03 04:05:07.000 +07:00   
    | | first
    o | 230dd059e1b0 test.user@example.com 2001-02-03 04:05:07.000 +07:00   
     /  (no description set)
    o 69231a40d60d test.user@example.com 2001-02-03 04:05:09.000 +07:00   
    | second
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    o b567edda97ab test.user@example.com 2001-02-03 04:05:09.000 +07:00   
      second
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path, args: &[&str]) -> String {
    // Filter out the change ID since it's random
    let regex = Regex::new("^([o@| ]+)?([0-9a-f]{12}) ([0-9a-f]{12}) ").unwrap();
    let mut lines = vec![];
    let stdout = test_env.jj_cmd_success(repo_path, args);
    for line in stdout.split_inclusive('\n') {
        lines.push(regex.replace(line, "$1$2 "));
    }
    lines.join("")
}
