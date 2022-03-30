// Copyright 2022 Google LLC
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
fn test_restore() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file2"), "b\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "c\n").unwrap();
    std::fs::write(repo_path.join("file3"), "c\n").unwrap();

    // Restores from parent by default
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore"]);
    insta::assert_snapshot!(stdout, @r###"
    Created b05f8b84f2fc 
    Working copy now at: b05f8b84f2fc 
    Added 1 files, modified 1 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"");

    // Can restore from other revision
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "--from", "@--"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 9cb58509136b 
    Working copy now at: 9cb58509136b 
    Added 1 files, modified 0 files, removed 2 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"R file2
");

    // Can restore into other revision
    // TODO: Maybe we should make `--from @` be the default when using `--to`. That
    // would probably mean declaring both `--from` and `--to` optional and
    // without default values and handle the defaults in code.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "--from", "@", "--to", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    Created 5ed06151e039 
    Rebased 1 descendant commits
    Working copy now at: ca6c95b68bd2 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    A file2
    A file3
    "###);

    // Can restore only specified paths
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["restore", "file2", "file3"]);
    insta::assert_snapshot!(stdout, @r###"
    Created a5bd47134d7e 
    Working copy now at: a5bd47134d7e 
    Added 0 files, modified 1 files, removed 1 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @"R file1
");
}
