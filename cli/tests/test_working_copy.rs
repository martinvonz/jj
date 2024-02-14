// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

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
