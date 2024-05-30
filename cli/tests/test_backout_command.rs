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
    test_env.jj_cmd_ok(repo_path, &["branch", "create", name]);
}

#[test]
fn test_backout() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  2443ea76b0b1 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    A a
    "###);

    // Backout the commit
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["backout", "-r", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  6d845ed9fb6a Back out "a"
    │
    │  This backs out commit 2443ea76b0b1c531326908326aab7020abab8e6c.
    @  2443ea76b0b1 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@+"]);
    insta::assert_snapshot!(stdout, @r###"
    D a
    "###);

    // Backout the new backed-out commit
    test_env.jj_cmd_ok(&repo_path, &["edit", "@+"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["backout", "-r", "@"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    ◉  79555ea9040b Back out "Back out "a""
    │
    │  This backs out commit 6d845ed9fb6a3d367e2d7068ef0256b1a10705a9.
    @  6d845ed9fb6a Back out "a"
    │
    │  This backs out commit 2443ea76b0b1c531326908326aab7020abab8e6c.
    ◉  2443ea76b0b1 a
    ◉  000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "-r", "@+"]);
    insta::assert_snapshot!(stdout, @r###"
    A a
    "###);
}

fn get_log_output(test_env: &TestEnvironment, cwd: &Path) -> String {
    let template = r#"commit_id.short() ++ " " ++ description"#;
    test_env.jj_cmd_success(cwd, &["log", "-T", template])
}
