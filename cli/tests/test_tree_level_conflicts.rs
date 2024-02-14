// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

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
    @  mzvwutvl test.user@example.com 2001-02-03 04:05:11.000 +07:00 f2101bed conflict
    │  (empty) (no description set)
    ◉    zsuskuln test.user@example.com 2001-02-03 04:05:10.000 +07:00 5100e4e1 conflict
    ├─╮  (empty) merge
    │ ◉  kkmpptxz test.user@example.com 2001-02-03 04:05:10.000 +07:00 0b65c8fb
    │ │  right
    ◉ │  rlvkpnrz test.user@example.com 2001-02-03 04:05:09.000 +07:00 32003b88
    ├─╯  left
    ◉  zzzzzzzz root() 00000000
    "###);

    // Enable tree-level conflicts
    test_env.add_config(r#"format.tree-level-conflicts = true"#);
    // We get a new working-copy commit. The working copy unfortunately appears
    // non-empty
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  mzvwutvl test.user@example.com 2001-02-03 04:05:13.000 +07:00 54c562fa conflict
    │  (no description set)
    ◉    zsuskuln test.user@example.com 2001-02-03 04:05:10.000 +07:00 5100e4e1 conflict
    ├─╮  (empty) merge
    │ ◉  kkmpptxz test.user@example.com 2001-02-03 04:05:10.000 +07:00 0b65c8fb
    │ │  right
    ◉ │  rlvkpnrz test.user@example.com 2001-02-03 04:05:09.000 +07:00 32003b88
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
    @  yostqsxw test.user@example.com 2001-02-03 04:05:15.000 +07:00 112f0ac2
    │  (empty) (no description set)
    ~
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @"");
}
