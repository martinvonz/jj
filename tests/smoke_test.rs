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

use jujutsu::testutils::{capture_matches, TestEnvironment};

#[test]
fn smoke_test() {
    let test_env = TestEnvironment::default();
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo"])
        .assert()
        .success();

    let repo_path = test_env.env_root().join("repo");
    // Check the output of `jj status` right after initializing repo
    let assert = test_env.jj_cmd(&repo_path, &["status"]).assert().success();
    let output_regex = "^Parent commit: 000000000000 \nWorking copy : ([[:xdigit:]]+) \nThe \
                        working copy is clean\n$";
    let (_, matches) = capture_matches(assert, output_regex);
    let wc_hex_id_empty = matches[0].clone();

    // Write some files and check the output of `jj status`
    std::fs::write(repo_path.join("file1"), "file1").unwrap();
    std::fs::write(repo_path.join("file2"), "file2").unwrap();
    std::fs::write(repo_path.join("file3"), "file3").unwrap();

    let assert = test_env.jj_cmd(&repo_path, &["status"]).assert().success();
    let output_regex = "^Parent commit: 000000000000 \nWorking copy : ([[:xdigit:]]+) \nWorking \
                        copy changes:
A file1
A file2
A file3
$";
    let (_, matches) = capture_matches(assert, output_regex);
    let wc_hex_id_non_empty = matches[0].clone();

    // The working copy's id should have changed
    assert_ne!(wc_hex_id_non_empty, wc_hex_id_empty);

    // Running `jj status` again gives the same output
    let assert = test_env.jj_cmd(&repo_path, &["status"]).assert().success();
    let (_, matches) = capture_matches(assert, output_regex);
    let wc_hex_id_again = matches[0].clone();
    assert_eq!(wc_hex_id_again, wc_hex_id_non_empty);

    // Add a commit description
    let assert = test_env
        .jj_cmd(&repo_path, &["describe", "-m", "add some files"])
        .assert()
        .success();
    let output_regex = "^Working copy now at: [[:xdigit:]]+ add some files
$";
    assert.stdout(predicates::str::is_match(output_regex).unwrap());

    // Close the commit
    let assert = test_env.jj_cmd(&repo_path, &["close"]).assert().success();
    let output_regex = "^Working copy now at: [[:xdigit:]]+ \n$";
    assert.stdout(predicates::str::is_match(output_regex).unwrap());
}
