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

    /* BUG!!! Panics instead of failing! */
    // let stderr = test_env.jj_cmd_failure(&repo_path, &["duplicate", "root"]);
    // insta::assert_snapshot!(stderr, @r###"
    // "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Created: 3d341b2f2b09 a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o 3d341b2f2b09   a
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
    Created: 2426bb15bfd6 c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   2426bb15bfd6   c
    |\  
    | | o 3d341b2f2b09   a
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
