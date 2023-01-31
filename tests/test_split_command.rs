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

use std::path::{Path, PathBuf};

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_split_by_paths() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    std::fs::write(repo_path.join("file2"), "foo").unwrap();
    std::fs::write(repo_path.join("file3"), "foo").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "change_id.short()"]);
    insta::assert_snapshot!(stdout, @r###"
    @ 9a45c67d3e96
    o 000000000000
    "###);

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(
        edit_script,
        ["dump editor0", "next invocation\n", "dump editor1"].join("\0"),
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["split", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    First part: 5eebce1de3b0 (no description set)
    Second part: 45833353d94e (no description set)
    Working copy now at: 45833353d94e (no description set)
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor0")).unwrap(), @r###"
    JJ: Enter commit description for the first part (parent).

    JJ: This commit contains the following changes:
    JJ:     A file2

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);
    insta::assert_snapshot!(
        std::fs::read_to_string(test_env.env_root().join("editor1")).unwrap(), @r###"
    JJ: Enter commit description for the second part (child).

    JJ: This commit contains the following changes:
    JJ:     A file1
    JJ:     A file3

    JJ: Lines starting with "JJ: " (like this one) will be removed.
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "change_id.short()"]);
    insta::assert_snapshot!(stdout, @r###"
    @ ffdaa62087a2
    o 9a45c67d3e96
    o 000000000000
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    A file2
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    A file1
    A file3
    "###);

    // Insert an empty commit after @- with "split ."
    test_env.set_up_fake_editor();
    let stdout = test_env.jj_cmd_success(&repo_path, &["split", "-r", "@-", "."]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    First part: 31425b568fcf (no description set)
    Second part: af0963926ac3 (no description set)
    Working copy now at: 28d4ec20efa9 (no description set)
    "###);

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "-T", r#"change_id.short() " " empty"#]);
    insta::assert_snapshot!(stdout, @r###"
    @ ffdaa62087a2 false
    o 19b790168e73 true
    o 9a45c67d3e96 false
    o 000000000000 true
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@--"]);
    insta::assert_snapshot!(stdout, @r###"
    A file2
    "###);

    // Remove newly created empty commit
    test_env.jj_cmd_success(&repo_path, &["abandon", "@-"]);

    // Insert an empty commit before @- with "split nonexistent"
    test_env.set_up_fake_editor();
    let assert = test_env
        .jj_cmd(&repo_path, &["split", "-r", "@-", "nonexistent"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    Rebased 1 descendant commits
    First part: 0647b2cbd0da (no description set)
    Second part: d5d77af65446 (no description set)
    Working copy now at: 86f228dc3a50 (no description set)
    "###);
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    The given paths do not match any file: nonexistent
    "###);

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "-T", r#"change_id.short() " " empty"#]);
    insta::assert_snapshot!(stdout, @r###"
    @ ffdaa62087a2 false
    o fa9213bcf78e false
    o 9a45c67d3e96 true
    o 000000000000 true
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    A file2
    "###);
}

#[test]
fn test_split_empty_child() {
    let (test_env, repo_path) = setup_split_for_empty();
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 0cffa7997ffe second original child
    | o 48523d946ad2 first original child
    |/  
    o   d043564ef936 |target| target_commit
    |\  
    o | 0757f5ec8418 second original parent
    | o 9a45c67d3e96 first original parent
    |/  
    o 000000000000 (empty) 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["split", "-r", "@-", "--empty-child"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 descendant commits
    First part: a993a447ac69 target_commit
    Second part: 84f59c80ebce (no description set)
    Working copy now at: 166713ad11c5 second original child
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 0cffa7997ffe second original child
    | o 48523d946ad2 first original child
    |/  
    o fdf57e73a939 (empty) |target|
    o   d043564ef936 target_commit
    |\  
    o | 0757f5ec8418 second original parent
    | o 9a45c67d3e96 first original parent
    |/  
    o 000000000000 (empty)
    "###);
}

#[test]
fn test_split_empty_parent() {
    let (test_env, repo_path) = setup_split_for_empty();
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 0cffa7997ffe second original child
    | o 48523d946ad2 first original child
    |/  
    o   d043564ef936 |target| target_commit
    |\  
    o | 0757f5ec8418 second original parent
    | o 9a45c67d3e96 first original parent
    |/  
    o 000000000000 (empty) 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["split", "-r", "@-", "--empty-parent"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 2 descendant commits
    First part: dbe80808baf3 (no description set)
    Second part: 8727315222b7 target_commit
    Working copy now at: d2624a76a1e5 second original child
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @ 0cffa7997ffe second original child
    | o 48523d946ad2 first original child
    |/  
    o fdf57e73a939 |target| target_commit
    o   d043564ef936 (empty)
    |\  
    o | 0757f5ec8418 second original parent
    | o 9a45c67d3e96 first original parent
    |/  
    o 000000000000 (empty)
    "###);
}

/// Setup a repo containing a target commit at @- with two parents and two
/// children.
fn setup_split_for_empty() -> (TestEnvironment, PathBuf) {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first original parent"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "fop"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second original parent", "root"]);
    std::fs::write(repo_path.join("file2"), "foo").unwrap();

    test_env.jj_cmd_success(&repo_path, &["new", "-m", "target_commit", "@", "fop"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "forget", "fop"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "create", "target"]);
    std::fs::write(repo_path.join("file3"), "foo").unwrap();

    test_env.jj_cmd_success(&repo_path, &["new", "-m", "first original child"]);
    std::fs::write(repo_path.join("file4"), "foo").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first original child"]);

    test_env.jj_cmd_success(
        &repo_path,
        &["new", "-r", "@-", "-m", "second original child"],
    );
    std::fs::write(repo_path.join("file4"), "foo").unwrap();
    (test_env, repo_path)
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "-T",
            r#"change_id.short() if(empty, " (empty)")
            if(branches, " |" branches "|")
            if(description, " " description)"#,
        ],
    )
}
