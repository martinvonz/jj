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

fn set_up_tagged_git_repo(git_repo: &git2::Repository) {
    let signature =
        git2::Signature::new("Some One", "some.one@example.com", &git2::Time::new(0, 0)).unwrap();
    let mut tree_builder = git_repo.treebuilder(None).unwrap();
    let file_oid = git_repo.blob(b"content").unwrap();
    tree_builder
        .insert("file", file_oid, git2::FileMode::Blob.into())
        .unwrap();
    let tree_oid = tree_builder.write().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    git_repo
        .commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "message",
            &tree,
            &[],
        )
        .unwrap();
    git_repo.set_head("refs/heads/main").unwrap();

    let obj = git_repo.revparse_single("HEAD").unwrap();
    git_repo
        .tag("test_tag", &obj, &signature, "test tag message", false)
        .unwrap();
    git_repo
        .tag("test_tag2", &obj, &signature, "test tag message", false)
        .unwrap();
}

#[test]
fn test_tag_list() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-branch = true");
    let git_repo_path = test_env.env_root().join("source");
    let git_repo = git2::Repository::init(git_repo_path).unwrap();

    set_up_tagged_git_repo(&git_repo);

    test_env.jj_cmd_ok(test_env.env_root(), &["git", "clone", "source", "tagged"]);

    let local_path = test_env.env_root().join("tagged");
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&local_path, &["tag", "list"]),
        @r###"
        test_tag
        test_tag2
         "###);

    // Test pattern matching.
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&local_path, &["tag", "list", "test_tag2"]),
        @r###"
        test_tag2
         "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&local_path, &["tag", "list", "glob:test_tag?"]),
        @r###"
        test_tag2
         "###);
}
