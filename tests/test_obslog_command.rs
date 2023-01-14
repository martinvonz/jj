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
    @  test.user@example.com 2001-02-03 04:05:10.000 +07:00 66b42ad36073
    | my description
    o  test.user@example.com 2001-02-03 04:05:09.000 +07:00 af536e5af67e conflict
    | my description
    o  test.user@example.com 2001-02-03 04:05:09.000 +07:00 6fbba7bcb590
    | my description
    o  test.user@example.com 2001-02-03 04:05:08.000 +07:00 eac0d0dae082
      (empty) my description
    "###);

    // There should be no diff caused by the rebase because it was a pure rebase
    // (even even though it resulted in a conflict).
    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "-p"]);
    insta::assert_snapshot!(stdout, @r###"
    @  test.user@example.com 2001-02-03 04:05:10.000 +07:00 66b42ad36073
    | my description
    | Resolved conflict in file1:
    |    1    1: <<<<<<<resolved
    |    2     : %%%%%%%
    |    3     : +bar
    |    4     : >>>>>>>
    o  test.user@example.com 2001-02-03 04:05:09.000 +07:00 af536e5af67e conflict
    | my description
    o  test.user@example.com 2001-02-03 04:05:09.000 +07:00 6fbba7bcb590
    | my description
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    | Added regular file file2:
    |         1: foo
    o  test.user@example.com 2001-02-03 04:05:08.000 +07:00 eac0d0dae082
      (empty) my description
    "###);

    // Test `--no-graph`
    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
     test.user@example.com 2001-02-03 04:05:10.000 +07:00 66b42ad36073
    my description
     test.user@example.com 2001-02-03 04:05:09.000 +07:00 af536e5af67e conflict
    my description
     test.user@example.com 2001-02-03 04:05:09.000 +07:00 6fbba7bcb590
    my description
     test.user@example.com 2001-02-03 04:05:08.000 +07:00 eac0d0dae082
    (empty) my description
    "###);

    // Test `--git` format, and that it implies `-p`
    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "--no-graph", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
     test.user@example.com 2001-02-03 04:05:10.000 +07:00 66b42ad36073
    my description
    diff --git a/file1 b/file1
    index e155302a24...2ab19ae607 100644
    --- a/file1
    +++ b/file1
    @@ -1,4 +1,1 @@
    -<<<<<<<
    -%%%%%%%
    -+bar
    ->>>>>>>
    +resolved
     test.user@example.com 2001-02-03 04:05:09.000 +07:00 af536e5af67e conflict
    my description
     test.user@example.com 2001-02-03 04:05:09.000 +07:00 6fbba7bcb590
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
     test.user@example.com 2001-02-03 04:05:08.000 +07:00 eac0d0dae082
    (empty) my description
    "###);
}

#[test]
fn test_obslog_squash() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);
    std::fs::write(repo_path.join("file1"), "foo\nbar\n").unwrap();

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(edit_script, "write\nsquashed").unwrap();
    test_env.jj_cmd_success(&repo_path, &["squash"]);

    let stdout = get_log_output(&test_env, &repo_path, &["obslog", "-p", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    o    test.user@example.com 2001-02-03 04:05:10.000 +07:00 27e721a5ba72
    |\  squashed
    | | Modified regular file file1:
    | |    1    1: foo
    | |         2: bar
    o |  test.user@example.com 2001-02-03 04:05:09.000 +07:00 9764e503e1a9
    | | first
    | | Added regular file file1:
    | |         1: foo
    o |  test.user@example.com 2001-02-03 04:05:08.000 +07:00 69542c1984c1
    | | (empty) first
    o |  test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
     /  (empty) (no description set)
    o  test.user@example.com 2001-02-03 04:05:10.000 +07:00 f09a38899f2b
    | second
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    o  test.user@example.com 2001-02-03 04:05:09.000 +07:00 579965369703
      (empty) second
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path, args: &[&str]) -> String {
    // Filter out the change ID since it's random
    let regex = Regex::new("^([o@| ]+)?([0-9a-f]{12})").unwrap();
    let mut lines = vec![];
    let stdout = test_env.jj_cmd_success(repo_path, args);
    for line in stdout.split_inclusive('\n') {
        lines.push(regex.replace(line, "$1"));
    }
    lines.join("")
}
