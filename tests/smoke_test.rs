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

use jujube::testutils;
use regex::Regex;

#[test]
fn smoke_test() {
    let temp_dir = tempfile::tempdir().unwrap();

    let output = testutils::CommandRunner::new(temp_dir.path()).run(vec!["init", "repo"]);
    assert_eq!(output.status, 0);
    let repo_path = temp_dir.path().join("repo");

    // Check the output of `jj status` right after initializing repo
    let output = testutils::CommandRunner::new(&repo_path).run(vec!["status"]);
    assert_eq!(output.status, 0);
    let stdout_string = output.stdout_string();
    let output_regex = Regex::new(
        "^Working copy : ([[:xdigit:]]+) \n\
             Parent commit: 000000000000 \n\
             Diff summary:\n\
             $",
    )
    .unwrap();
    assert!(
        output_regex.is_match(&stdout_string),
        "output was: {}",
        stdout_string
    );
    let wc_hex_id_empty = output_regex
        .captures(&stdout_string)
        .unwrap()
        .get(1)
        .unwrap()
        .as_str()
        .to_owned();

    // Write some files and check the output of `jj status`
    std::fs::write(repo_path.join("file1"), "file1").unwrap();
    std::fs::write(repo_path.join("file2"), "file2").unwrap();
    std::fs::write(repo_path.join("file3"), "file3").unwrap();

    let output = testutils::CommandRunner::new(&repo_path).run(vec!["status"]);
    assert_eq!(output.status, 0);
    let stdout_string = output.stdout_string();
    let output_regex = Regex::new(
        "^Working copy : ([[:xdigit:]]+) \n\
             Parent commit: 000000000000 \n\
             Diff summary:\n\
             A file1\n\
             A file2\n\
             A file3\n\
             $",
    )
    .unwrap();
    assert!(
        output_regex.is_match(&stdout_string),
        "output was: {}",
        stdout_string
    );
    let wc_hex_id_non_empty = output_regex
        .captures(&stdout_string)
        .unwrap()
        .get(1)
        .unwrap()
        .as_str()
        .to_owned();

    // The working copy's id should have changed
    assert_ne!(wc_hex_id_empty, wc_hex_id_non_empty);

    // Running `jj status` again gives the same output
    let output2 = testutils::CommandRunner::new(&repo_path).run(vec!["status"]);
    assert_eq!(output, output2);

    // Add a commit description
    let output =
        testutils::CommandRunner::new(&repo_path).run(vec!["describe", "--text", "add some files"]);
    assert_eq!(output.status, 0);
    let stdout_string = output.stdout_string();
    let output_regex =
        Regex::new("^leaving: [[:xdigit:]]+ \nnow at: [[:xdigit:]]+ add some files\n$").unwrap();
    assert!(
        output_regex.is_match(&stdout_string),
        "output was: {}",
        stdout_string
    );

    // Close the commit
    let output = testutils::CommandRunner::new(&repo_path).run(vec!["close"]);
    assert_eq!(output.status, 0);
    let stdout_string = output.stdout_string();
    let output_regex =
        Regex::new("^leaving: [[:xdigit:]]+ add some files\nnow at: [[:xdigit:]]+ \n$").unwrap();
    assert!(
        output_regex.is_match(&stdout_string),
        "output was: {}",
        stdout_string
    );
}
