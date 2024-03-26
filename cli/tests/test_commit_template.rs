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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    ◉ │  4db490c88528133d579540b6900b8098f0c17701
    ├─╯  P: 1 230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    │  P: 1 0000000000000000000000000000000000000000
    ◉  0000000000000000000000000000000000000000
       P: 0
    "###);

    let template = r#"parents.map(|c| c.commit_id().shortest(4))"#;
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", template, "-r@", "--color=always"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;4m4[0m[38;5;8mdb4[39m [1m[38;5;4m2[0m[38;5;8m30d[39m
    │
    ~
    "###);

    // Commit object isn't printable
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-T", "parents"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse template: Expected expression of type "Template"
    Caused by:  --> 1:1
      |
    1 | parents
      | ^-----^
      |
      = Expected expression of type "Template"
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-03 04:05:09.000 +07:00
    ◉  2001-02-03 04:05:07.000 +07:00
    ◉  1970-01-01 00:00:00.000 +00:00
    "###);
}

#[test]
fn test_log_author_timestamp_ago() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().utc()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-02 21:05:07.000 +00:00
    ◉  1970-01-01 00:00:00.000 +00:00
    "###);
}

#[cfg(unix)]
#[test]
fn test_log_author_timestamp_local() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.add_env_var("TZ", "UTC-05:30");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().local()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-03 08:05:07.000 +11:00
    ◉  1970-01-01 11:00:00.000 +11:00
    "###);
    test_env.add_env_var("TZ", "UTC+10:00");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author.timestamp().local()"]);
    insta::assert_snapshot!(stdout, @r###"
    @  2001-02-03 08:05:07.000 +11:00
    ◉  1970-01-01 11:00:00.000 +11:00
    "###);
}

