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

use std::path::Path;

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_split_by_paths() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    std::fs::write(repo_path.join("file2"), "foo").unwrap();
    std::fs::write(repo_path.join("file3"), "foo").unwrap();

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  qpvuntsmwlqt false
    ◉  zzzzzzzzzzzz true
    "###);

    let edit_script = test_env.set_up_fake_editor();
    std::fs::write(
        edit_script,
        ["dump editor0", "next invocation\n", "dump editor1"].join("\0"),
    )
    .unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["split", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    First part: qpvuntsm 5eebce1d (no description set)
    Second part: kkmpptxz 45833353 (no description set)
    Working copy now at: kkmpptxz 45833353 (no description set)
    Parent commit      : qpvuntsm 5eebce1d (no description set)
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

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  kkmpptxzrspx false
    ◉  qpvuntsmwlqt false
    ◉  zzzzzzzzzzzz true
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
    First part: qpvuntsm 31425b56 (no description set)
    Second part: yqosqzyt af096392 (empty) (no description set)
    Working copy now at: kkmpptxz 28d4ec20 (no description set)
    Parent commit      : yqosqzyt af096392 (empty) (no description set)
    "###);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  kkmpptxzrspx false
    ◉  yqosqzytrlsw true
    ◉  qpvuntsmwlqt false
    ◉  zzzzzzzzzzzz true
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@--"]);
    insta::assert_snapshot!(stdout, @r###"
    A file2
    "###);

    // Remove newly created empty commit
    test_env.jj_cmd_success(&repo_path, &["abandon", "@-"]);

    // Insert an empty commit before @- with "split nonexistent"
    test_env.set_up_fake_editor();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["split", "-r", "@-", "nonexistent"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 1 descendant commits
    First part: qpvuntsm 0647b2cb (empty) (no description set)
    Second part: kpqxywon d5d77af6 (no description set)
    Working copy now at: kkmpptxz 86f228dc (no description set)
    Parent commit      : kpqxywon d5d77af6 (no description set)
    "###);
    insta::assert_snapshot!(stderr, @r###"
    The given paths do not match any file: nonexistent
    "###);

    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  kkmpptxzrspx false
    ◉  kpqxywonksrl false
    ◉  qpvuntsmwlqt true
    ◉  zzzzzzzzzzzz true
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@-"]);
    insta::assert_snapshot!(stdout, @r###"
    A file2
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"change_id.short() ++ " " ++ empty"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
