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
