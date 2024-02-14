// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

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