#[test]
fn test_log_default() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m", "description 1"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "my-branch"]);

    // Test default log output format
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    @  kkmpptxz test.user@example.com 2001-02-03 08:05:09 my-branch 9de54178
    │  (empty) description 1
    ◉  qpvuntsm test.user@example.com 2001-02-03 08:05:08 4291e264
    │  add a file
    ◉  zzzzzzzz root() 00000000
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;13mk[38;5;8mkmpptxz[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:09[39m [38;5;13mmy-branch[39m [38;5;12m9[38;5;8mde54178[39m[0m
    │  [1m[38;5;10m(empty)[39m description 1[0m
    ◉  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:08[39m [1m[38;5;4m4[0m[38;5;8m291e264[39m
    │  add a file
    ◉  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);

    // Color without graph
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    [1m[38;5;13mk[38;5;8mkmpptxz[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:09[39m [38;5;13mmy-branch[39m [38;5;12m9[38;5;8mde54178[39m[0m
    [1m[38;5;10m(empty)[39m description 1[0m
    [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:08[39m [1m[38;5;4m4[0m[38;5;8m291e264[39m
    add a file
    [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);
}

#[test]
fn test_log_builtin_templates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "my-branch"]);

    insta::assert_snapshot!(render(r#"builtin_log_oneline"#), @r###"
    rlvkpnrz (no email set) 2001-02-03 08:05:08 my-branch dc315397 (empty) (no description set)
    qpvuntsm test.user 2001-02-03 08:05:07 230dd059 (empty) (no description set)
    zzzzzzzz root() 00000000
    [EOF]
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_compact"#), @r###"
    rlvkpnrz (no email set) 2001-02-03 08:05:08 my-branch dc315397
    (empty) (no description set)
    qpvuntsm test.user@example.com 2001-02-03 08:05:07 230dd059
    (empty) (no description set)
    zzzzzzzz root() 00000000
    [EOF]
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_comfortable"#), @r###"
    rlvkpnrz (no email set) 2001-02-03 08:05:08 my-branch dc315397
    (empty) (no description set)

    qpvuntsm test.user@example.com 2001-02-03 08:05:07 230dd059
    (empty) (no description set)

    zzzzzzzz root() 00000000

    [EOF]
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_detailed"#), @r###"
    Commit ID: dc31539712c7294d1d712cec63cef4504b94ca74
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Branches: my-branch
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "my-branch"]);

    insta::assert_snapshot!(render(r#"builtin_log_oneline"#), @r###"
    @  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;9m(no email set)[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;13mmy-branch[39m [38;5;12md[38;5;8mc315397[39m [38;5;10m(empty)[39m [38;5;10m(no description set)[39m[0m
    ◉  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    ◉  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_compact"#), @r###"
    @  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;9m(no email set)[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;13mmy-branch[39m [38;5;12md[38;5;8mc315397[39m[0m
    │  [1m[38;5;10m(empty)[39m [38;5;10m(no description set)[39m[0m
    ◉  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
    │  [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    ◉  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_comfortable"#), @r###"
    @  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;9m(no email set)[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;13mmy-branch[39m [38;5;12md[38;5;8mc315397[39m[0m
    │  [1m[38;5;10m(empty)[39m [38;5;10m(no description set)[39m[0m
    │
    ◉  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
    │  [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    │
    ◉  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);

    insta::assert_snapshot!(render(r#"builtin_log_detailed"#), @r###"
    @  Commit ID: [38;5;4mdc31539712c7294d1d712cec63cef4504b94ca74[39m
    │  Change ID: [38;5;5mrlvkpnrzqnoowoytxnquwvuryrwnrmlp[39m
    │  Branches: [38;5;5mmy-branch[39m
    │  Author: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m2001-02-03 08:05:08[39m)
    │  Committer: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m2001-02-03 08:05:08[39m)
    │
    │  [38;5;2m    (no description set)[39m
    │
    ◉  Commit ID: [38;5;4m230dd059e1b059aefc0da06a2e5a7dbf22362f22[39m
    │  Change ID: [38;5;5mqpvuntsmwlqtpsluzzsnyyzlmlwvmlnu[39m
    │  Author: Test User <[38;5;3mtest.user@example.com[39m> ([38;5;6m2001-02-03 08:05:07[39m)
    │  Committer: Test User <[38;5;3mtest.user@example.com[39m> ([38;5;6m2001-02-03 08:05:07[39m)
    │
    │  [38;5;2m    (no description set)[39m
    │
    ◉  Commit ID: [38;5;4m0000000000000000000000000000000000000000[39m
       Change ID: [38;5;5mzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz[39m
       Author: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m1970-01-01 11:00:00[39m)
       Committer: [38;5;1m(no name set)[39m <[38;5;1m(no email set)[39m> ([38;5;6m1970-01-01 11:00:00[39m)

       [38;5;2m    (no description set)[39m

    "###);
}

#[test]
fn test_log_obslog_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "description 1"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log"]);
    // No divergence
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm test.user@example.com 2001-02-03 08:05:08 7a17d52e
    │  description 1
    ◉  zzzzzzzz root() 00000000
    "###);

    // Create divergence
    test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "-m", "description 2", "--at-operation", "@-"],
    );
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  qpvuntsm?? test.user@example.com 2001-02-03 08:05:10 8979953d
    │  description 2
    │ @  qpvuntsm?? test.user@example.com 2001-02-03 08:05:08 7a17d52e
    ├─╯  description 1
    ◉  zzzzzzzz root() 00000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Concurrent modification detected, resolving automatically.
    "###);

    // Color
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉  [1m[4m[38;5;1mq[0m[38;5;1mpvuntsm??[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:10[39m [1m[38;5;4m8[0m[38;5;8m979953d[39m
    │  description 2
    │ @  [1m[4m[38;5;1mq[24mpvuntsm[38;5;9m??[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;12m7[38;5;8ma17d52e[39m[0m
    ├─╯  [1mdescription 1[0m
    ◉  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);

    // Obslog and hidden divergent
    let stdout = test_env.jj_cmd_success(&repo_path, &["obslog"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsm?? test.user@example.com 2001-02-03 08:05:08 7a17d52e
    │  description 1
    ◉  qpvuntsm hidden test.user@example.com 2001-02-03 08:05:08 3b68ce25
    │  (no description set)
    ◉  qpvuntsm hidden test.user@example.com 2001-02-03 08:05:07 230dd059
       (empty) (no description set)
    "###);

    // Colored obslog
    let stdout = test_env.jj_cmd_success(&repo_path, &["obslog", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[4m[38;5;1mq[24mpvuntsm[38;5;9m??[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:08[39m [38;5;12m7[38;5;8ma17d52e[39m[0m
    │  [1mdescription 1[0m
    ◉  [1m[39mq[0m[38;5;8mpvuntsm[39m hidden [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:08[39m [1m[38;5;4m3[0m[38;5;8mb68ce25[39m
    │  [38;5;3m(no description set)[39m
    ◉  [1m[39mq[0m[38;5;8mpvuntsm[39m hidden [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
       [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    "###);
}

#[test]
fn test_log_branches() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-branch = true");
    test_env.add_config(r#"revset-aliases."immutable_heads()" = "none()""#);

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "--git", "origin"]);
    let origin_path = test_env.env_root().join("origin");
    let origin_git_repo_path = origin_path
        .join(".jj")
        .join("repo")
        .join("store")
        .join("git");

    // Created some branches on the remote
    test_env.jj_cmd_ok(&origin_path, &["describe", "-m=description 1"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "branch1"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "root()", "-m=description 2"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "branch2", "unchanged"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "root()", "-m=description 3"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "branch3"]);
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

    // Rewrite branch1, move branch2 forward, create conflict in branch3, add
    // new-branch
    test_env.jj_cmd_ok(
        &workspace_root,
        &["describe", "branch1", "-m", "modified branch1 commit"],
    );
    test_env.jj_cmd_ok(&workspace_root, &["new", "branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "set", "branch2"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "new-branch"]);
    test_env.jj_cmd_ok(&workspace_root, &["describe", "branch3", "-m=local"]);
    test_env.jj_cmd_ok(&origin_path, &["describe", "branch3", "-m=origin"]);
    test_env.jj_cmd_ok(&origin_path, &["git", "export"]);
    test_env.jj_cmd_ok(&workspace_root, &["git", "fetch"]);

    let template = r#"commit_id.short() ++ " " ++ if(branches, branches, "(no branches)")"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ◉  fed794e2ba44 branch3?? branch3@origin
    │ ◉  b1bb3766d584 branch3??
    ├─╯
    │ ◉  21c33875443e branch1*
    ├─╯
    │ @  a5b4d15489cc branch2* new-branch
    │ ◉  8476341eb395 branch2@origin unchanged
    ├─╯
    ◉  000000000000 (no branches)
    "###);

    let template = r#"branches.map(|b| separate("/", b.remote(), b.name())).join(", ")"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ◉  branch3, origin/branch3
    │ ◉  branch3
    ├─╯
    │ ◉  branch1
    ├─╯
    │ @  branch2, new-branch
    │ ◉  origin/branch2, unchanged
    ├─╯
    ◉
    "###);

    let template = r#"separate(" ", "L:", local_branches, "R:", remote_branches)"#;
    let output = test_env.jj_cmd_success(&workspace_root, &["log", "-T", template]);
    insta::assert_snapshot!(output, @r###"
    ◉  L: branch3?? R: branch3@origin
    │ ◉  L: branch3?? R:
    ├─╯
    │ ◉  L: branch1* R:
    ├─╯
    │ @  L: branch2* new-branch R:
    │ ◉  L: unchanged R: branch2@origin unchanged@origin
    ├─╯
    ◉  L: R:
    "###);
}

#[test]
fn test_log_git_head() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);

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
    ◉  name: HEAD, remote: git
    ◉  remote: <Error: No RefName available>
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "--color=always"]);
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;13mr[38;5;8mlvkpnrz[39m [38;5;3mtest.user@example.com[39m [38;5;14m2001-02-03 08:05:09[39m [38;5;12m5[38;5;8m0aaf475[39m[0m
    │  [1minitial[0m
    ◉  [1m[38;5;5mq[0m[38;5;8mpvuntsm[39m [38;5;3mtest.user@example.com[39m [38;5;6m2001-02-03 08:05:07[39m [38;5;2mHEAD@git[39m [1m[38;5;4m2[0m[38;5;8m30dd059[39m
    │  [38;5;2m(empty)[39m [38;5;2m(no description set)[39m
    ◉  [1m[38;5;5mz[0m[38;5;8mzzzzzzz[39m [38;5;2mroot()[39m [1m[38;5;4m0[0m[38;5;8m0000000[39m
    "###);
}

#[test]
fn test_log_customize_short_id() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    @  Q_pvun test.user@example.com 2001-02-03 08:05:08 6_9542
    │  (empty) first
    ◉  Z_zzzz root() 0_0000
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
    @  QPVUNTSM test.user@example.com 2001-02-03 08:05:08 69542c19
    │  (empty) first
    ◉  ZZZZZZZZ root() 00000000
    "###);
}

#[test]
fn test_log_immutable() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["new", "-mA", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mB"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "main"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mC"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-mD", "root()"]);

    let template = r#"
    separate(" ",
      description.first_line(),
      branches,
      if(immutable, "[immutable]"),
    ) ++ "\n"
    "#;

    test_env.add_config("revset-aliases.'immutable_heads()' = 'main'");
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-r::", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @  D
    │ ◉  C
    │ ◉  B main [immutable]
    │ ◉  A [immutable]
    ├─╯
    ◉  [immutable]
    "###);

    // Suppress error that could be detected earlier
    test_env.add_config("revsets.short-prefixes = ''");

    test_env.add_config("revset-aliases.'immutable_heads()' = 'unknown_fn()'");
    let stderr = test_env.jj_cmd_failure(&repo_path, &["log", "-r::", "-T", template]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Failed to parse template: Failed to parse revset
    Caused by:
    1:  --> 5:10
      |
    5 |       if(immutable, "[immutable]"),
      |          ^-------^
      |
      = Failed to parse revset
    2:  --> 1:1
      |
    1 | unknown_fn()
      | ^--------^
      |
      = Function "unknown_fn" doesn't exist
    "###);

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
