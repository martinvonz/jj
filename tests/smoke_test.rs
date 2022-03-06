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

use jujutsu::testutils::{get_stdout_string, TestEnvironment};

#[test]
fn smoke_test() {
    let test_env = TestEnvironment::default();
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo", "--git"])
        .assert()
        .success();

    let repo_path = test_env.env_root().join("repo");
    // Check the output of `jj status` right after initializing repo
    let assert = test_env
        .jj_cmd(&repo_path, &["status"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    Parent commit: 000000000000 
    Working copy : 1d1984a23811 
    The working copy is clean
    "###);

    // Write some files and check the output of `jj status`
    std::fs::write(repo_path.join("file1"), "file1").unwrap();
    std::fs::write(repo_path.join("file2"), "file2").unwrap();
    std::fs::write(repo_path.join("file3"), "file3").unwrap();

    // The working copy's ID should have changed
    let assert = test_env
        .jj_cmd(&repo_path, &["status"])
        .assert()
        .success();
    let stdout_string = get_stdout_string(&assert);
    insta::assert_snapshot!(stdout_string, @r###"
    Parent commit: 000000000000 
    Working copy : 5e60c5091e43 
    Working copy changes:
    A file1
    A file2
    A file3
    "###);

    // Running `jj status` again gives the same output
    test_env
        .jj_cmd(&repo_path, &["status"])
        .assert()
        .success()
        .stdout(stdout_string);

    // Add a commit description
    let assert = test_env
        .jj_cmd(&repo_path, &["describe", "-m", "add some files"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"Working copy now at: 6f13b3e41065 add some files
");

    // Close the commit
    let assert = test_env
        .jj_cmd(&repo_path, &["close"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"Working copy now at: 6ff8a22d8ce1 
");
}
