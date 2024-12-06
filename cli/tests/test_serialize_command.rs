// Copyright 2024 The Jujutsu Authors
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

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_ok(repo_path, &["new", "root()", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_ok(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_ok(repo_path, &["bookmark", "create", name]);
}

#[test]
fn test_serialize() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");


    create_commit(&test_env, &repo_path, "start", &[]);
    create_commit(&test_env, &repo_path, "a1", &["start"]);
    create_commit(&test_env, &repo_path, "a2", &["a1"]);
    create_commit(&test_env, &repo_path, "a3", &["a2"]);
    create_commit(&test_env, &repo_path, "a4", &["a3"]);
    create_commit(&test_env, &repo_path, "b1", &["start"]);
    create_commit(&test_env, &repo_path, "b2", &["b1"]);
    create_commit(&test_env, &repo_path, "b3", &["b1"]);
    create_commit(&test_env, &repo_path, "final", &["start"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  final: start
    │ ○  b3: b1
    │ │ ○  b2: b1
    │ ├─╯
    │ ○  b1: start
    ├─╯
    │ ○  a4: a3
    │ ○  a3: a2
    │ ○  a2: a1
    │ ○  a1: start
    ├─╯
    ○  start
    ◆
    ");
    let setup_opid = test_env.current_operation_id(&repo_path);

    // Re-order A-branch.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["serialize", "a3", "a2", "a4", "a1", "-d", "start"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"Rebased 3 descendant commits");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  final: start
    │ ○  a1: a4
    │ ○  a4: a2
    │ ○  a2: a3
    │ ○  a3: start
    ├─╯
    │ ○  b3: b1
    │ │ ○  b2: b1
    │ ├─╯
    │ ○  b1: start
    ├─╯
    ○  start
    ◆
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Serialize everything in messy order, after.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["serialize", "b1", "a1::", "b1+", "-A", "start"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Working copy now at: xznxytkn 927f4b68 final | final
    Parent commit      : nkmrtpmo bca1bcd7 b3 | b3
    Added 7 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  final: b3
    ○  b3: b2
    ○  b2: a4
    ○  a4: a3
    ○  a3: a2
    ○  a2: a1
    ○  a1: b1
    ○  b1: start
    ○  start
    ◆
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // Serialize everything in messy order, before.
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["serialize", "b1", "a1::", "b1+", "-B", "final"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r"
    Working copy now at: xznxytkn 39cf9c51 final | final
    Parent commit      : nkmrtpmo ed5a1dd9 b3 | b3
    Added 7 files, modified 0 files, removed 0 files
    ");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    @  final: b3
    ○  b3: b2
    ○  b2: a4
    ○  a4: a3
    ○  a3: a2
    ○  a2: a1
    ○  a1: b1
    ○  b1: start
    ○  start
    ◆
    ");
    test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);

    // // Serialize in-between.
    // let (stdout, stderr) = test_env.jj_cmd_ok(
    //     &repo_path,
    //     &["serialize", "b1", "a3", "-A", "all:b1+", "-B", "final"],
    // );
    // insta::assert_snapshot!(stdout, @"");
    // insta::assert_snapshot!(stderr, @r"
    // Working copy now at: xznxytkn 927f4b68 final | final
    // Parent commit      : nkmrtpmo bca1bcd7 b3 | b3
    // Added 7 files, modified 0 files, removed 0 files
    // ");
    // insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r"
    // @  final: b3
    // ○  b3: b2
    // ○  b2: a4
    // ○  a4: a3
    // ○  a3: a2
    // ○  a2: a1
    // ○  a1: b1
    // ○  b1: start
    // ○  start
    // ◆
    // ");
    // test_env.jj_cmd_ok(&repo_path, &["op", "restore", &setup_opid]);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = "bookmarks ++ surround(': ', '', parents.map(|c| c.bookmarks()))";
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}

fn get_long_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = "bookmarks ++ '  ' ++ change_id.shortest(8) ++ '  ' ++ commit_id.shortest(8) \
                    ++ surround(':  ', '', parents.map(|c| c.bookmarks()))";
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}
