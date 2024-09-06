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

use crate::common::get_stdout_string;
use crate::common::TestEnvironment;

#[test]
fn test_snapshot_large_file() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // test a small file using raw-integer-literal syntax, which is interpreted
    // in bytes
    test_env.add_config(r#"snapshot.max-new-file-size = 10"#);
    std::fs::write(repo_path.join("large"), "a lot of text").unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to snapshot the working copy
    The file '$TEST_ENV/repo/large' is too large to be snapshotted: it is 3 bytes too large; the maximum size allowed is 10 bytes (10.0B).
    Hint: This is to prevent large files from being added on accident. You can fix this error by:
      - Adding the file to `.gitignore`
      - Run `jj config set --repo snapshot.max-new-file-size 13`
        This will increase the maximum file size allowed for new files, in this repository only.
      - Run `jj --config-toml 'snapshot.max-new-file-size=13' st`
        This will increase the maximum file size allowed for new files, for this command only.
    "###);

    // test with a larger file using 'KB' human-readable syntax
    test_env.add_config(r#"snapshot.max-new-file-size = "10KB""#);
    let big_string = vec![0; 1024 * 11];
    std::fs::write(repo_path.join("large"), big_string).unwrap();
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to snapshot the working copy
    The file '$TEST_ENV/repo/large' is too large to be snapshotted: it is 1024 bytes too large; the maximum size allowed is 10240 bytes (10.0KiB).
    Hint: This is to prevent large files from being added on accident. You can fix this error by:
      - Adding the file to `.gitignore`
      - Run `jj config set --repo snapshot.max-new-file-size 11264`
        This will increase the maximum file size allowed for new files, in this repository only.
      - Run `jj --config-toml 'snapshot.max-new-file-size=11264' st`
        This will increase the maximum file size allowed for new files, for this command only.
    "###);
}

#[test]
fn test_snapshot_cachedir_tag_file() {
    // ensure that a CACHEDIR.TAG file and all the files in its tree are not
    // snapshotted: https://bford.info/cachedir/
    let sig = r#"Signature: 8a477f597d28d172789f06886806bc55
    # Some of these CACHEDIR files have comments after the first 43 bytes,
    # so add one here to make sure the test accounts for it
    "#;
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    // things are normal
    std::fs::write(repo_path.join("a"), "").unwrap();
    std::fs::write(repo_path.join("b"), "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    A a
    A b
    Working copy : qpvuntsm b6ccb224 (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // oops, we accidentally snapshotted the buck-out/ directory
    std::fs::create_dir_all(repo_path.join("buck-out").join("dir")).unwrap();
    std::fs::write(repo_path.join("buck-out/c"), "").unwrap();
    std::fs::write(repo_path.join("buck-out/dir/d"), "").unwrap();
    let assert = test_env.jj_cmd(&repo_path, &["status"]).assert().code(0);
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    A a
    A b
    A buck-out/c
    A buck-out/dir/d
    Working copy : qpvuntsm 9172549e (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // write an invalid CACHEDIR.TAG file, and we should see the bad
    // outcome again
    std::fs::write(repo_path.join("buck-out/CACHEDIR.TAG"), "").unwrap();
    let assert = test_env.jj_cmd(&repo_path, &["status"]).assert().code(0);
    let stdout = test_env.normalize_output(&get_stdout_string(&assert));
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    A a
    A b
    A buck-out/CACHEDIR.TAG
    A buck-out/c
    A buck-out/dir/d
    Working copy : qpvuntsm f1bf2de4 (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // fixed
    std::fs::write(repo_path.join("buck-out/CACHEDIR.TAG"), sig).unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    A a
    A b
    Working copy : qpvuntsm eca4b62b (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);
}
