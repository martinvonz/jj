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

use std::path::Path;

use itertools::Itertools;
use regex::Regex;

use crate::common::{get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_op_log() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 0"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "op",
            "log",
            "--config-toml",
            "template-aliases.'format_time_range(x)' = 'x'",
        ],
    );
    insta::assert_snapshot!(&stdout, @r###"
    @  98f7262e4a06 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    ◉  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
    let op_log_lines = stdout.lines().collect_vec();
    let add_workspace_id = op_log_lines[3].split(' ').nth(2).unwrap();
    let initialize_repo_id = op_log_lines[5].split(' ').nth(2).unwrap();

    // Can load the repo at a specific operation ID
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, initialize_repo_id), @r###"
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, add_workspace_id), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);
    // "@" resolves to the head operation
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@"), @r###"
    @  bc8f18aa6f396a93572811632313cbb5625d475d
    ◉  0000000000000000000000000000000000000000
    "###);
    // "@-" resolves to the parent of the head operation
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path, "@-"), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "@---"]), @r###"
    Error: The "@---" expression resolved to no operations
    "###);

    // We get a reasonable message if an invalid operation ID is specified
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "foo"]), @r###"
    Error: Operation ID "foo" is not a valid hexadecimal prefix
    "###);

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 1"]);
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "describe",
            "-m",
            "description 2",
            "--at-op",
            add_workspace_id,
        ],
    );
    insta::assert_snapshot!(test_env.jj_cmd_failure(&repo_path, &["log", "--at-op", "@-"]), @r###"
    Error: The "@" expression resolved to more than one operation
    "###);
}

#[test]
fn test_op_log_limit() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log", "-Tdescription", "--limit=1"]);
    insta::assert_snapshot!(stdout, @r###"
    @  add workspace 'default'
    "###);
}

