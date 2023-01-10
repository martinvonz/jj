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
    Duplicated 2443ea76b0b1 as 2f6dc5a1ffc2 a
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
    Duplicated 17a00fc21654 as 1dd099ea963c c
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
    Duplicated 1394f625cbbd as fdaaf3950f07 b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as 870cf438ccbb b
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

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s", "a", "-d", "a-"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 4 commits
    Working copy now at: 29bd36b60e60 b
    "###);
    // One of the duplicate commit's timestamps was changed a little to make it have
    // a different commit id from the other.
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    o b43fe7354758   b @ 2001-02-03 04:05:14.000 +07:00
    | o 08beb14c3ead   b @ 2001-02-03 04:05:15.000 +07:00
    |/  
    | @ 29bd36b60e60   b @ 2001-02-03 04:05:16.000 +07:00
    |/  
    o 2f6dc5a1ffc2   a @ 2001-02-03 04:05:16.000 +07:00
    o 000000000000   (no description set) @ 1970-01-01 00:00:00.000 +00:00
    "###);
}

#[test]
fn test_duplicate_many() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["b", "d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   921dde6e55c0   e
    |\  
    o | ebd06dba20ec   d
    o | c0cb3a0b73e7   c
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);

    // BUG! Copy of e shouldn't have b as a parent!
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b:"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as 3b74d9691015 b
    Duplicated 921dde6e55c0 as 8348ddcec733 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   8348ddcec733   e
    |\  
    o | 3b74d9691015   b
    | | @   921dde6e55c0   e
    | | |\  
    | |/ /  
    | o | ebd06dba20ec   d
    | o | c0cb3a0b73e7   c
    |/ /  
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b:", "d"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 1394f625cbbd as 0276d3d7c24d b
    Duplicated ebd06dba20ec as 9dbeec2f035d d
    Duplicated 921dde6e55c0 as 137eb3f539b4 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   137eb3f539b4   e
    |\  
    o | 9dbeec2f035d   d
    | o 0276d3d7c24d   b
    | | o   8348ddcec733   e
    | | |\  
    | | o | 3b74d9691015   b
    | |/ /  
    | | | @   921dde6e55c0   e
    | | | |\  
    | | |/ /  
    | | o | ebd06dba20ec   d
    | |/ /  
    |/| |   
    o | | c0cb3a0b73e7   c
    |/ /  
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "d:", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated ebd06dba20ec as 2181781b4f81 d
    Duplicated 8348ddcec733 as ac92c3a814fb e
    Duplicated 2443ea76b0b1 as 2f1b1790f312 a
    Duplicated 921dde6e55c0 as cdafcbccc31d e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   cdafcbccc31d   e
    |\  
    | | o 2f1b1790f312   a
    | | | o   ac92c3a814fb   e
    | | | |\  
    | |_|/ /  
    |/| | |   
    o | | | 2181781b4f81   d
    | | | | o   137eb3f539b4   e
    | | | | |\  
    | | | | o | 9dbeec2f035d   d
    | |_|_|/ /  
    |/| | | |   
    | | | | o 0276d3d7c24d   b
    | | | | | o   8348ddcec733   e
    | | | | | |\  
    | | | | |/ /  
    | | | |/| |   
    | | | o | | 3b74d9691015   b
    | | | |/ /  
    | | | | | @   921dde6e55c0   e
    | | | | | |\  
    | | | | |/ /  
    | | |_|_|/    
    | |/| | |     
    | | | | o ebd06dba20ec   d
    | |_|_|/  
    |/| | |   
    o | | | c0cb3a0b73e7   c
    | |_|/  
    |/| |   
    | o | 1394f625cbbd   b
    |/ /  
    o | 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);

    // Check for BUG -- makes too many 'a'-s, etc.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a:"]);
    insta::assert_snapshot!(stdout, @r###"
    Duplicated 2443ea76b0b1 as d302e05a88f2 a
    Duplicated 0276d3d7c24d as e3fd7a9b47f6 b
    Duplicated 1394f625cbbd as 220d10caa731 b
    Duplicated c0cb3a0b73e7 as 5175f03cfbc1 c
    Duplicated 2181781b4f81 as cd57ff69eef8 d
    Duplicated cdafcbccc31d as 31e58f23c012 e
    Duplicated 3b74d9691015 as 95b95c578ce8 b
    Duplicated ac92c3a814fb as 9cb2414b737d e
    Duplicated ebd06dba20ec as c7f34c9ca244 d
    Duplicated 8348ddcec733 as cdde834a642d e
    Duplicated 9dbeec2f035d as a36032e15fd8 d
    Duplicated 137eb3f539b4 as e6ecf9b920ea e
    Duplicated 921dde6e55c0 as 7d280d1abb23 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   7d280d1abb23   e
    |\  
    | | o   e6ecf9b920ea   e
    | | |\  
    | | o | a36032e15fd8   d
    | | | | o   cdde834a642d   e
    | | | | |\  
    | |_|_|/ /  
    |/| | | |   
    o | | | | c7f34c9ca244   d
    | |/ / /  
    |/| | |   
    | | | | o   9cb2414b737d   e
    | | | | |\  
    | | | |/ /  
    | | | o | 95b95c578ce8   b
    | | | | | o   31e58f23c012   e
    | | | | | |\  
    | | | | |/ /  
    | | |_|_|/    
    | |/| | |     
    | | | | o cd57ff69eef8   d
    | |_|_|/  
    |/| | |   
    o | | | 5175f03cfbc1   c
    | |_|/  
    |/| |   
    | o | 220d10caa731   b
    |/ /  
    | o e3fd7a9b47f6   b
    |/  
    o d302e05a88f2   a
    | o   cdafcbccc31d   e
    | |\  
    | | | o 2f1b1790f312   a
    | |_|/  
    |/| |   
    | | | o   ac92c3a814fb   e
    | | | |\  
    | | |/ /  
    | |/| |   
    | o | | 2181781b4f81   d
    | | | | o   137eb3f539b4   e
    | | | | |\  
    | | | | o | 9dbeec2f035d   d
    | | |_|/ /  
    | |/| | |   
    | | | | o 0276d3d7c24d   b
    | | | | | o   8348ddcec733   e
    | | | | | |\  
    | | | | |/ /  
    | | | |/| |   
    | | | o | | 3b74d9691015   b
    | | | |/ /  
    | | | | | @   921dde6e55c0   e
    | | | | | |\  
    | | | | |/ /  
    | | | |_|/    
    | | |/| |     
    | | | | o ebd06dba20ec   d
    | | |_|/  
    | |/| |   
    | o | | c0cb3a0b73e7   c
    | | |/  
    | |/|   
    | | o 1394f625cbbd   b
    | |/  
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
