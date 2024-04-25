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

use crate::common::TestEnvironment;

#[test]
fn test_tag_list() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let git_repo = {
        let mut git_repo_path = repo_path.clone();
        git_repo_path.extend([".jj", "repo", "store", "git"]);
        git2::Repository::open(git_repo_path).unwrap()
    };

    let copy_ref = |src_name: &str, dest_name: &str| {
        let src = git_repo.find_reference(src_name).unwrap();
        let oid = src.target().unwrap();
        git_repo.reference(dest_name, oid, true, "").unwrap();
    };

    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-mcommit1"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "branch1"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-mcommit2"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "branch2"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);

    copy_ref("refs/heads/branch1", "refs/tags/test_tag");
    copy_ref("refs/heads/branch2", "refs/tags/test_tag2");
    test_env.jj_cmd_ok(&repo_path, &["git", "import"]);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list"]),
        @r###"
        test_tag
        test_tag2
         "###);

    // Test pattern matching.
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "test_tag2"]),
        @r###"
        test_tag2
         "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "glob:test_tag?"]),
        @r###"
        test_tag2
         "###);

    let template = r#"'name: ' ++ name ++ "\n""#;
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "-T", template]),
        @r###"
    name: test_tag
    name: test_tag2
    "###);
}
