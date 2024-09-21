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

use regex::Regex;

use crate::common::TestEnvironment;

#[test]
fn test_log_parents() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@-"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "@", "@-"]);

    let template =
        r#"commit_id ++ "\nP: " ++ parents.len() ++ " " ++ parents.map(|c| c.commit_id()) ++ "\n""#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @    c067170d4ca1bc6162b64f7550617ec809647f84
    ├─╮  P: 2 4db490c88528133d579540b6900b8098f0c17701 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ○ │  4db490c88528133d579540b6900b8098f0c17701
    ├─╯  P: 1 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ○  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  P: 1 0000000000000000000000000000000000000000
    ◆  0000000000000000000000000000000000000000
       P: 0
    "###);

    let template = r#"parents.map(|c| c.commit_id().shortest(4))"#;
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", template, "-r@", "--color=always"],
    );
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;2m@[0m  [1m[38;5;4m4[0m[38;5;8mdb4[39m [1m[38;5;4m2[0m[38;5;8m30d[39m
    │
    ~
    "###);

    // Commit object isn't printable
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-T", "parents"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse template: Expected expression of type "Template", but actual type is "List<Commit>"
    Caused by:  --> 1:1
      |
    1 | parents
      | ^-----^
      |
      = Expected expression of type "Template", but actual type is "List<Commit>"
    "###);

    // Redundant argument passed to keyword method
    let template = r#"parents.map(|c| c.commit_id(""))"#;
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse template: Function "commit_id": Expected 0 arguments
    Caused by:  --> 1:29
      |
    1 | parents.map(|c| c.commit_id(""))
      |                             ^^
      |
      = Function "commit_id": Expected 0 arguments
    "###);
}

#[test]
fn test_log_author_timestamp() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-03 04:05:09.000 +07:00
    ○  2001-02-03 04:05:08.000 +07:00
    ◆  1970-01-01 00:00:00.000 +00:00
    "###);
}

#[test]
fn test_log_author_timestamp_ago() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "second"]);

    let template = r#"author.timestamp().ago() ++ "\n""#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-T", template]);
    let line_re = Regex::new(r"[0-9]+ years ago").unwrap();
    assert!(
        stdout.lines().all(|x| line_re.is_match(x)),
        "expected every line to match regex"
    );
}

#[test]
fn test_log_author_timestamp_utc() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().utc()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-02 21:05:07.000 +00:00
    ◆  1970-01-01 00:00:00.000 +00:00
    "###);
}

#[cfg(unix)]
#[test]
fn test_log_author_timestamp_local() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_env_var("TZ", "UTC-05:30");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().local()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-03 08:05:07.000 +11:00
    ◆  1970-01-01 11:00:00.000 +11:00
    "###);
    test_env.add_env_var("TZ", "UTC+10:00");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().local()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-03 08:05:07.000 +11:00
    ◆  1970-01-01 11:00:00.000 +11:00
    "###);
}

#[test]
fn test_mine_is_true_when_author_is_user() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--config-toml=user.email='johndoe@example.com'",
            "--config-toml=user.name='John Doe'",
            "new",
        ],
    );

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            r#"coalesce(if(mine, "mine"), author.email(), email_placeholder)"#,
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  johndoe@example.com
    ○  mine
    ◆  (no email set)
    "###);
}

#[test]
fn test_log_default() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "description 1"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "my-bookmark"]);

    // Test default log output format
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  kkmpptxz test.user@example.com 2001-02-03 08:05:09 my-bookmark bac9ff9e
    │  (empty) description 1
    ○  qpvuntsm test.user@example.com 2001-02-03 08:05:08 aa2015d7
    │  add a file
    ◆  zzzzzzzz root() 00000000
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r#"
    [1m[38;5;2m@[0m  [1m[38;5;13mk[38;5;8mkmpptxz[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:09[39m [38;5;13mmy-bookmark[39m [38;5;12mb[38;5;8mac9ff9e[39m[0m
    │  [1m[38;5;10m(empty)[39m description 1[0m
    ○  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:08[39m [1m[38;5;4ma[0m[38;5;8ma2015d7[39m
    │  add a file
    [1m[38;5;14m◆[0m  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "#);

    // Color without graph
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r#"
    [1m[38;5;13mk[38;5;8mkmpptxz[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:09[39m [38;5;13mmy-bookmark[39m [38;5;12mb[38;5;8mac9ff9e[39m[0m
    [1m[38;5;10m(empty)[39m description 1[0m
    [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:08[39m [1m[38;5;4ma[0m[38;5;8ma2015d7[39m
    add a file
    [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "#);
}

#[test]
fn test_log_builtin_templates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    // Render without graph and append "[EOF]" marker to test line ending
    let render = |template| {
        test_env.jj_cmd_success(&repo_path, &["log", "-T", template, "--no-graph"]) + "[EOF]\n"
    };

    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--config-toml=user.email=''",
            "--config-toml=user.name=''",
            "new",
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "my-bookmark"]);

    insta::assert_snapshot!(render(r#"builtin_log_oneline"#), @r###"
    rlvkpnrz (no email set) 2001-02-03 08:05:08 my-bookmark dc315397 (empty) (no description set)
    qpvuntsm test.user 2001-02-03 08:05:07 230dd059 (empty) (no description set)
    zzzzzzzz root() 00000000
    [EOF]
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_compact"#), @r###"
    rlvkpnrz (no email set) 2001-02-03 08:05:08 my-bookmark dc315397
    (empty) (no description set)
    qpvuntsm test.user@example.com 2001-02-03 08:05:07 230dd059
    (empty) (no description set)
    zzzzzzzz root() 00000000
    [EOF]
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_comfortable"#), @r###"
    rlvkpnrz (no email set) 2001-02-03 08:05:08 my-bookmark dc315397
    (empty) (no description set)

    qpvuntsm test.user@example.com 2001-02-03 08:05:07 230dd059
    (empty) (no description set)

    zzzzzzzz root() 00000000

    [EOF]
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_detailed"#), @r###"
    Commit ID: dc31539712c7294d1d712cec63cef4504b94ca74
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Bookmarks: my-bookmark
    Author: (no name set) <(no email set)> (2001-02-03 08:05:08)
    Committer: (no name set) <(no email set)> (2001-02-03 08:05:08)

        (no description set)

    Commit ID: 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 08:05:07)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:07)

        (no description set)

    Commit ID: 0000000000000000000000000000000000000000
    Change ID: zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz
    Author: (no name set) <(no email set)> (1970-01-01 11:00:00)
    Committer: (no name set) <(no email set)> (1970-01-01 11:00:00)

        (no description set)

    [EOF]
    "###);
}

#[test]
fn test_log_builtin_templates_colored() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let render =
        |template| test_env.jj_cmd_success(&repo_path, &["--color=always", "log", "-T", template]);

    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--config-toml=user.email=''",
            "--config-toml=user.name=''",
            "new",
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "my-bookmark"]);

    insta::assert_snapshot!(render(r#"builtin_log_oneline"#), @r#"
    [1m[38;5;2m@[0m  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;9m(no email set)[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;13mmy-bookmark[39m [38;5;12md[38;5;8mc315397[39m [38;5;10m(empty)[39m [38;5;10m(no description set)[39m[0m
    ○  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    [1m[38;5;14m◆[0m  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "#);

    insta::assert_snapshot!(render(r#"builtin_log_compact"#), @r#"
    [1m[38;5;2m@[0m  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;9m(no email set)[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;13mmy-bookmark[39m [38;5;12md[38;5;8mc315397[39m[0m
    │  [1m[38;5;10m(empty)[39m [38;5;10m(no description set)[39m[0m
    ○  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
    │  [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    [1m[38;5;14m◆[0m  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "#);

    insta::assert_snapshot!(render(r#"builtin_log_comfortable"#), @r#"
    [1m[38;5;2m@[0m  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;9m(no email set)[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;13mmy-bookmark[39m [38;5;12md[38;5;8mc315397[39m[0m
    │  [1m[38;5;10m(empty)[39m [38;5;10m(no description set)[39m[0m
    │
    ○  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
    │  [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    │
    [1m[38;5;14m◆[0m  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m

    "#);

    insta::assert_snapshot!(render(r#"builtin_log_detailed"#), @r###"
    [1m[38;5;2m@[0m  Commit ID: [38;5;4mdc31539712c7294d1d712cec63cef4504b94ca74[39m
    │  Change ID: [38;5;5mrlvkpnrzqnoowoytxnquwvuryrwnrmlp[39m
    │  Bookmarks: [38;5;5mmy-bookmark[39m
    │  Author: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m2001-02-03 08:05:08[39m)
    │  Committer: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m2001-02-03 08:05:08[39m)
    │
    │  [38;5;2m    (no description set)[39m
    │
    ○  Commit ID: [38;5;4m230dd059e1b059aefc0da06a2e5a7dbf22362f22[39m
    │  Change ID: [38;5;5mqpvuntsmwlqtpsluzzsnyyzlmlwvmlnu[39m
    │  Author: Test User <[38;5;3mtest.user@example.com[39m> ([38;5;6m2001-02-03 08:05:07[39m)
    │  Committer: Test User <[38;5;3mtest.user@example.com[39m> ([38;5;6m2001-02-03 08:05:07[39m)
    │
    │  [38;5;2m    (no description set)[39m
    │
    [1m[38;5;14m◆[0m  Commit ID: [38;5;4m0000000000000000000000000000000000000000[39m
       Change ID: [38;5;5mzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz[39m
       Author: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m1970-01-01 11:00:00[39m)
       Committer: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m1970-01-01 11:00:00[39m)

       [38;5;2m    (no description set)[39m

    "###);
}

#[test]
fn test_log_builtin_templates_colored_debug() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    let render =
        |template| test_env.jj_cmd_success(&repo_path, &["--color=debug", "log", "-T", template]);

    test_env.jj_cmd_ok(
        &repo_path,
        &[
            "--config-toml=user.email=''",
            "--config-toml=user.name=''",
            "new",
        ],
    );
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "my-bookmark"]);

    insta::assert_snapshot!(render(r#"builtin_log_oneline"#), @r#"
    [1m[38;5;2m<<node working_copy::@>>[0m  [1m[38;5;13m<<log working_copy change_id shortest prefix::r>>[38;5;8m<<log working_copy change_id shortest rest::lvkpnrz>>[39m<<log working_copy:: >>[38;5;9m<<log working_copy email placeholder::(no email set)>>[39m<<log working_copy:: >>[38;5;14m<<log working_copy committer timestamp local format::2001-02-03 08:05:08>>[39m<<log working_copy:: >>[38;5;13m<<log working_copy bookmarks name::my-bookmark>>[39m<<log working_copy:: >>[38;5;12m<<log working_copy commit_id shortest prefix::d>>[38;5;8m<<log working_copy commit_id shortest rest::c315397>>[39m<<log working_copy:: >>[38;5;10m<<log working_copy empty::(empty)>>[39m<<log working_copy:: >>[38;5;10m<<log working_copy empty description placeholder::(no description set)>>[39m<<log working_copy::>>[0m
    <<node::○>>  [1m[38;5;5m<<log change_id shortest prefix::q>>[0m[38;5;8m<<log change_id shortest rest::pvuntsm>>[39m<<log:: >>[38;5;3m<<log author username::test.user>>[39m<<log:: >>[38;5;6m<<log committer timestamp local format::2001-02-03 08:05:07>>[39m<<log:: >>[1m[38;5;4m<<log commit_id shortest prefix::2>>[0m[38;5;8m<<log commit_id shortest rest::30dd059>>[39m<<log:: >>[38;5;2m<<log empty::(empty)>>[39m<<log:: >>[38;5;2m<<log empty description placeholder::(no description set)>>[39m<<log::>>
    [1m[38;5;14m<<node immutable::◆>>[0m  [1m[38;5;5m<<log change_id shortest prefix::z>>[0m[38;5;8m<<log change_id shortest rest::zzzzzzz>>[39m<<log:: >>[38;5;2m<<log root::root()>>[39m<<log:: >>[1m[38;5;4m<<log commit_id shortest prefix::0>>[0m[38;5;8m<<log commit_id shortest rest::0000000>>[39m<<log::>>
    "#);

    insta::assert_snapshot!(render(r#"builtin_log_compact"#), @r#"
    [1m[38;5;2m<<node working_copy::@>>[0m  [1m[38;5;13m<<log working_copy change_id shortest prefix::r>>[38;5;8m<<log working_copy change_id shortest rest::lvkpnrz>>[39m<<log working_copy:: >>[38;5;9m<<log working_copy email placeholder::(no email set)>>[39m<<log working_copy:: >>[38;5;14m<<log working_copy committer timestamp local format::2001-02-03 08:05:08>>[39m<<log working_copy:: >>[38;5;13m<<log working_copy bookmarks name::my-bookmark>>[39m<<log working_copy:: >>[38;5;12m<<log working_copy commit_id shortest prefix::d>>[38;5;8m<<log working_copy commit_id shortest rest::c315397>>[39m<<log working_copy::>>[0m
    │  [1m[38;5;10m<<log working_copy empty::(empty)>>[39m<<log working_copy:: >>[38;5;10m<<log working_copy empty description placeholder::(no description set)>>[39m<<log working_copy::>>[0m
    <<node::○>>  [1m[38;5;5m<<log change_id shortest prefix::q>>[0m[38;5;8m<<log change_id shortest rest::pvuntsm>>[39m<<log:: >>[38;5;3m<<log author email::test.user@example.com>>[39m<<log:: >>[38;5;6m<<log committer timestamp local format::2001-02-03 08:05:07>>[39m<<log:: >>[1m[38;5;4m<<log commit_id shortest prefix::2>>[0m[38;5;8m<<log commit_id shortest rest::30dd059>>[39m<<log::>>
    │  [38;5;2m<<log empty::(empty)>>[39m<<log:: >>[38;5;2m<<log empty description placeholder::(no description set)>>[39m<<log::>>
    [1m[38;5;14m<<node immutable::◆>>[0m  [1m[38;5;5m<<log change_id shortest prefix::z>>[0m[38;5;8m<<log change_id shortest rest::zzzzzzz>>[39m<<log:: >>[38;5;2m<<log root::root()>>[39m<<log:: >>[1m[38;5;4m<<log commit_id shortest prefix::0>>[0m[38;5;8m<<log commit_id shortest rest::0000000>>[39m<<log::>>
    "#);

    insta::assert_snapshot!(render(r#"builtin_log_comfortable"#), @r#"
    [1m[38;5;2m<<node working_copy::@>>[0m  [1m[38;5;13m<<log working_copy change_id shortest prefix::r>>[38;5;8m<<log working_copy change_id shortest rest::lvkpnrz>>[39m<<log working_copy:: >>[38;5;9m<<log working_copy email placeholder::(no email set)>>[39m<<log working_copy:: >>[38;5;14m<<log working_copy committer timestamp local format::2001-02-03 08:05:08>>[39m<<log working_copy:: >>[38;5;13m<<log working_copy bookmarks name::my-bookmark>>[39m<<log working_copy:: >>[38;5;12m<<log working_copy commit_id shortest prefix::d>>[38;5;8m<<log working_copy commit_id shortest rest::c315397>>[39m<<log working_copy::>>[0m
    │  [1m[38;5;10m<<log working_copy empty::(empty)>>[39m<<log working_copy:: >>[38;5;10m<<log working_copy empty description placeholder::(no description set)>>[39m<<log working_copy::>>[0m
    │  <<log::>>
    <<node::○>>  [1m[38;5;5m<<log change_id shortest prefix::q>>[0m[38;5;8m<<log change_id shortest rest::pvuntsm>>[39m<<log:: >>[38;5;3m<<log author email::test.user@example.com>>[39m<<log:: >>[38;5;6m<<log committer timestamp local format::2001-02-03 08:05:07>>[39m<<log:: >>[1m[38;5;4m<<log commit_id shortest prefix::2>>[0m[38;5;8m<<log commit_id shortest rest::30dd059>>[39m<<log::>>
    │  [38;5;2m<<log empty::(empty)>>[39m<<log:: >>[38;5;2m<<log empty description placeholder::(no description set)>>[39m<<log::>>
    │  <<log::>>
    [1m[38;5;14m<<node immutable::◆>>[0m  [1m[38;5;5m<<log change_id shortest prefix::z>>[0m[38;5;8m<<log change_id shortest rest::zzzzzzz>>[39m<<log:: >>[38;5;2m<<log root::root()>>[39m<<log:: >>[1m[38;5;4m<<log commit_id shortest prefix::0>>[0m[38;5;8m<<log commit_id shortest rest::0000000>>[39m<<log::>>
       <<log::>>
    "#);

    insta::assert_snapshot!(render(r#"builtin_log_detailed"#), @r###"
    [1m[38;5;2m<<node working_copy::@>>[0m  <<log::Commit ID: >>[38;5;4m<<log commit_id::dc31539712c7294d1d712cec63cef4504b94ca74>>[39m<<log::>>
    │  <<log::Change ID: >>[38;5;5m<<log change_id::rlvkpnrzqnoowoytxnquwvuryrwnrmlp>>[39m<<log::>>
    │  <<log::Bookmarks: >>[38;5;5m<<log local_bookmarks name::my-bookmark>>[39m<<log::>>
    │  <<log::Author: >>[38;5;1m<<log name placeholder::(no name set)>>[39m<<log:: <>>[38;5;1m<<log email placeholder::(no email set)>>[39m<<log::> (>>[38;5;6m<<log author timestamp local format::2001-02-03 08:05:08>>[39m<<log::)>>
    │  <<log::Committer: >>[38;5;1m<<log name placeholder::(no name set)>>[39m<<log:: <>>[38;5;1m<<log email placeholder::(no email set)>>[39m<<log::> (>>[38;5;6m<<log committer timestamp local format::2001-02-03 08:05:08>>[39m<<log::)>>
    │  <<log::>>
    │  [38;5;2m<<log empty description placeholder::    (no description set)>>[39m<<log::>>
    │  <<log::>>
    <<node::○>>  <<log::Commit ID: >>[38;5;4m<<log commit_id::230dd059e1b059aefc0da06a2e5a7dbf22362f22>>[39m<<log::>>
    │  <<log::Change ID: >>[38;5;5m<<log change_id::qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu>>[39m<<log::>>
    │  <<log::Author: >><<log author name::Test User>><<log:: <>>[38;5;3m<<log author email::test.user@example.com>>[39m<<log::> (>>[38;5;6m<<log author timestamp local format::2001-02-03 08:05:07>>[39m<<log::)>>
    │  <<log::Committer: >><<log committer name::Test User>><<log:: <>>[38;5;3m<<log committer email::test.user@example.com>>[39m<<log::> (>>[38;5;6m<<log committer timestamp local format::2001-02-03 08:05:07>>[39m<<log::)>>
    │  <<log::>>
    │  [38;5;2m<<log empty description placeholder::    (no description set)>>[39m<<log::>>
    │  <<log::>>
    [1m[38;5;14m<<node immutable::◆>>[0m  <<log::Commit ID: >>[38;5;4m<<log commit_id::0000000000000000000000000000000000000000>>[39m<<log::>>
       <<log::Change ID: >>[38;5;5m<<log change_id::zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz>>[39m<<log::>>
       <<log::Author: >>[38;5;1m<<log name placeholder::(no name set)>>[39m<<log:: <>>[38;5;1m<<log email placeholder::(no email set)>>[39m<<log::> (>>[38;5;6m<<log author timestamp local format::1970-01-01 11:00:00>>[39m<<log::)>>
       <<log::Committer: >>[38;5;1m<<log name placeholder::(no name set)>>[39m<<log:: <>>[38;5;1m<<log email placeholder::(no email set)>>[39m<<log::> (>>[38;5;6m<<log committer timestamp local format::1970-01-01 11:00:00>>[39m<<log::)>>
       <<log::>>
       [38;5;2m<<log empty description placeholder::    (no description set)>>[39m<<log::>>
       <<log::>>
    "###);
}

#[test]
fn test_log_evolog_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 1"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    // No divergence
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 08:05:08 ff309c29
    │  description 1
    ◆  zzzzzzzz root() 00000000
    "###);

    // Create divergence
    test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "-m", "description 2", "--at-operation", "@-"],
    );
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    ○  qpvuntsm?? test.user@example.com 2001-02-03 08:05:10 6ba70e00
    │  description 2
    │ @  qpvuntsm?? test.user@example.com 2001-02-03 08:05:08 ff309c29
    ├─╯  description 1
    ◆  zzzzzzzz root() 00000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    ○  [1m[4m[38;5;1mq[0m[38;5;1mpvuntsm??[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:10[39m [1m[38;5;4m6[0m[38;5;8mba70e00[39m
    │  description 2
    │ [1m[38;5;2m@[0m  [1m[4m[38;5;1mq[24mpvuntsm[38;5;9m??[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;12mf[38;5;8mf309c29[39m[0m
    ├─╯  [1mdescription 1[0m
    [1m[38;5;14m◆[0m  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);

    // Evolog and hidden divergent
    let stdout = test_env.jj_cmd_success(&repo_path, &["evolog"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm?? test.user@example.com 2001-02-03 08:05:08 ff309c29
    │  description 1
    ○  qpvuntsm hidden test.user@example.com 2001-02-03 08:05:08 485d52a9
    │  (no description set)
    ○  qpvuntsm hidden test.user@example.com 2001-02-03 08:05:07 230dd059
       (empty) (no description set)
    "###);

    // Colored evolog
    let stdout = test_env.jj_cmd_success(&repo_path, &["evolog", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;2m@[0m  [1m[4m[38;5;1mq[24mpvuntsm[38;5;9m??[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;12mf[38;5;8mf309c29[39m[0m
    │  [1mdescription 1[0m
    ○  [1m[39mq[0m[38;5;8mpvuntsm[39m hidden [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:08[39m [1m[38;5;4m4[0m[38;5;8m85d52a9[39m
    │  [38;5;3m(no description set)[39m
    ○  [1m[39mq[0m[38;5;8mpvuntsm[39m hidden [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
       [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    "###);
}

#[test]
fn test_log_bookmarks() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-branch = true");
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);

    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    // Created some bookmarks on the remote
    test_env.jj_cmd_ok(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "create", "bookmark1"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "root()", "-m=description 2"]);
    test_env.jj_cmd_ok(
        &origin_path,
        &["bookmark", "create", "bookmark2", "unchanged"],
    );
    test_env.jj_cmd_ok(&origin_path, &["new", "root()", "-m=description 3"]);
    test_env.jj_cmd_ok(&origin_path, &["bookmark", "create", "bookmark3"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);
    test_env.jj_cmd_ok(
        test_env.env_root(),
        &[
            "git",
            "clone",
            origin_git_repo_path.to_str().unwrap(),
            "local",
        ],
    );
    let workspace_root = test_env.env_root().join("local");

    // Rewrite bookmark1, move bookmark2 forward, create conflict in bookmark3, add
    // new-bookmark
    test_env.jj_cmd_ok(
        &workspace_root,
        &["describe", "bookmark1", "-m", "modified bookmark1 commit"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["new", "bookmark2"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "set", "bookmark2"]);
    test_env.jj_cmd_ok(&workspace_root, &["bookmark", "create", "new-bookmark"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "bookmark3", "-m=local"]);
    test_env.jj_cmd_ok(&origin_path, &["describe", "bookmark3", "-m=origin"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);
    test_env.jj_cmd_ok(&workspace_root, &["git", "fetch"]);

    let template = r#"commit_id.short() ++ " " ++ if(bookmarks, bookmarks, "(no bookmarks)")"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ○  fed794e2ba44 bookmark3?? bookmark3@origin
    │ ○  b1bb3766d584 bookmark3??
    ├─╯
    │ ○  4a7e4246fc4d bookmark1*
    ├─╯
    │ @  a5b4d15489cc bookmark2* new-bookmark
    │ ○  8476341eb395 bookmark2@origin unchanged
    ├─╯
    ◆  000000000000 (no bookmarks)
    "###);

    let template = r#"bookmarks.map(|b| separate("/", b.remote(), b.name())).join(", ")"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ○  bookmark3, origin/bookmark3
    │ ○  bookmark3
    ├─╯
    │ ○  bookmark1
    ├─╯
    │ @  bookmark2, new-bookmark
    │ ○  origin/bookmark2, unchanged
    ├─╯
    ◆
    "###);

    let template = r#"separate(" ", "L:", local_bookmarks, "R:", remote_bookmarks)"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ○  L: bookmark3?? R: bookmark3@origin
    │ ○  L: bookmark3?? R:
    ├─╯
    │ ○  L: bookmark1* R:
    ├─╯
    │ @  L: bookmark2* new-bookmark R:
    │ ○  L: unchanged R: bookmark2@origin unchanged@origin
    ├─╯
    ◆  L: R:
    "###);

    let template = r#"
    remote_bookmarks.map(|ref| concat(
      ref,
      if(ref.tracked(),
        "(+" ++ ref.tracking_ahead_count().lower()
        ++ "/-" ++ ref.tracking_behind_count().lower() ++ ")"),
    ))
    "#;
    let output = test_env.jj_cmd_success(
        &workspace_root,
        &["log", "-r::remote_bookmarks()", "-T", template],
    );
    insta::assert_snapshot!(output, @r###"
    ○  bookmark3@origin(+0/-1)
    │ ○  bookmark2@origin(+0/-1) unchanged@origin(+0/-0)
    ├─╯
    │ ○  bookmark1@origin(+1/-1)
    ├─╯
    ◆
    "###);
}

#[test]
fn test_log_git_head() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["git", "init", "--git-repo=."]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=initial"]);
    std::fs::write(repo_path.join("file"), "foo\n").unwrap();

    let template = r#"
    separate(", ",
      if(git_head, "name: " ++ git_head.name()),
      "remote: " ++ git_head.remote(),
    ) ++ "\n"
    "#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @  remote: <Error: No RefName available>
    ○  name: HEAD, remote: git
    ◆  remote: <Error: No RefName available>
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;2m@[0m  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:09[39m [38;5;12m5[38;5;8m0aaf475[39m[0m
    │  [1minitial[0m
    ○  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [38;5;2mHEAD@git[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
    │  [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    [1m[38;5;14m◆[0m  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);
}

#[test]
fn test_log_commit_id_normal_hex() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            r#"commit_id ++ ": " ++ commit_id.normal_hex()"#,
        ],
    );
    insta::assert_snapshot!(stdout, @r#"
    @  6572f22267c6f0f2bf7b8a37969ee5a7d54b8aae: 6572f22267c6f0f2bf7b8a37969ee5a7d54b8aae
    ○  222fa9f0b41347630a1371203b8aad3897d34e5f: 222fa9f0b41347630a1371203b8aad3897d34e5f
    ○  230dd059e1b059aefc0da06a2e5a7dbf22362f22: 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◆  0000000000000000000000000000000000000000: 0000000000000000000000000000000000000000
    "#);
}

#[test]
fn test_log_change_id_normal_hex() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            r#"change_id ++ ": " ++ change_id.normal_hex()"#,
        ],
    );
    insta::assert_snapshot!(stdout, @r#"
    @  kkmpptxzrspxrzommnulwmwkkqwworpl: ffdaa62087a280bddc5e3d3ff933b8ae
    ○  rlvkpnrzqnoowoytxnquwvuryrwnrmlp: 8e4fac809cbb3b162c953458183c8dea
    ○  qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu: 9a45c67d3e96a7e5007c110ede34dec5
    ◆  zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz: 00000000000000000000000000000000
    "#);
}

#[test]
fn test_log_customize_short_id() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "first"]);

    // Customize both the commit and the change id
    let decl = "template-aliases.'format_short_id(id)'";
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--config-toml",
            &format!(r#"{decl}='id.shortest(5).prefix().upper() ++ "_" ++ id.shortest(5).rest()'"#),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  Q_pvun test.user@example.com 2001-02-03 08:05:08 F_a156
    │  (empty) first
    ◆  Z_zzzz root() 0_0000
    "###);

    // Customize only the change id
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "--config-toml",
            r#"
                [template-aliases]
                'format_short_change_id(id)'='format_short_id(id).upper()'
            "#,
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  QPVUNTSM test.user@example.com 2001-02-03 08:05:08 fa15625b
    │  (empty) first
    ◆  ZZZZZZZZ root() 00000000
    "###);
}

#[test]
fn test_log_immutable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["new", "-mA", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mB"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mC"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mD", "root()"]);

    let template = r#"
    separate(" ",
      description.first_line(),
      bookmarks,
      if(immutable, "[immutable]"),
    ) ++ "\n"
    "#;

    test_env.add_config("revset-aliases.'immutable_heads()' = 'main'");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r::", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @  D
    │ ○  C
    │ ◆  B main [immutable]
    │ ◆  A [immutable]
    ├─╯
    ◆  [immutable]
    "###);

    // Suppress error that could be detected earlier
    test_env.add_config("revsets.short-prefixes = ''");

    test_env.add_config("revset-aliases.'immutable_heads()' = 'unknown_fn()'");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r::", "-T", template]);
    insta::assert_snapshot!(stderr, @r#"
    Config error: Invalid `revset-aliases.immutable_heads()`
    Caused by:  --> 1:1
      |
    1 | unknown_fn()
      | ^--------^
      |
      = Function "unknown_fn" doesn't exist
    For help, see https://martinvonz.github.io/jj/latest/config/.
    "#);

    test_env.add_config("revset-aliases.'immutable_heads()' = 'unknown_symbol'");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r::", "-T", template]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse template: Failed to evaluate revset
    Caused by:
    1:  --> 5:10
      |
    5 |       if(immutable, "[immutable]"),
      |          ^-------^
      |
      = Failed to evaluate revset
    2: Revision "unknown_symbol" doesn't exist
    "###);
}

#[test]
fn test_log_contained_in() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["new", "-mA", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mB"]);
    test_env.jj_cmd_ok(&repo_path, &["bookmark", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mC"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mD", "root()"]);

    let template_for_revset = |revset: &str| {
        format!(
            r#"
    separate(" ",
      description.first_line(),
      bookmarks,
      if(self.contained_in("{revset}"), "[contained_in]"),
    ) ++ "\n"
    "#
        )
    };

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-r::",
            "-T",
            &template_for_revset(r#"description(A)::"#),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  D
    │ ○  C [contained_in]
    │ ○  B main [contained_in]
    │ ○  A [contained_in]
    ├─╯
    ◆
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-r::",
            "-T",
            &template_for_revset(r#"visible_heads()"#),
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  D [contained_in]
    │ ○  C [contained_in]
    │ ○  B main
    │ ○  A
    ├─╯
    ◆
    "###);

    // Suppress error that could be detected earlier
    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["log", "-r::", "-T", &template_for_revset("unknown_fn()")],
    );
    insta::assert_snapshot!(stderr, @r#"
    Error: Failed to parse template: In revset expression
    Caused by:
    1:  --> 5:28
      |
    5 |       if(self.contained_in("unknown_fn()"), "[contained_in]"),
      |                            ^------------^
      |
      = In revset expression
    2:  --> 1:1
      |
    1 | unknown_fn()
      | ^--------^
      |
      = Function "unknown_fn" doesn't exist
    "#);

    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["log", "-r::", "-T", &template_for_revset("author(x:'y')")],
    );
    insta::assert_snapshot!(stderr, @r#"
    Error: Failed to parse template: In revset expression
    Caused by:
    1:  --> 5:28
      |
    5 |       if(self.contained_in("author(x:'y')"), "[contained_in]"),
      |                            ^-------------^
      |
      = In revset expression
    2:  --> 1:8
      |
    1 | author(x:'y')
      |        ^---^
      |
      = Invalid string pattern
    3: Invalid string pattern kind "x:"
    Hint: Try prefixing with one of `exact:`, `glob:`, `regex:`, or `substring:`
    "#);

    let stderr = test_env.jj_cmd_failure(
        &repo_path,
        &["log", "-r::", "-T", &template_for_revset("maine")],
    );
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse template: Failed to evaluate revset
    Caused by:
    1:  --> 5:28
      |
    5 |       if(self.contained_in("maine"), "[contained_in]"),
      |                            ^-----^
      |
      = Failed to evaluate revset
    2: Revision "maine" doesn't exist
    Hint: Did you mean "main"?
    "###);
}

#[test]
fn test_short_prefix_in_transaction() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_config(r#"
        [revsets]
        log = '::description(test)'

        [templates]
        log = 'summary ++ "\n"'
        commit_summary = 'summary'

        [template-aliases]
        'format_id(id)' = 'id.shortest(12).prefix() ++ "[" ++ id.shortest(12).rest() ++ "]"'
        'summary' = 'separate(" ", format_id(change_id), format_id(commit_id), description.first_line())'
    "#);

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "initial"]);

    // Create a chain of 5 commits
    for i in 0..5 {
        test_env.jj_cmd_ok(&repo_path, &["new", "-m", &format!("commit{i}")]);
        std::fs::write(repo_path.join("file"), format!("file {i}\n")).unwrap();
    }
    // Create 2^4 duplicates of the chain
    for _ in 0..4 {
        test_env.jj_cmd_ok(&repo_path, &["duplicate", "description(commit)"]);
    }

    // Short prefix should be used for commit summary inside the transaction
    let parent_id = "58731d"; // Force id lookup to build index before mutation.
                              // If the cached index wasn't invalidated, the
                              // newly created commit wouldn't be found in it.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["new", parent_id, "--no-edit", "-m", "test"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Created new commit km[kuslswpqwq] 7[4ac55dd119b] test
    "###);

    // Should match log's short prefixes
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    km[kuslswpqwq] 7[4ac55dd119b] test
    y[qosqzytrlsw] 5[8731db5875e] commit4
    r[oyxmykxtrkr] 9[95cc897bca7] commit3
    m[zvwutvlkqwt] 3[74534c54448] commit2
    zs[uskulnrvyr] d[e304c281bed] commit1
    kk[mpptxzrspx] 05[2755155952] commit0
    q[pvuntsmwlqt] e[0e22b9fae75] initial
    zz[zzzzzzzzzz] 00[0000000000]
    "###);

    test_env.add_config(r#"revsets.short-prefixes = """#);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    kmk[uslswpqwq] 74ac[55dd119b] test
    yq[osqzytrlsw] 587[31db5875e] commit4
    ro[yxmykxtrkr] 99[5cc897bca7] commit3
    mz[vwutvlkqwt] 374[534c54448] commit2
    zs[uskulnrvyr] de[304c281bed] commit1
    kk[mpptxzrspx] 052[755155952] commit0
    qp[vuntsmwlqt] e0[e22b9fae75] initial
    zz[zzzzzzzzzz] 00[0000000000]
    "###);
}

#[test]
fn test_log_diff_predefined_formats() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\nb\n").unwrap();
    std::fs::write(repo_path.join("file2"), "a\n").unwrap();
    std::fs::write(repo_path.join("rename-source"), "rename").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nc\n").unwrap();
    std::fs::write(repo_path.join("file2"), "b\nc\n").unwrap();
    std::fs::rename(
        repo_path.join("rename-source"),
        repo_path.join("rename-target"),
    )
    .unwrap();

    let template = r#"
    concat(
      "=== color_words ===\n",
      diff.color_words(),
      "=== git ===\n",
      diff.git(),
      "=== stat ===\n",
      diff.stat(80),
      "=== summary ===\n",
      diff.summary(),
    )
    "#;

    // color, without paths
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "--no-graph", "--color=always", "-r@", "-T", template],
    );
    insta::assert_snapshot!(stdout, @r###"
    === color_words ===
    [38;5;3mModified regular file file1:[39m
    [38;5;1m   1[39m [38;5;2m   1[39m: a
    [38;5;1m   2[39m [38;5;2m   2[39m: b
         [38;5;2m   3[39m: [4m[38;5;2mc[24m[39m
    [38;5;3mModified regular file file2:[39m
    [38;5;1m   1[39m [38;5;2m   1[39m: [4m[38;5;1ma[38;5;2mb[24m[39m
         [38;5;2m   2[39m: [4m[38;5;2mc[24m[39m
    [38;5;3mModified regular file rename-target (rename-source => rename-target):[39m
    === git ===
    [1mdiff --git a/file1 b/file1[0m
    [1mindex 422c2b7ab3..de980441c3 100644[0m
    [1m--- a/file1[0m
    [1m+++ b/file1[0m
    [38;5;6m@@ -1,2 +1,3 @@[39m
     a
     b
    [38;5;2m+[4mc[24m[39m
    [1mdiff --git a/file2 b/file2[0m
    [1mindex 7898192261..9ddeb5c484 100644[0m
    [1m--- a/file2[0m
    [1m+++ b/file2[0m
    [38;5;6m@@ -1,1 +1,2 @@[39m
    [38;5;1m-[4ma[24m[39m
    [38;5;2m+[4mb[24m[39m
    [38;5;2m+[4mc[24m[39m
    [1mdiff --git a/rename-source b/rename-target[0m
    [1mrename from rename-source[0m
    [1mrename to rename-target[0m
    === stat ===
    file1                            | 1 [38;5;2m+[38;5;1m[39m
    file2                            | 3 [38;5;2m++[38;5;1m-[39m
    {rename-source => rename-target} | 0[38;5;1m[39m
    3 files changed, 3 insertions(+), 1 deletion(-)
    === summary ===
    [38;5;6mM file1[39m
    [38;5;6mM file2[39m
    [38;5;6mR {rename-source => rename-target}[39m
    "###);

    // color labels
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "--no-graph", "--color=debug", "-r@", "-T", template],
    );
    insta::assert_snapshot!(stdout, @r###"
    <<log::=== color_words ===>>
    [38;5;3m<<log diff color_words header::Modified regular file file1:>>[39m
    [38;5;1m<<log diff color_words removed line_number::   1>>[39m<<log diff color_words:: >>[38;5;2m<<log diff color_words added line_number::   1>>[39m<<log diff color_words::: a>>
    [38;5;1m<<log diff color_words removed line_number::   2>>[39m<<log diff color_words:: >>[38;5;2m<<log diff color_words added line_number::   2>>[39m<<log diff color_words::: b>>
    <<log diff color_words::     >>[38;5;2m<<log diff color_words added line_number::   3>>[39m<<log diff color_words::: >>[4m[38;5;2m<<log diff color_words added token::c>>[24m[39m
    [38;5;3m<<log diff color_words header::Modified regular file file2:>>[39m
    [38;5;1m<<log diff color_words removed line_number::   1>>[39m<<log diff color_words:: >>[38;5;2m<<log diff color_words added line_number::   1>>[39m<<log diff color_words::: >>[4m[38;5;1m<<log diff color_words removed token::a>>[38;5;2m<<log diff color_words added token::b>>[24m[39m<<log diff color_words::>>
    <<log diff color_words::     >>[38;5;2m<<log diff color_words added line_number::   2>>[39m<<log diff color_words::: >>[4m[38;5;2m<<log diff color_words added token::c>>[24m[39m
    [38;5;3m<<log diff color_words header::Modified regular file rename-target (rename-source => rename-target):>>[39m
    <<log::=== git ===>>
    [1m<<log diff git file_header::diff --git a/file1 b/file1>>[0m
    [1m<<log diff git file_header::index 422c2b7ab3..de980441c3 100644>>[0m
    [1m<<log diff git file_header::--- a/file1>>[0m
    [1m<<log diff git file_header::+++ b/file1>>[0m
    [38;5;6m<<log diff git hunk_header::@@ -1,2 +1,3 @@>>[39m
    <<log diff git context:: a>>
    <<log diff git context:: b>>
    [38;5;2m<<log diff git added::+>>[4m<<log diff git added token::c>>[24m[39m
    [1m<<log diff git file_header::diff --git a/file2 b/file2>>[0m
    [1m<<log diff git file_header::index 7898192261..9ddeb5c484 100644>>[0m
    [1m<<log diff git file_header::--- a/file2>>[0m
    [1m<<log diff git file_header::+++ b/file2>>[0m
    [38;5;6m<<log diff git hunk_header::@@ -1,1 +1,2 @@>>[39m
    [38;5;1m<<log diff git removed::->>[4m<<log diff git removed token::a>>[24m<<log diff git removed::>>[39m
    [38;5;2m<<log diff git added::+>>[4m<<log diff git added token::b>>[24m<<log diff git added::>>[39m
    [38;5;2m<<log diff git added::+>>[4m<<log diff git added token::c>>[24m[39m
    [1m<<log diff git file_header::diff --git a/rename-source b/rename-target>>[0m
    [1m<<log diff git file_header::rename from rename-source>>[0m
    [1m<<log diff git file_header::rename to rename-target>>[0m
    <<log::=== stat ===>>
    <<log diff stat::file1                            | 1 >>[38;5;2m<<log diff stat added::+>>[38;5;1m<<log diff stat removed::>>[39m
    <<log diff stat::file2                            | 3 >>[38;5;2m<<log diff stat added::++>>[38;5;1m<<log diff stat removed::->>[39m
    <<log diff stat::{rename-source => rename-target} | 0>>[38;5;1m<<log diff stat removed::>>[39m
    <<log diff stat stat-summary::3 files changed, 3 insertions(+), 1 deletion(-)>>
    <<log::=== summary ===>>
    [38;5;6m<<log diff summary modified::M file1>>[39m
    [38;5;6m<<log diff summary modified::M file2>>[39m
    [38;5;6m<<log diff summary renamed::R {rename-source => rename-target}>>[39m
    "###);

    // cwd != workspace root
    let stdout = test_env.jj_cmd_success(
        test_env.env_root(),
        &["log", "-Rrepo", "--no-graph", "-r@", "-T", template],
    );
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    === color_words ===
    Modified regular file repo/file1:
       1    1: a
       2    2: b
            3: c
    Modified regular file repo/file2:
       1    1: ab
            2: c
    Modified regular file repo/rename-target (repo/rename-source => repo/rename-target):
    === git ===
    diff --git a/file1 b/file1
    index 422c2b7ab3..de980441c3 100644
    --- a/file1
    +++ b/file1
    @@ -1,2 +1,3 @@
     a
     b
    +c
    diff --git a/file2 b/file2
    index 7898192261..9ddeb5c484 100644
    --- a/file2
    +++ b/file2
    @@ -1,1 +1,2 @@
    -a
    +b
    +c
    diff --git a/rename-source b/rename-target
    rename from rename-source
    rename to rename-target
    === stat ===
    repo/file1                            | 1 +
    repo/file2                            | 3 ++-
    repo/{rename-source => rename-target} | 0
    3 files changed, 3 insertions(+), 1 deletion(-)
    === summary ===
    M repo/file1
    M repo/file2
    R repo/{rename-source => rename-target}
    "###);

    // color_words() with parameters
    let template = "self.diff('file1').color_words(0)";
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r@", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file1:
        ...
            3: c
    "###);

    // git() with parameters
    let template = "self.diff('file1').git(1)";
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--no-graph", "-r@", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file1 b/file1
    index 422c2b7ab3..de980441c3 100644
    --- a/file1
    +++ b/file1
    @@ -2,1 +2,2 @@
     b
    +c
    "###);
}
