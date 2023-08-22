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
fn test_git_remotes() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["git", "remote", "add", "foo", "http://example.com/repo/foo"],
    );
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["git", "remote", "add", "bar", "http://example.com/repo/bar"],
    );
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    bar http://example.com/repo/bar
    foo http://example.com/repo/foo
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "remove", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @"bar http://example.com/repo/bar
");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "remove", "nonexistent"]);
    insta::assert_snapshot!(stderr, @"Error: Remote doesn't exist
");
}

#[test]
fn test_git_remote_rename() {
    let test_env = TestEnvironment::default();

    test_env.jj_cmd_success(test_env.env_root(), &["init", "--git", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_success(
        &repo_path,
        &["git", "remote", "add", "foo", "http://example.com/repo/foo"],
    );
    let stderr = test_env.jj_cmd_failure(&repo_path, &["git", "remote", "rename", "bar", "foo"]);
    insta::assert_snapshot!(stderr, @"Error: Remote doesn't exist\n");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "rename", "foo", "bar"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["git", "remote", "list"]);
    insta::assert_snapshot!(stdout, @"bar http://example.com/repo/foo");
}
