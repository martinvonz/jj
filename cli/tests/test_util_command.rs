// Copyright 2023 The Jujutsu Authors
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

use insta::assert_snapshot;

use crate::common::TestEnvironment;

#[test]
fn test_util_config_schema() {
    let test_env = TestEnvironment::default();
    let stdout = test_env.jj_cmd_success(test_env.env_root(), &["util", "config-schema"]);
    // Validate partial snapshot, redacting any lines nested 2+ indent levels.
    insta::with_settings!({filters => vec![(r"(?m)(^        .*$\r?\n)+", "        [...]\n")]}, {
        assert_snapshot!(stdout, @r###"
        {
            "$schema": "http://json-schema.org/draft-07/schema",
            "title": "Jujutsu config",
            "type": "object",
            "description": "User configuration for Jujutsu VCS. See https://github.com/martinvonz/jj/blob/main/docs/config.md for details",
            "properties": {
                [...]
            }
        }
        "###)
    });
}

#[test]
fn test_gc_args() {
    let test_env = TestEnvironment::default();
    // Use the local backend because GitBackend::gc() depends on the git CLI.
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["init", "repo", "--config-toml=ui.allow-init-native=true"],
    );
    let repo_path = test_env.env_root().join("repo");

    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["util", "gc"]);
    insta::assert_snapshot!(stderr, @"");

    let stderr = test_env.jj_cmd_failure(&repo_path, &["util", "gc", "--at-op=@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot garbage collect from a non-head operation
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["util", "gc", "--expire=foobar"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: --expire only accepts 'now'
    "###);
}

#[test]
fn test_gc_operation_log() {
    let test_env = TestEnvironment::default();
    // Use the local backend because GitBackend::gc() depends on the git CLI.
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &["init", "repo", "--config-toml=ui.allow-init-native=true"],
    );
    let repo_path = test_env.env_root().join("repo");

    // Create an operation.
    std::fs::write(repo_path.join("file"), "a change\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "a change"]);
    let op_to_remove = test_env.current_operation_id(&repo_path);

    // Make another operation the head.
    std::fs::write(repo_path.join("file"), "another change\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "another change"]);

    // This works before the operation is removed.
    test_env.jj_cmd_ok(&repo_path, &["debug", "operation", &op_to_remove]);

    // Remove some operations.
    test_env.jj_cmd_ok(&repo_path, &["operation", "abandon", "..@-"]);
    test_env.jj_cmd_ok(&repo_path, &["util", "gc", "--expire=now"]);

    // Now this doesn't work.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["debug", "operation", &op_to_remove]);
    insta::assert_snapshot!(stderr, @r###"
    Error: No operation ID matching "35688918195690874cbf1f282140cda33c882e48a84dbb0f92c262b52ace4a5753777432b18e9de01bc23121b23261eb2c828622836b9ec7ded7c0ca3c7c1670"
    "###);
}
