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

use std::io::Write;

use jujutsu::testutils::{get_stdout_string, TestEnvironment};

#[test]
fn test_gitignores() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&workspace_root).unwrap();
    test_env
        .jj_cmd(&workspace_root, &["init", "--git-repo", "."])
        .assert()
        .success();

    // Say in .git/info/exclude that we don't want file1 and file2
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace_root.join(".git").join("info").join("exclude"))
        .unwrap();
    file.write_all(b"file1\nfile2").unwrap();
    drop(file);

    // Say in .gitignore (in the working copy) that we actually do want file2
    std::fs::write(workspace_root.join(".gitignore"), "!file2").unwrap();

    // Writes some files to the working copy
    std::fs::write(workspace_root.join("file0"), "contents").unwrap();
    std::fs::write(workspace_root.join("file1"), "contents").unwrap();
    std::fs::write(workspace_root.join("file2"), "contents").unwrap();

    let assert = test_env
        .jj_cmd(&workspace_root, &["diff", "-s"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    A .gitignore
    A file0
    A file2
    "###);
}
