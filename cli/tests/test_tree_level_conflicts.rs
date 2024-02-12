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
fn test_enable_tree_level_conflicts() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"format.tree-level-conflicts = false"#);
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    // Create a few commits before we enable tree-level conflicts
    let file_path = repo_path.join("file");
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=left"]);
    std::fs::write(&file_path, "left").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=right"]);
    std::fs::write(&file_path, "right").unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "new",
            r#"description("left")"#,
            r#"description("right")"#,
            "-m=merge",
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  mzvwutvl test.user@example.com 2001-02-03 08:05:11 f2101bed conflict
    │  (empty) (no description set)
    ◉    zsuskuln test.user@example.com 2001-02-03 08:05:10 5100e4e1 conflict
    ├─╮  (empty) merge
    │ ◉  kkmpptxz test.user@example.com 2001-02-03 08:05:10 0b65c8fb
    │ │  right
    ◉ │  rlvkpnrz test.user@example.com 2001-02-03 08:05:09 32003b88
    ├─╯  left
    ◉  zzzzzzzz root() 00000000
    "###);

    // Enable tree-level conflicts
    test_env.add_config(r#"format.tree-level-conflicts = true"#);
    // We get a new working-copy commit. The working copy unfortunately appears
    // non-empty
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  mzvwutvl test.user@example.com 2001-02-03 08:05:13 51f1748d conflict
    │  (no description set)
    ◉    zsuskuln test.user@example.com 2001-02-03 08:05:10 5100e4e1 conflict
    ├─╮  (empty) merge
    │ ◉  kkmpptxz test.user@example.com 2001-02-03 08:05:10 0b65c8fb
    │ │  right
    ◉ │  rlvkpnrz test.user@example.com 2001-02-03 08:05:09 32003b88
    ├─╯  left
    ◉  zzzzzzzz root() 00000000
    "###);
    // ...but at least it has no diff
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @"");

    // If we create new commit off of an unconflicted commit, it correctly appears
    // empty
    test_env.jj_cmd_ok(&repo_path, &["new", "k"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r=@"]);
    insta::assert_snapshot!(stdout, @r###"
    @  yostqsxw test.user@example.com 2001-02-03 08:05:15 112f0ac2
    │  (empty) (no description set)
    ~
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @"");
}
