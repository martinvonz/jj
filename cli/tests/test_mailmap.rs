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

use crate::common::{get_stdout_string, TestEnvironment};

#[test]
fn test_mailmap() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    let mut mailmap = String::new();
    let mailmap_path = repo_path.join(".mailmap");
    let mut append_mailmap = move |extra| {
        mailmap.push_str(extra);
        std::fs::write(&mailmap_path, &mailmap).unwrap()
    };

    let run_as = |name: &str, email: &str, args: &[&str]| {
        test_env
            .jj_cmd(&repo_path, args)
            .env("JJ_USER", name)
            .env("JJ_EMAIL", email)
            .assert()
            .success()
    };

    append_mailmap("# test comment\n");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Test User <test.user@example.com>
    ◉
    "###);

    // Map an email address without any name change.
    run_as("Test Üser", "TeSt.UsEr@ExAmPlE.cOm", &["new"]);
    append_mailmap("<test.user@example.net> <test.user@example.com>\n");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Test Üser <test.user@example.net>
    ◉  Test User <test.user@example.net>
    ◉
    "###);

    // Map an email address to a new name.
    run_as("West User", "xest.user@example.com", &["new"]);
    append_mailmap("Fest User <xest.user@example.com>\n");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Fest User <xest.user@example.com>
    ◉  Test Üser <test.user@example.net>
    ◉  Test User <test.user@example.net>
    ◉
    "###);

    // Map an email address to a new name and email address.
    run_as("Pest User", "pest.user@example.com", &["new"]);
    append_mailmap("Best User <best.user@example.com> <pest.user@example.com>\n");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Best User <best.user@example.com>
    ◉  Fest User <xest.user@example.com>
    ◉  Test Üser <test.user@example.net>
    ◉  Test User <test.user@example.net>
    ◉
    "###);

    // Map an ambiguous email address using names for disambiguation.
    run_as("Rest User", "user@test", &["new"]);
    run_as("Vest User", "user@test", &["new"]);
    append_mailmap(
        &[
            "Jest User <jest.user@example.org> ReSt UsEr <UsEr@TeSt>\n",
            "Zest User <zest.user@example.org> vEsT uSeR <uSeR@tEsT>\n",
        ]
        .concat(),
    );

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Zest User <zest.user@example.org>
    ◉  Jest User <jest.user@example.org>
    ◉  Best User <best.user@example.com>
    ◉  Fest User <xest.user@example.com>
    ◉  Test Üser <test.user@example.net>
    ◉  Test User <test.user@example.net>
    ◉
    "###);

    // The `.mailmap` file in the current workspace’s @ commit should be used.
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "author", "--at-operation=@-"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Vest User <user@test>
    ◉  Rest User <user@test>
    ◉  Best User <best.user@example.com>
    ◉  Fest User <xest.user@example.com>
    ◉  Test Üser <test.user@example.net>
    ◉  Test User <test.user@example.net>
    ◉
    "###);

    // The `author(pattern)` revset function should find mapped committers.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "author", "-r", "author(substring-i:bEsT)"],
    );
    insta::assert_snapshot!(stdout, @r###"
    ◉  Best User <best.user@example.com>
    │
    ~
    "###);

    // The `author(pattern)` revset function should only search the mapped form.
    // This matches Git’s behaviour and the principle of not surfacing raw
    // signatures by default.
    let stdout =
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "author", "-r", "author(pest)"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);

    // The `author_raw(pattern)` revset function should search the unmapped
    // commit data.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "author", "-r", "author_raw(\"user@test\")"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  Zest User <zest.user@example.org>
    ◉  Jest User <jest.user@example.org>
    │
    ~
    "###);

    // `mine()` should find commits that map to the current `user.email`.
    let assert = run_as(
        "Tëst Üser",
        "tEsT.uSeR@eXaMpLe.NeT",
        &["log", "-T", "author", "-r", "mine()"],
    );
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    ◉  Test Üser <test.user@example.net>
    ◉  Test User <test.user@example.net>
    │
    ~
    "###);

    // `mine()` should only search the mapped author; this may be confusing in this
    // case, but matches the semantics of it expanding to `author(‹user.email›)`.
    let stdout: String =
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "author", "-r", "mine()"]);
    insta::assert_snapshot!(stdout, @r###"
    "###);
}
