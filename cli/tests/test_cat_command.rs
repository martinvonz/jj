// Copyright 2022 The Jujutsu Authors
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
fn test_cat() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "b\n").unwrap();
    std::fs::create_dir(repo_path.join("dir")).unwrap();
    std::fs::write(repo_path.join("dir").join("file2"), "c\n").unwrap();

    // Can print the contents of a file in a commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file1", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    a
    "###);

    // Defaults to printing the working-copy version
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // `print` is an alias for `cat`
    let stdout = test_env.jj_cmd_success(&repo_path, &["print", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);

    // Can print a file in a subdirectory
    let subdir_file = if cfg!(unix) {
        "dir/file2"
    } else {
        "dir\\file2"
    };
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", subdir_file]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Error if the path doesn't exist
    let stderr = test_env.jj_cmd_failure(&repo_path, &["cat", "nonexistent"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such path: nonexistent
    "###);

    // Can print files under the specified directory
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "dir"]);
    insta::assert_snapshot!(stdout, @r###"
    c
    "###);

    // Can print multiple files
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "."]);
    insta::assert_snapshot!(stdout, @r###"
    c
    b
    "###);

    // Unmatched paths should generate warnings
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["cat", "file1", "non-existent"]);
    insta::assert_snapshot!(stdout, @r###"
    b
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: No matching entries for paths: non-existent
    "###);

    // Can print a conflict
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "c\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["rebase", "-r", "@", "-d", "@--"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    -b
    +a
    +++++++ Contents of side #2
    c
    >>>>>>>
    "###);
}

#[cfg(unix)]
#[test]
fn test_cat_symlink() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    std::fs::create_dir(repo_path.join("dir")).unwrap();
    std::fs::write(repo_path.join("dir").join("file2"), "c\n").unwrap();
    std::os::unix::fs::symlink("symlink1_target", repo_path.join("symlink1")).unwrap();

    // Can print multiple files
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["cat", "."]);
    insta::assert_snapshot!(stdout, @r###"
    c
    a
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Path exists but is not a file: symlink1
    "###);
}
