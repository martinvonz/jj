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

pub mod common;

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

#[test]
fn test_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   17a00fc21654   c
    |\  
    o | d370aee184ba   b
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["duplicate", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rewrite the root commit
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Created: 2f6dc5a1ffc2 a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o 2f6dc5a1ffc2   a
    | @   17a00fc21654   c
    | |\  
    | o | d370aee184ba   b
    |/ /  
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate" /* duplicates `c` */]);
    insta::assert_snapshot!(stdout, @r###"
    Created: 1dd099ea963c c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   1dd099ea963c   c
    |\  
    | | o 2f6dc5a1ffc2   a
    | | | @   17a00fc21654   c
    | | | |\  
    | |_|/ /  
    |/| | |   
    | | |/    
    | |/|     
    o | | d370aee184ba   b
    | |/  
    |/|   
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);
}

// https://github.com/martinvonz/jj/issues/694
#[test]
fn test_rebase_duplicates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    @ 1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    o 2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    o 000000000000   (no description set) @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Created: fdaaf3950f07 b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Created: 870cf438ccbb b
    "###);
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    o 870cf438ccbb   b @ 2001-02-03 04:05:14.000 +07:00
    | o fdaaf3950f07   b @ 2001-02-03 04:05:13.000 +07:00
    |/  
    | @ 1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    |/  
    o 2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    o 000000000000   (no description set) @ 1970-01-01 00:00:00.000 +00:00
    "###);

    // This is the bug: this should succeed
    let stderr = test_env.jj_cmd_failure(&repo_path, &["rebase", "-s", "a", "-d", "a-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Unexpected error from backend: Error: Git commit '29bd36b60e6002f04e03c5077f989c93e3c910e1' already exists with different associated non-Git meta-data
    "###);
}
fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "-T",
            r#"commit_id.short() "   " description.first_line()"#,
        ],
    )
}

// The timestamp is relevant for the bugfix
fn get_log_output_with_ts(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "-T",
            r#"commit_id.short() "   " description.first_line() " @ " committer.timestamp()"#,
        ],
    )
}