#[test]
fn test_op_log_no_graph() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout =
        test_env.jj_cmd_success(&repo_path, &["op", "log", "--no-graph", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;12m19b8089fc78b[39m [38;5;3mtest-username@host.example.com[39m [38;5;14m2001-02-03 04:05:07.000 +07:00[39m - [38;5;14m2001-02-03 04:05:07.000 +07:00[39m[0m
    [1madd workspace 'default'[0m
    [38;5;4mf1c462c494be[39m [38;5;3mtest-username@host.example.com[39m [38;5;6m2001-02-03 04:05:07.000 +07:00[39m - [38;5;6m2001-02-03 04:05:07.000 +07:00[39m
    initialize repo
    "###);
}

#[test]
fn test_op_log_no_graph_null_terminated() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "message1"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "message2"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "op",
            "log",
            "--no-graph",
            "--template",
            r#"id.short(4) ++ "\0""#,
        ],
    );
    insta::assert_debug_snapshot!(stdout, @r###""c8b0\07277\019b8\0f1c4\0""###);
}

#[test]
fn test_op_log_template() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| test_env.jj_cmd_success(&repo_path, &["op", "log", "-T", template]);

    insta::assert_snapshot!(render(r#"id ++ "\n""#), @r###"
    @  19b8089fc78b7c49171f3c8934248be6f89f52311005e961cab5780f9f138b142456d77b27d223d7ee84d21d8c30c4a80100eaf6735b548b1acd0da688f94c80
    ◉  f1c462c494be39f6690928603c5393f908866bc8d81d8cd1ae0bb2ea02cb4f78cafa47165fa5b7cda258e2178f846881de199066991960a80954ba6066ba0821
    "###);
    insta::assert_snapshot!(
        render(r#"separate(" ", id.short(5), current_operation, user,
                                time.start(), time.end(), time.duration()) ++ "\n""#), @r###"
    @  19b80 true test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 2001-02-03 04:05:07.000 +07:00 less than a microsecond
    ◉  f1c46 false test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 2001-02-03 04:05:07.000 +07:00 less than a microsecond
    "###);

    // Negative length shouldn't cause panic (and is clamped.)
    // TODO: If we add runtime error, this will probably error out.
    insta::assert_snapshot!(render(r#"id.short(-1) ++ "|""#), @r###"
    @  |
    ◉  |
    "###);

    // Test the default template, i.e. with relative start time and duration. We
    // don't generally use that template because it depends on the current time,
    // so we need to reset the time range format here.
    test_env.add_config(
        r#"
[template-aliases]
'format_time_range(time_range)' = 'time_range.start().ago() ++ ", lasted " ++ time_range.duration()'
        "#,
    );
    let regex = Regex::new(r"\d\d years").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    insta::assert_snapshot!(regex.replace_all(&stdout, "NN years"), @r###"
    @  19b8089fc78b test-username@host.example.com NN years ago, lasted less than a microsecond
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com NN years ago, lasted less than a microsecond
       initialize repo
    "###);
}

#[test]
fn test_op_log_builtin_templates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| test_env.jj_cmd_success(&repo_path, &["op", "log", "-T", template]);
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 0"]);

    insta::assert_snapshot!(render(r#"builtin_op_log_compact"#), @r###"
    @  98f7262e4a06 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    ◉  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    insta::assert_snapshot!(render(r#"builtin_op_log_comfortable"#), @r###"
    @  98f7262e4a06 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    │
    ◉  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    │
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo

    "###);
}

#[test]
fn test_op_log_word_wrap() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |args: &[&str], columns: u32, word_wrap: bool| {
        let mut args = args.to_vec();
        if word_wrap {
            args.push("--config-toml=ui.log-word-wrap=true");
        }
        let assert = test_env
            .jj_cmd(&repo_path, &args)
            .env("COLUMNS", columns.to_string())
            .assert()
            .success()
            .stderr("");
        get_stdout_string(&assert)
    };

    // ui.log-word-wrap option works
    insta::assert_snapshot!(render(&["op", "log"], 40, false), @r###"
    @  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
    insta::assert_snapshot!(render(&["op", "log"], 40, true), @r###"
    @  19b8089fc78b
    │  test-username@host.example.com
    │  2001-02-03 04:05:07.000 +07:00 -
    │  2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be
       test-username@host.example.com
       2001-02-03 04:05:07.000 +07:00 -
       2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
}

#[test]
fn test_op_log_configurable() {
    let test_env = TestEnvironment::default();
    test_env.add_config(
        r#"operation.hostname = "my-hostname"
        operation.username = "my-username"
        "#,
    );
    test_env
        .jj_cmd(test_env.env_root(), &["init", "repo", "--git"])
        .env_remove("JJ_OP_HOSTNAME")
        .env_remove("JJ_OP_USERNAME")
        .assert()
        .success();
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["op", "log"]);
    assert!(stdout.contains("my-username@my-hostname"));
}

#[test]
fn test_op_abandon_ancestors() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 1"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 2"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  bacc8030a969 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  bb26fe31d66f test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj commit -m 'commit 1'
    ◉  19b8089fc78b test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Abandon old operations. The working-copy operation id should be updated.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 2 operations and reparented 1 descendant operations.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["debug", "workingcopy"]), @r###"
    Current operation: OperationId("fb5252a68411468f5e3cf480a75b8b54d8ca9231406a3d0ddc4dfb31d851839a855aca5615ba4b09018fe45d11a04e1c051817a98de1c1ef5dd75cb6c2c09ba8")
    Current tree: Merge(Resolved(TreeId("4b825dc642cb6eb9a060e54bf8d69288fbee4904")))
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  fb5252a68411 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Abandon operation range.
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 3"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 4"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 5"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "@---..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 2 operations and reparented 1 descendant operations.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  ee40c9ad806a test-username@host.example.com 2001-02-03 04:05:16.000 +07:00 - 2001-02-03 04:05:16.000 +07:00
    │  commit e184d62c9ab118b0f62de91959b857550a9273a5
    │  args: jj commit -m 'commit 5'
    ◉  fb5252a68411 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Can't abandon the current operation.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["op", "abandon", "..@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot abandon the current operation
    Hint: Run `jj undo` to revert the current operation, then use `jj op abandon`
    "###);

    // Abandon the current operation by undoing it first.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 1 operations and reparented 1 descendant operations.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["debug", "workingcopy"]), @r###"
    Current operation: OperationId("05aebafee59813d56c0ea1576520b3074f5ba3e128f2b31df7370284cee593bed5043475dc2cdd30a6f22662c1dfb6aba92b83806147e77c17ad14356c07079d")
    Current tree: Merge(Resolved(TreeId("4b825dc642cb6eb9a060e54bf8d69288fbee4904")))
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  05aebafee598 test-username@host.example.com 2001-02-03 04:05:20.000 +07:00 - 2001-02-03 04:05:20.000 +07:00
    │  undo operation ee40c9ad806a7d42f351beab5aa81a8ac38d926d02711c059229bf6a7388b7b4a7c04c004067ee6c5b6253e8398fa82bc74d0d621f8bc2c8c11f33d445f90b77
    │  args: jj undo
    ◉  fb5252a68411 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  f1c462c494be test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Abandon empty range.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "@-..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log", "-l1"]), @r###"
    @  05aebafee598 test-username@host.example.com 2001-02-03 04:05:20.000 +07:00 - 2001-02-03 04:05:20.000 +07:00
    │  undo operation ee40c9ad806a7d42f351beab5aa81a8ac38d926d02711c059229bf6a7388b7b4a7c04c004067ee6c5b6253e8398fa82bc74d0d621f8bc2c8c11f33d445f90b77
    │  args: jj undo
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path, op_id: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "-T", "commit_id", "--at-op", op_id, "-r", "all()"],
    )
}
