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

pub mod common;

#[test]
fn test_undo_rewrite_with_child() {
    // Test that if we undo an operation that rewrote some commit, any descendants
    // after that will be rebased on top of the un-rewritten commit.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "modified"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    let op_id_hex = stdout[3..15].to_string();
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "child"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  child
    ◉  modified
    ◉
    "###);
    test_env.jj_cmd_success(&repo_path, &["undo", &op_id_hex]);

    // Since we undid the description-change, the child commit should now be on top
    // of the initial commit
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  child
    ◉  initial
    ◉
    "###);
}
