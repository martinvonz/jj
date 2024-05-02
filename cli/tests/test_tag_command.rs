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
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-mcommit3"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "branch3"]);
    test_env.jj_cmd_ok(&repo_path, &["git", "export"]);

    copy_ref("refs/heads/branch1", "refs/tags/test_tag");
    copy_ref("refs/heads/branch2", "refs/tags/test_tag2");
    copy_ref("refs/heads/branch1", "refs/tags/conflicted_tag");
    test_env.jj_cmd_ok(&repo_path, &["git", "import"]);
    copy_ref("refs/heads/branch2", "refs/tags/conflicted_tag");
    test_env.jj_cmd_ok(&repo_path, &["git", "import"]);
    copy_ref("refs/heads/branch3", "refs/tags/conflicted_tag");
    test_env.jj_cmd_ok(&repo_path, &["git", "import", "--at-op=@-"]);
    test_env.jj_cmd_ok(&repo_path, &["status"]); // resolve concurrent ops

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list"]),
        @r###"
    conflicted_tag (conflicted):
      - rlvkpnrz caf975d0 (empty) commit1
      + zsuskuln 3db783e0 (empty) commit2
      + royxmykx 68d950ce (empty) commit3
    test_tag: rlvkpnrz caf975d0 (empty) commit1
    test_tag2: zsuskuln 3db783e0 (empty) commit2
    "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "--color=always"]),
        @r###"
    [38;5;5mconflicted_tag[39m [38;5;1m(conflicted)[39m:
      - [1m[38;5;5mrl[0m[38;5;8mvkpnrz[39m [1m[38;5;4mc[0m[38;5;8maf975d0[39m [38;5;2m(empty)[39m commit1
      + [1m[38;5;5mzs[0m[38;5;8muskuln[39m [1m[38;5;4m3[0m[38;5;8mdb783e0[39m [38;5;2m(empty)[39m commit2
      + [1m[38;5;5mr[0m[38;5;8moyxmykx[39m [1m[38;5;4m6[0m[38;5;8m8d950ce[39m [38;5;2m(empty)[39m commit3
    [38;5;5mtest_tag[39m: [1m[38;5;5mrl[0m[38;5;8mvkpnrz[39m [1m[38;5;4mc[0m[38;5;8maf975d0[39m [38;5;2m(empty)[39m commit1
    [38;5;5mtest_tag2[39m: [1m[38;5;5mzs[0m[38;5;8muskuln[39m [1m[38;5;4m3[0m[38;5;8mdb783e0[39m [38;5;2m(empty)[39m commit2
    "###);

    // Test pattern matching.
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "test_tag2"]),
        @r###"
    test_tag2: zsuskuln 3db783e0 (empty) commit2
    "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "glob:test_tag?"]),
        @r###"
    test_tag2: zsuskuln 3db783e0 (empty) commit2
    "###);

    let template = r#"
    concat(
      "[" ++ name ++ "]\n",
      separate(" ", "present:", present) ++ "\n",
      separate(" ", "conflict:", conflict) ++ "\n",
      separate(" ", "normal_target:", normal_target.description().first_line()) ++ "\n",
      separate(" ", "removed_targets:", removed_targets.map(|c| c.description().first_line())) ++ "\n",
      separate(" ", "added_targets:", added_targets.map(|c| c.description().first_line())) ++ "\n",
    )
    "#;
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["tag", "list", "-T", template]),
        @r###"
    [conflicted_tag]
    present: true
    conflict: true
    normal_target: <Error: No Commit available>
    removed_targets: commit1
    added_targets: commit2 commit3
    [test_tag]
    present: true
    conflict: false
    normal_target: commit1
    removed_targets:
    added_targets: commit1
    [test_tag2]
    present: true
    conflict: false
    normal_target: commit2
    removed_targets:
    added_targets: commit2
    "###);
}
