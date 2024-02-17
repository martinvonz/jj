// Copyright 2023 The Jujutsu Authors
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
fn test_snapshot_large_file() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"snapshot.max-new-file-size = "10""#);
    std::fs::write(repo_path.join("large"), "a lot of text").unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["files"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to snapshot the working copy
    Caused by: New file $TEST_ENV/repo/large of size ~13.0B exceeds snapshot.max-new-file-size (10.0B)
    Hint: Increase the value of the `snapshot.max-new-file-size` config option if you
    want this file to be snapshotted. Otherwise add it to your `.gitignore` file.
    "###);
}

#[test]
fn test_consecutive_snapshots() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"snapshot.squash-consecutive-snapshots = true"#);

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "sample text"]);
    // initial WC is a predecessor of a described WC now

    std::fs::write(repo_path.join("a"), "").unwrap();
    test_env.jj_cmd_success(&repo_path, &["files"]);

    std::fs::write(repo_path.join("b"), "").unwrap();
    test_env.jj_cmd_success(&repo_path, &["files"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["operation", "log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  70756af40ab9 test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    │  snapshot working copy
    │  args: jj files
    │  snapshots: 2
    ◉  4f5b67be313f test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'sample text'
    ◉  6ac4339ad699 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  1b0049c19762 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  initialize repo
    ◉  000000000000 root()
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["obslog"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 04:05:10.000 +07:00 bf6e46e6
    │  sample text
    ◉  qpvuntsm hidden test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059
       (empty) (no description set)
    "###);
}
