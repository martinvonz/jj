// Copyright 2020 The Jujutsu Authors
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
fn test_gitsubmodule_print_gitmodules() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);

    std::fs::write(
        workspace_root.join(".gitmodules"),
        "
[submodule \"old\"]
	path = old
	url = https://github.com/old/old.git
",
    )
    .unwrap();

    test_env.jj_cmd_ok(&workspace_root, &["new"]);

    std::fs::write(
        workspace_root.join(".gitmodules"),
        "
[submodule \"new\"]
	path = new
	url = https://github.com/new/new.git
",
    )
    .unwrap();

    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["git", "submodule", "print-gitmodules", "-r", "@-"],
    );
    insta::assert_snapshot!(stdout, @r###"
    name:old
    url:https://github.com/old/old.git
    path:old


    "###);

    let stdout =
        test_env.jj_cmd_success(&workspace_root, &["git", "submodule", "print-gitmodules"]);
    insta::assert_snapshot!(stdout, @r###"
	name:new
	url:https://github.com/new/new.git
	path:new
    "###);
}
