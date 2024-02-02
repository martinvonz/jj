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

use test_case::test_case;
use testutils::{TestRepoBackend, TestWorkspace};

use crate::common::TestEnvironment;

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_root(backend: TestRepoBackend) {
    let test_env = TestEnvironment::default();
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init_with_backend(&settings, backend);
    let root = test_workspace.workspace.workspace_root();
    let subdir = root.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let stdout = test_env.jj_cmd_success(&subdir, &["root"]);
    assert_eq!(&stdout, &[root.to_str().unwrap(), "\n"].concat());
}

#[test]
fn test_root_outside_a_repo() {
    let test_env = TestEnvironment::default();
    let stdout = test_env.jj_cmd_failure(Path::new("/"), &["root"]);
    insta::assert_snapshot!(stdout, @r###"
    Error: There is no jj repo in "."
    "###);
}
