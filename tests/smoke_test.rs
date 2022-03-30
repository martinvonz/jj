// Copyright 2020 Google LLC
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

pub mod common;

#[test]
fn smoke_test() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);

    let repo_path = test_env.env_root().join("repo");
    // Check the output of `jj status` right after initializing repo
    let stdout = test_env.jj_cmd_success(&repo_path, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Parent commit: 000000000000 
    Working copy : 230dd059e1b0 
    The working copy is clean
    "###);

    // Write some files and check the output of `jj status`
    std::fs::write(repo_path.join("file1"), "file1").unwrap();
    std::fs::write(repo_path.join("file2"), "file2").unwrap();
    std::fs::write(repo_path.join("file3"), "file3").unwrap();

    // The working copy's ID should have changed
    let stdout = test_env.jj_cmd_success(&repo_path, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Parent commit: 000000000000 
    Working copy : d38745675403 
    Working copy changes:
    A file1
    A file2
    A file3
    "###);

    // Running `jj status` again gives the same output
    let stdout_again = test_env.jj_cmd_success(&repo_path, &["status"]);
    assert_eq!(stdout_again, stdout);

    // Add a commit description
    let stdout = test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add some files"]);
    insta::assert_snapshot!(stdout, @"Working copy now at: 701b3d5a2eb3 add some files
");

    // Close the commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["close"]);
    insta::assert_snapshot!(stdout, @"Working copy now at: a13f828fab1a 
");
}
