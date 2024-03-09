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

use std::path::Path;

use crate::common::TestEnvironment;

fn create_commit(
    test_env: &TestEnvironment,
    repo_path: &Path,
    name: &str,
    parents: &[&str],
    files: &[(&str, &str)],
) {
    if parents.is_empty() {
        test_env.jj_cmd_ok(repo_path, &["new", "root()", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_ok(repo_path, &args);
    }
    for (name, content) in files {
        std::fs::write(repo_path.join(name), content).unwrap();
    }
    test_env.jj_cmd_ok(repo_path, &["branch", "create", name]);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}

#[test]
fn test_chmod_regular_conflict() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "n", &["base"], &[("file", "n\n")]);
    create_commit(&test_env, &repo_path, "x", &["base"], &[("file", "x\n")]);
    // Test chmodding a file. The effect will be visible in the conflict below.
    test_env.jj_cmd_ok(&repo_path, &["chmod", "x", "file", "-r=x"]);
    create_commit(&test_env, &repo_path, "conflict", &["x", "n"], &[]);

    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    conflict
    ├─╮
    │ ◉  n
    ◉ │  x
    ├─╯
    ◉  base
    ◉
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    file: Conflicted([Some(File { id: FileId("587be6b4c3f93f93c489c0111bba5596147a26cb"), executable: true }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: false }), Some(File { id: FileId("8ba3a16384aacc37d01564b28401755ce8053f51"), executable: false })])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    <<<<<<<
    %%%%%%%
    -base
    +x
    +++++++
    n
    >>>>>>>
    "###);

    // Test chmodding a conflict
    test_env.jj_cmd_ok(&repo_path, &["chmod", "x", "file"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    file: Conflicted([Some(File { id: FileId("587be6b4c3f93f93c489c0111bba5596147a26cb"), executable: true }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: true }), Some(File { id: FileId("8ba3a16384aacc37d01564b28401755ce8053f51"), executable: true })])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    <<<<<<<
    %%%%%%%
    -base
    +x
    +++++++
    n
    >>>>>>>
    "###);
    test_env.jj_cmd_ok(&repo_path, &["chmod", "n", "file"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    file: Conflicted([Some(File { id: FileId("587be6b4c3f93f93c489c0111bba5596147a26cb"), executable: false }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: false }), Some(File { id: FileId("8ba3a16384aacc37d01564b28401755ce8053f51"), executable: false })])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    <<<<<<<
    %%%%%%%
    -base
    +x
    +++++++
    n
    >>>>>>>
    "###);

    // An error prevents `chmod` from making any changes.
    // In this case, the failure with `nonexistent` prevents any changes to `file`.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["chmod", "x", "nonexistent", "file"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No such path at 'nonexistent'.
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    file: Conflicted([Some(File { id: FileId("587be6b4c3f93f93c489c0111bba5596147a26cb"), executable: false }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: false }), Some(File { id: FileId("8ba3a16384aacc37d01564b28401755ce8053f51"), executable: false })])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "file"]);
    insta::assert_snapshot!(stdout, 
    @r###"
    <<<<<<<
    %%%%%%%
    -base
    +x
    +++++++
    n
    >>>>>>>
    "###);
}

// TODO: Test demonstrating that conflicts whose *base* is not a file are
// chmod-dable

#[test]
fn test_chmod_file_dir_deletion_conflicts() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "file", &["base"], &[("file", "a\n")]);

    create_commit(&test_env, &repo_path, "deletion", &["base"], &[]);
    std::fs::remove_file(repo_path.join("file")).unwrap();

    create_commit(&test_env, &repo_path, "dir", &["base"], &[]);
    std::fs::remove_file(repo_path.join("file")).unwrap();
    std::fs::create_dir(repo_path.join("file")).unwrap();
    // Without a placeholder file, `jj` ignores an empty directory
    std::fs::write(repo_path.join("file").join("placeholder"), "").unwrap();

    // Create a file-dir conflict and a file-deletion conflict
    create_commit(&test_env, &repo_path, "file_dir", &["file", "dir"], &[]);
    create_commit(
        &test_env,
        &repo_path,
        "file_deletion",
        &["file", "deletion"],
        &[],
    );
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    file_deletion
    ├─╮
    │ ◉  deletion
    │ │ ◉  file_dir
    ╭───┤
    │ │ ◉  dir
    │ ├─╯
    ◉ │  file
    ├─╯
    ◉  base
    ◉
    "###);

    // The file-dir conflict cannot be chmod-ed
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree", "-r=file_dir"]);
    insta::assert_snapshot!(stdout,
    @r###"
    file: Conflicted([Some(File { id: FileId("78981922613b2afb6025042ff6bd878ac1994e85"), executable: false }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: false }), Some(Tree(TreeId("133bb38fc4e4bf6b551f1f04db7e48f04cac2877")))])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "-r=file_dir", "file"]);
    insta::assert_snapshot!(stdout,
    @r###"
    Conflict:
      Removing file with id df967b96a579e45a18b8251732d16804b2e56a55
      Adding file with id 78981922613b2afb6025042ff6bd878ac1994e85
      Adding tree with id 133bb38fc4e4bf6b551f1f04db7e48f04cac2877
    "###);
    let stderr = test_env.jj_cmd_failure(&repo_path, &["chmod", "x", "file", "-r=file_dir"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Some of the sides of the conflict are not files at 'file'.
    "###);

    // The file_deletion conflict can be chmod-ed
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree", "-r=file_deletion"]);
    insta::assert_snapshot!(stdout,
    @r###"
    file: Conflicted([Some(File { id: FileId("78981922613b2afb6025042ff6bd878ac1994e85"), executable: false }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: false }), None])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "-r=file_deletion", "file"]);
    insta::assert_snapshot!(stdout,
    @r###"
    <<<<<<<
    +++++++
    a
    %%%%%%%
    -base
    >>>>>>>
    "###);
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["chmod", "x", "file", "-r=file_deletion"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    New conflicts appeared in these commits:
      kmkuslsw b4c38719 file_deletion | (conflict) file_deletion
    To resolve the conflicts, start by updating to it:
      jj new kmkuslswpqwq
    Then use `jj resolve`, or edit the conflict markers in the file directly.
    Once the conflicts are resolved, you may want inspect the result with `jj diff`.
    Then run `jj squash` to move the resolution into the conflicted commit.
    Working copy now at: kmkuslsw b4c38719 file_deletion | (conflict) file_deletion
    Parent commit      : zsuskuln c51c9c55 file | file
    Parent commit      : royxmykx 6b18b3c1 deletion | deletion
    Added 0 files, modified 1 files, removed 0 files
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["debug", "tree", "-r=file_deletion"]);
    insta::assert_snapshot!(stdout,
    @r###"
    file: Conflicted([Some(File { id: FileId("78981922613b2afb6025042ff6bd878ac1994e85"), executable: true }), Some(File { id: FileId("df967b96a579e45a18b8251732d16804b2e56a55"), executable: true }), None])
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["cat", "-r=file_deletion", "file"]);
    insta::assert_snapshot!(stdout,
    @r###"
    <<<<<<<
    +++++++
    a
    %%%%%%%
    -base
    >>>>>>>
    "###);
}
