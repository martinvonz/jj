// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use insta::assert_snapshot;

use crate::common::TestEnvironment;

#[test]
fn test_deprecated_flags() {
    let test_env = TestEnvironment::default();
    let (stdout, stderr) =
        test_env.jj_cmd_ok(test_env.env_root(), &["util", "completion", "--bash"]);
    assert_snapshot!(
        stderr,
        @r###"
    Warning: `jj util completion --bash` will be removed in a future version, and this will be a hard error
    Hint: Use `jj util completion bash` instead
    "###
    );
    assert!(stdout.contains("COMPREPLY"));
}
