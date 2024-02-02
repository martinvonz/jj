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
