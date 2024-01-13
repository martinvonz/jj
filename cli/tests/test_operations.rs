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
    @  00cc30b6a1b8 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    ◉  90e23f7e6c7c test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
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
    [1m[38;5;12m90e23f7e6c7c[39m [38;5;3mtest-username@host.example.com[39m [38;5;14m2001-02-03 04:05:07.000 +07:00[39m - [38;5;14m2001-02-03 04:05:07.000 +07:00[39m[0m
    [1madd workspace 'default'[0m
    [38;5;4mb73abeebcf94[39m [38;5;3mtest-username@host.example.com[39m [38;5;6m2001-02-03 04:05:07.000 +07:00[39m - [38;5;6m2001-02-03 04:05:07.000 +07:00[39m
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
    insta::assert_debug_snapshot!(stdout, @r###""6dec\08542\090e2\0b73a\0""###);
}

#[test]
fn test_op_log_template() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |template| test_env.jj_cmd_success(&repo_path, &["op", "log", "-T", template]);

    insta::assert_snapshot!(render(r#"id ++ "\n""#), @r###"
    @  90e23f7e6c7ce885df899e0b4990b39a43816549596a834b727d7eed9650cfe917ac2d043518734223fd7537b9d464e556d28e73cdfed7a0aceb592b89fdba4b
    ◉  b73abeebcf94a1528dfeeed335d6f8c8bac623d060254b01fb6b329d0407f1271eb2c16ecf56f6bac5de73dab988a1524dfb98bfff960eba5e54fa01547fea0d
    "###);
    insta::assert_snapshot!(
        render(r#"separate(" ", id.short(5), current_operation, user,
                                time.start(), time.end(), time.duration()) ++ "\n""#), @r###"
    @  90e23 true test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 2001-02-03 04:05:07.000 +07:00 less than a microsecond
    ◉  b73ab false test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 2001-02-03 04:05:07.000 +07:00 less than a microsecond
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
    @  90e23f7e6c7c test-username@host.example.com NN years ago, lasted less than a microsecond
    │  add workspace 'default'
    ◉  b73abeebcf94 test-username@host.example.com NN years ago, lasted less than a microsecond
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
    @  00cc30b6a1b8 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    ◉  90e23f7e6c7c test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    insta::assert_snapshot!(render(r#"builtin_op_log_comfortable"#), @r###"
    @  00cc30b6a1b8 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  describe commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj describe -m 'description 0'
    │
    ◉  90e23f7e6c7c test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    │
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
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
    @  90e23f7e6c7c test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);
    insta::assert_snapshot!(render(&["op", "log"], 40, true), @r###"
    @  90e23f7e6c7c
    │  test-username@host.example.com
    │  2001-02-03 04:05:07.000 +07:00 -
    │  2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  b73abeebcf94
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
    @  362b35cdcee0 test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  b46d00bc0741 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  commit 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  args: jj commit -m 'commit 1'
    ◉  90e23f7e6c7c test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Abandon old operations. The working-copy operation id should be updated.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 2 operations and reparented 1 descendant operations.
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["debug", "workingcopy", "--ignore-working-copy"]), @r###"
    Current operation: OperationId("3a0226ca7b0f364650c64a8b8ffe3c9802c5db40d9cc19387a29a28826ccada0dd4ec6926e649c5eb3c8150ebd5cd8ffb3c2c8922445c5dc8ccf474cd83d0d9c")
    Current tree: Legacy(TreeId("4b825dc642cb6eb9a060e54bf8d69288fbee4904"))
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  3a0226ca7b0f test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
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
    @  54d9d0dfc198 test-username@host.example.com 2001-02-03 04:05:16.000 +07:00 - 2001-02-03 04:05:16.000 +07:00
    │  commit e184d62c9ab118b0f62de91959b857550a9273a5
    │  args: jj commit -m 'commit 5'
    ◉  3a0226ca7b0f test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Can't abandon the current operation.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["op", "abandon", "..@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot abandon the current operation
    Hint: Run `jj undo` to revert the current operation, then use `jj op abandon`
    "###);

    // Can't create concurrent abandoned operations explicitly.
    let stderr = test_env.jj_cmd_failure(&repo_path, &["op", "abandon", "--at-op=@-", "@"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: --at-op is not respected
    "###);

    // Abandon the current operation by undoing it first.
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 1 operations and reparented 1 descendant operations.
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["debug", "workingcopy", "--ignore-working-copy"]), @r###"
    Current operation: OperationId("25cd03cd1029c1d22ea67cc11b89d357000bae5836f482be46d054220b1b08852713e32d0eec542bb275803214415ef8bb25b65a740fbc5c7b37094a311cac7c")
    Current tree: Legacy(TreeId("4b825dc642cb6eb9a060e54bf8d69288fbee4904"))
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @  25cd03cd1029 test-username@host.example.com 2001-02-03 04:05:21.000 +07:00 - 2001-02-03 04:05:21.000 +07:00
    │  undo operation 54d9d0dfc198e3550d04acd89329f8b49765a94c4c85239ee44d6a602127ac658ed91933ad32211cc26e32b3ef2f384e9e0bab471327338fb1b3f04df7916cc2
    │  args: jj undo
    ◉  3a0226ca7b0f test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    │  commit a8ac27b29a157ae7dabc0deb524df68823505730
    │  args: jj commit -m 'commit 2'
    ◉  b73abeebcf94 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
       initialize repo
    "###);

    // Abandon empty range.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "@-..@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log", "-l1"]), @r###"
    @  25cd03cd1029 test-username@host.example.com 2001-02-03 04:05:21.000 +07:00 - 2001-02-03 04:05:21.000 +07:00
    │  undo operation 54d9d0dfc198e3550d04acd89329f8b49765a94c4c85239ee44d6a602127ac658ed91933ad32211cc26e32b3ef2f384e9e0bab471327338fb1b3f04df7916cc2
    │  args: jj undo
    "###);
}

#[test]
fn test_op_abandon_without_updating_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 1"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 2"]);
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "commit 3"]);

    // Abandon without updating the working copy.
    let (_stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["op", "abandon", "@-", "--ignore-working-copy"],
    );
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 1 operations and reparented 1 descendant operations.
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["debug", "workingcopy", "--ignore-working-copy"]), @r###"
    Current operation: OperationId("324622dffebde1f5b9c75347254d0481aa5f51931816782a8a5c0578fe74893fbf63ba7921e94f44058b1e1162e932648146136b4c13236cf2eccc42347e6dcf")
    Current tree: Legacy(TreeId("4b825dc642cb6eb9a060e54bf8d69288fbee4904"))
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["op", "log", "-l1", "--ignore-working-copy"]), @r###"
    @  bc4e5d99e86e test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    │  commit 268f5f16139313ff25bef31280b2ec2e675200f3
    │  args: jj commit -m 'commit 3'
    "###);

    // The working-copy operation id isn't updated if it differs from the repo.
    // It could be updated if the tree matches, but there's no extra logic for
    // that.
    let (_stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["op", "abandon", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 1 operations and reparented 1 descendant operations.
    The working copy operation 324622dffebd is not updated because it differs from the repo bc4e5d99e86e.
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["debug", "workingcopy", "--ignore-working-copy"]), @r###"
    Current operation: OperationId("324622dffebde1f5b9c75347254d0481aa5f51931816782a8a5c0578fe74893fbf63ba7921e94f44058b1e1162e932648146136b4c13236cf2eccc42347e6dcf")
    Current tree: Legacy(TreeId("4b825dc642cb6eb9a060e54bf8d69288fbee4904"))
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["op", "log", "-l1", "--ignore-working-copy"]), @r###"
    @  d7c3591bc1df test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    │  commit 268f5f16139313ff25bef31280b2ec2e675200f3
    │  args: jj commit -m 'commit 3'
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path, op_id: &str) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &["log", "-T", "commit_id", "--at-op", op_id, "-r", "all()"],
    )
}
