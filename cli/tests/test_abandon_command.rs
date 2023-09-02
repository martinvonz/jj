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
        test_env.jj_cmd_success(repo_path, &["new", "root()", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

#[test]
fn test_rebase_branch_with_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &[]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["a", "d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  d
    │ ◉  c
    │ │ ◉  b
    ├───╯
    ◉ │  a
    ├─╯
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "d"]);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned commit vruxwmqv b7c62f28 d | d
    Rebased 1 descendant commits onto parents of abandoned commits
    Working copy now at: znkkpsqq 11a2e10e e | e
    Parent commit      : rlvkpnrz 2443ea76 a | a
    Parent commit      : royxmykx fe2e8e8b c d | c
    Added 0 files, modified 0 files, removed 1 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  c d
    │ │ ◉  b
    ├───╯
    ◉ │  a
    ├─╯
    ◉
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon"] /* abandons `e` */);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned commit znkkpsqq 5557ece3 e | e
    Working copy now at: nkmrtpmo 6b527513 (empty) (no description set)
    Parent commit      : rlvkpnrz 2443ea76 a e?? | a
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @
    │ ◉  b
    ├─╯
    ◉  a e??
    │ ◉  d e??
    │ ◉  c
    ├─╯
    ◉
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "descendants(c)"]);
    // TODO(ilyagr): Minor Bug: The branch `e` should be shown next
    // to the commit with description `e` below. This is because the commits are
    // printed in the state *after* abandonment. This will be fixed together with
    // adding (hidden) to the commit template, which causes a more obvious version
    // of the same problem.
    insta::assert_snapshot!(stdout, @r###"
    Abandoned the following commits:
      znkkpsqq 5557ece3 e | e
      vruxwmqv b7c62f28 d | d
      royxmykx fe2e8e8b c | c
    Working copy now at: xtnwkqum e7bb0612 (empty) (no description set)
    Parent commit      : rlvkpnrz 2443ea76 a e?? | a
    Added 0 files, modified 0 files, removed 3 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @
    │ ◉  b
    ├─╯
    ◉  a e??
    ◉  c d e??
    "###);

    // Test abandoning the same commit twice directly
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "b", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned commit zsuskuln 1394f625 b | b
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @    e
    ├─╮
    │ ◉  d
    │ ◉  c
    ◉ │  a b
    ├─╯
    ◉
    "###);

    // Test abandoning the same commit twice indirectly
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["abandon", "d::", "a::"]);
    insta::assert_snapshot!(stdout, @r###"
    Abandoned the following commits:
      znkkpsqq 5557ece3 e | e
      vruxwmqv b7c62f28 d | d
      zsuskuln 1394f625 b | b
      rlvkpnrz 2443ea76 a | a
    Working copy now at: xlzxqlsl af874bff (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 a b e?? | (empty) (no description set)
    Added 0 files, modified 0 files, removed 4 files
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @
    │ ◉  c d e??
    ├─╯
    ◉  a b e??
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["abandon", "root()"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rewrite commit 000000000000
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}
