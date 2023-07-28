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

use common::{get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_log_with_empty_revision() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["log", "-r="]);
    insta::assert_snapshot!(stderr, @r###"
    error: a value is required for '--revisions <REVISIONS>' but none was supplied

    For more information, try '--help'.
    "###);
}

#[test]
fn test_log_legacy_range_operator() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-r=@:"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsmwlqt test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
    │  (empty) (no description set)
    ~
    "###);
    insta::assert_snapshot!(stderr, @r###"
    The `:` revset operator is deprecated. Please switch to `::`.
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-r=:@"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsmwlqt test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
    │  (empty) (no description set)
    ◉  zzzzzzzzzzzz 1970-01-01 00:00:00.000 +00:00 000000000000
       (empty) (no description set)
    "###);
    insta::assert_snapshot!(stderr, @r###"
    The `:` revset operator is deprecated. Please switch to `::`.
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "-r=root:@"]);
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsmwlqt test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
    │  (empty) (no description set)
    ◉  zzzzzzzzzzzz 1970-01-01 00:00:00.000 +00:00 000000000000
       (empty) (no description set)
    "###);
    insta::assert_snapshot!(stderr, @r###"
    The `:` revset operator is deprecated. Please switch to `::`.
    "###);
    let (stdout, stderr) = test_env.jj_cmd_ok(
        &repo_path,
        &["log", "-r=x", "--config-toml", "revset-aliases.x = '@:'"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  qpvuntsmwlqt test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
    │  (empty) (no description set)
    ~
    "###);
    insta::assert_snapshot!(stderr, @r###"
    The `:` revset operator is deprecated. Please switch to `::`.
    "###);
}

#[test]
fn test_log_with_or_without_diff() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "a new commit"]);
    std::fs::write(repo_path.join("file1"), "foo\nbar\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    ◉  add a file
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-p"]);
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  Modified regular file file1:
    │     1    1: foo
    │          2: bar
    ◉  add a file
    │  Added regular file file1:
    │          1: foo
    ◉
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    add a file
    "###);

    // `-p` for default diff output, `-s` for summary
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-p", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  M file1
    │  Modified regular file file1:
    │     1    1: foo
    │          2: bar
    ◉  add a file
    │  A file1
    │  Added regular file file1:
    │          1: foo
    ◉
    "###);

    // `-s` for summary, `--git` for git diff (which implies `-p`)
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-s", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  M file1
    │  diff --git a/file1 b/file1
    │  index 257cc5642c...3bd1f0e297 100644
    │  --- a/file1
    │  +++ b/file1
    │  @@ -1,1 +1,2 @@
    │   foo
    │  +bar
    ◉  add a file
    │  A file1
    │  diff --git a/file1 b/file1
    │  new file mode 100644
    │  index 0000000000..257cc5642c
    │  --- /dev/null
    │  +++ b/file1
    │  @@ -1,0 +1,1 @@
    │  +foo
    ◉
    "###);

    // `-p` enables default "summary" output, so `-s` is noop
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            "description",
            "-p",
            "-s",
            "--config-toml=ui.diff.format='summary'",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  M file1
    ◉  add a file
    │  A file1
    ◉
    "###);

    // `-p` enables default "color-words" diff output, so `--color-words` is noop
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "-p", "--color-words"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  Modified regular file file1:
    │     1    1: foo
    │          2: bar
    ◉  add a file
    │  Added regular file file1:
    │          1: foo
    ◉
    "###);

    // `--git` enables git diff, so `-p` is noop
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "--no-graph", "-p", "--git"],
    );
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    diff --git a/file1 b/file1
    index 257cc5642c...3bd1f0e297 100644
    --- a/file1
    +++ b/file1
    @@ -1,1 +1,2 @@
     foo
    +bar
    add a file
    diff --git a/file1 b/file1
    new file mode 100644
    index 0000000000..257cc5642c
    --- /dev/null
    +++ b/file1
    @@ -1,0 +1,1 @@
    +foo
    "###);

    // Cannot use both `--git` and `--color-words`
    let stderr = test_env.jj_cmd_cli_error(
        &repo_path,
        &[
            "log",
            "-T",
            "description",
            "--no-graph",
            "-p",
            "--git",
            "--color-words",
        ],
    );
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--git' cannot be used with '--color-words'

    Usage: jj log --template <TEMPLATE> --no-graph --patch --git [PATHS]...

    For more information, try '--help'.
    "###);

    // `-s` with or without graph
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  M file1
    ◉  add a file
    │  A file1
    ◉
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "--no-graph", "-s"],
    );
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    M file1
    add a file
    A file1
    "###);

    // `--git` implies `-p`, with or without graph
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "-r", "@", "--git"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  diff --git a/file1 b/file1
    ~  index 257cc5642c...3bd1f0e297 100644
       --- a/file1
       +++ b/file1
       @@ -1,1 +1,2 @@
        foo
       +bar
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "-r", "@", "--no-graph", "--git"],
    );
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    diff --git a/file1 b/file1
    index 257cc5642c...3bd1f0e297 100644
    --- a/file1
    +++ b/file1
    @@ -1,1 +1,2 @@
     foo
    +bar
    "###);

    // `--color-words` implies `-p`, with or without graph
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "-r", "@", "--color-words"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @  a new commit
    │  Modified regular file file1:
    ~     1    1: foo
               2: bar
    "###);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            "description",
            "-r",
            "@",
            "--no-graph",
            "--color-words",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    Modified regular file file1:
       1    1: foo
            2: bar
    "###);
}

#[test]
fn test_log_shortest_accessors() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let render = |rev, template| {
        test_env.jj_cmd_success(
            &repo_path,
            &["log", "--no-graph", "-r", rev, "-T", template],
        )
    };
    test_env.add_config(
        r###"
        [template-aliases]
        'format_id(id)' = 'id.shortest(12).prefix() ++ "[" ++ id.shortest(12).rest() ++ "]"'
        "###,
    );

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c", "original"]);
    insta::assert_snapshot!(
        render("original", r#"format_id(change_id) ++ " " ++ format_id(commit_id)"#),
        @"q[pvuntsmwlqt] b[a1a30916d29]");

    // Create a chain of 10 commits
    for i in 1..10 {
        test_env.jj_cmd_success(&repo_path, &["new", "-m", &format!("commit{i}")]);
        std::fs::write(repo_path.join("file"), format!("file {i}\n")).unwrap();
    }
    // Create 2^3 duplicates of the chain
    for _ in 0..3 {
        test_env.jj_cmd_success(&repo_path, &["duplicate", "description(commit)"]);
    }

    insta::assert_snapshot!(
        render("original", r#"format_id(change_id) ++ " " ++ format_id(commit_id)"#),
        @"qpv[untsmwlqt] ba1[a30916d29]");

    insta::assert_snapshot!(
        render("::@", r#"change_id.shortest() ++ " " ++ commit_id.shortest() ++ "\n""#),
        @r###"
    wq 03
    km f7
    kp e7
    zn 38
    yo 0cf
    vr 9e
    yq 06
    ro 1f
    mz 7b
    qpv ba1
    zzz 00
    "###);

    insta::assert_snapshot!(
        render("::@", r#"format_id(change_id) ++ " " ++ format_id(commit_id) ++ "\n""#),
        @r###"
    wq[nwkozpkust] 03[f51310b83e]
    km[kuslswpqwq] f7[7fb1909080]
    kp[qxywonksrl] e7[15ad5db646]
    zn[kkpsqqskkl] 38[622e54e2e5]
    yo[stqsxwqrlt] 0cf[42f60199c]
    vr[uxwmqvtpmx] 9e[6015e4e622]
    yq[osqzytrlsw] 06[f34d9b1475]
    ro[yxmykxtrkr] 1f[99a5e19891]
    mz[vwutvlkqwt] 7b[1f7dee65b4]
    qpv[untsmwlqt] ba1[a30916d29]
    zzz[zzzzzzzzz] 00[0000000000]
    "###);

    // Can get shorter prefixes in configured revset
    test_env.add_config(r#"revsets.short-prefixes = "(@----):""#);
    insta::assert_snapshot!(
        render("::@", r#"format_id(change_id) ++ " " ++ format_id(commit_id) ++ "\n""#),
        @r###"
    w[qnwkozpkust] 03[f51310b83e]
    km[kuslswpqwq] f[77fb1909080]
    kp[qxywonksrl] e[715ad5db646]
    z[nkkpsqqskkl] 3[8622e54e2e5]
    y[ostqsxwqrlt] 0c[f42f60199c]
    vr[uxwmqvtpmx] 9e[6015e4e622]
    yq[osqzytrlsw] 06f[34d9b1475]
    ro[yxmykxtrkr] 1f[99a5e19891]
    mz[vwutvlkqwt] 7b[1f7dee65b4]
    qpv[untsmwlqt] ba1[a30916d29]
    zzz[zzzzzzzzz] 00[0000000000]
    "###);

    // Can disable short prefixes by setting to empty string
    test_env.add_config(r#"revsets.short-prefixes = """#);
    insta::assert_snapshot!(
        render("::@", r#"format_id(change_id) ++ " " ++ format_id(commit_id) ++ "\n""#),
        @r###"
    wq[nwkozpkust] 03[f51310b83e]
    km[kuslswpqwq] f7[7fb1909080]
    kp[qxywonksrl] e7[15ad5db646]
    zn[kkpsqqskkl] 38[622e54e2e5]
    yo[stqsxwqrlt] 0cf[42f60199c]
    vr[uxwmqvtpmx] 9e[6015e4e622]
    yq[osqzytrlsw] 06f[34d9b1475]
    ro[yxmykxtrkr] 1f[99a5e19891]
    mz[vwutvlkqwt] 7b[1f7dee65b4]
    qpv[untsmwlqt] ba1[a30916d29]
    zzz[zzzzzzzzz] 00[0000000000]
    "###);
}

#[test]
fn test_log_prefix_highlight_styled() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    fn prefix_format(len: Option<usize>) -> String {
        format!(
            r###"
            separate(" ",
              "Change",
              change_id.shortest({0}),
              description.first_line(),
              commit_id.shortest({0}),
              branches,
            )
            "###,
            len.map(|l| l.to_string()).unwrap_or_default()
        )
    }

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c", "original"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "original", "-T", &prefix_format(Some(12))]),
        @r###"
    @  Change qpvuntsmwlqt initial ba1a30916d29 original
    │
    ~
    "###
    );

    // Create a chain of 10 commits
    for i in 1..10 {
        test_env.jj_cmd_success(&repo_path, &["new", "-m", &format!("commit{i}")]);
        std::fs::write(repo_path.join("file"), format!("file {i}\n")).unwrap();
    }
    // Create 2^3 duplicates of the chain
    for _ in 0..3 {
        test_env.jj_cmd_success(&repo_path, &["duplicate", "description(commit)"]);
    }

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "original", "-T", &prefix_format(Some(12))]),
        @r###"
    ◉  Change qpvuntsmwlqt initial ba1a30916d29 original
    │
    ~
    "###
    );
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "--color=always",
            "log",
            "-r",
            "@-----------..@",
            "-T",
            &prefix_format(Some(12)),
        ],
    );
    insta::assert_snapshot!(stdout,
        @r###"
    @  Change [1m[38;5;5mwq[0m[38;5;8mnwkozpkust[39m commit9 [1m[38;5;4m03[0m[38;5;8mf51310b83e[39m
    ◉  Change [1m[38;5;5mkm[0m[38;5;8mkuslswpqwq[39m commit8 [1m[38;5;4mf7[0m[38;5;8m7fb1909080[39m
    ◉  Change [1m[38;5;5mkp[0m[38;5;8mqxywonksrl[39m commit7 [1m[38;5;4me7[0m[38;5;8m15ad5db646[39m
    ◉  Change [1m[38;5;5mzn[0m[38;5;8mkkpsqqskkl[39m commit6 [1m[38;5;4m38[0m[38;5;8m622e54e2e5[39m
    ◉  Change [1m[38;5;5myo[0m[38;5;8mstqsxwqrlt[39m commit5 [1m[38;5;4m0cf[0m[38;5;8m42f60199c[39m
    ◉  Change [1m[38;5;5mvr[0m[38;5;8muxwmqvtpmx[39m commit4 [1m[38;5;4m9e[0m[38;5;8m6015e4e622[39m
    ◉  Change [1m[38;5;5myq[0m[38;5;8mosqzytrlsw[39m commit3 [1m[38;5;4m06[0m[38;5;8mf34d9b1475[39m
    ◉  Change [1m[38;5;5mro[0m[38;5;8myxmykxtrkr[39m commit2 [1m[38;5;4m1f[0m[38;5;8m99a5e19891[39m
    ◉  Change [1m[38;5;5mmz[0m[38;5;8mvwutvlkqwt[39m commit1 [1m[38;5;4m7b[0m[38;5;8m1f7dee65b4[39m
    ◉  Change [1m[38;5;5mqpv[0m[38;5;8muntsmwlqt[39m initial [1m[38;5;4mba1[0m[38;5;8ma30916d29[39m [38;5;5moriginal[39m
    ◉  Change [1m[38;5;5mzzz[0m[38;5;8mzzzzzzzzz[39m [1m[38;5;4m00[0m[38;5;8m0000000000[39m
    "###
    );
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "--color=always",
            "log",
            "-r",
            "@-----------..@",
            "-T",
            &prefix_format(Some(3)),
        ],
    );
    insta::assert_snapshot!(stdout,
        @r###"
    @  Change [1m[38;5;5mwq[0m[38;5;8mn[39m commit9 [1m[38;5;4m03[0m[38;5;8mf[39m
    ◉  Change [1m[38;5;5mkm[0m[38;5;8mk[39m commit8 [1m[38;5;4mf7[0m[38;5;8m7[39m
    ◉  Change [1m[38;5;5mkp[0m[38;5;8mq[39m commit7 [1m[38;5;4me7[0m[38;5;8m1[39m
    ◉  Change [1m[38;5;5mzn[0m[38;5;8mk[39m commit6 [1m[38;5;4m38[0m[38;5;8m6[39m
    ◉  Change [1m[38;5;5myo[0m[38;5;8ms[39m commit5 [1m[38;5;4m0cf[0m
    ◉  Change [1m[38;5;5mvr[0m[38;5;8mu[39m commit4 [1m[38;5;4m9e[0m[38;5;8m6[39m
    ◉  Change [1m[38;5;5myq[0m[38;5;8mo[39m commit3 [1m[38;5;4m06[0m[38;5;8mf[39m
    ◉  Change [1m[38;5;5mro[0m[38;5;8my[39m commit2 [1m[38;5;4m1f[0m[38;5;8m9[39m
    ◉  Change [1m[38;5;5mmz[0m[38;5;8mv[39m commit1 [1m[38;5;4m7b[0m[38;5;8m1[39m
    ◉  Change [1m[38;5;5mqpv[0m initial [1m[38;5;4mba1[0m [38;5;5moriginal[39m
    ◉  Change [1m[38;5;5mzzz[0m [1m[38;5;4m00[0m[38;5;8m0[39m
    "###
    );
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "--color=always",
            "log",
            "-r",
            "@-----------..@",
            "-T",
            &prefix_format(None),
        ],
    );
    insta::assert_snapshot!(stdout,
        @r###"
    @  Change [1m[38;5;5mwq[0m commit9 [1m[38;5;4m03[0m
    ◉  Change [1m[38;5;5mkm[0m commit8 [1m[38;5;4mf7[0m
    ◉  Change [1m[38;5;5mkp[0m commit7 [1m[38;5;4me7[0m
    ◉  Change [1m[38;5;5mzn[0m commit6 [1m[38;5;4m38[0m
    ◉  Change [1m[38;5;5myo[0m commit5 [1m[38;5;4m0cf[0m
    ◉  Change [1m[38;5;5mvr[0m commit4 [1m[38;5;4m9e[0m
    ◉  Change [1m[38;5;5myq[0m commit3 [1m[38;5;4m06[0m
    ◉  Change [1m[38;5;5mro[0m commit2 [1m[38;5;4m1f[0m
    ◉  Change [1m[38;5;5mmz[0m commit1 [1m[38;5;4m7b[0m
    ◉  Change [1m[38;5;5mqpv[0m initial [1m[38;5;4mba1[0m [38;5;5moriginal[39m
    ◉  Change [1m[38;5;5mzzz[0m [1m[38;5;4m00[0m
    "###
    );
}

#[test]
fn test_log_prefix_highlight_counts_hidden_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.add_config(
        r###"
        [revsets]
        short-prefixes = "" # Disable short prefixes
        [template-aliases]
        'format_id(id)' = 'id.shortest(12).prefix() ++ "[" ++ id.shortest(12).rest() ++ "]"'
        "###,
    );

    let prefix_format = r###"
    separate(" ",
      "Change",
      format_id(change_id),
      description.first_line(),
      format_id(commit_id),
      branches,
    )
    "###;

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c", "original"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", prefix_format]),
        @r###"
    @  Change q[pvuntsmwlqt] initial b[a1a30916d29] original
    ◉  Change z[zzzzzzzzzzz] 0[00000000000]
    "###
    );

    // Create 2^7 hidden commits
    test_env.jj_cmd_success(&repo_path, &["new", "root", "-m", "extra"]);
    for _ in 0..7 {
        test_env.jj_cmd_success(&repo_path, &["duplicate", "description(extra)"]);
    }
    test_env.jj_cmd_success(&repo_path, &["abandon", "description(extra)"]);

    // The unique prefixes became longer.
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", prefix_format]),
        @r###"
    @  Change w[qnwkozpkust] 44[4c3c5066d3]
    │ ◉  Change q[pvuntsmwlqt] initial ba[1a30916d29] original
    ├─╯
    ◉  Change z[zzzzzzzzzzz] 00[0000000000]
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["log", "-r", "4", "-T", prefix_format]),
        @r###"
    Error: Commit ID prefix "4" is ambiguous
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "44", "-T", prefix_format]),
        @r###"
    @  Change w[qnwkozpkust] 44[4c3c5066d3]
    │
    ~
    "###
    );
}

#[test]
fn test_log_shortest_length_parameter() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id.shortest(0)"]), @r###"
    @  2
    ◉  0
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "commit_id.shortest(100)"]), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_log_author_format() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "--revisions=@"]),
        @r###"
    @  qpvuntsmwlqt test.user@example.com 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
    │  (empty) (no description set)
    ~
    "###
    );

    let decl = "template-aliases.'format_short_signature(signature)'";
    insta::assert_snapshot!(
        test_env.jj_cmd_success(
            &repo_path,
            &[
                "--config-toml",
                &format!("{decl}='signature.username()'"),
                "log",
                "--revisions=@",
            ],
        ),
        @r###"
    @  qpvuntsmwlqt test.user 2001-02-03 04:05:07.000 +07:00 230dd059e1b0
    │  (empty) (no description set)
    ~
    "###
    );
}

#[test]
fn test_log_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    let template = r#"description.first_line() ++ if(divergent, " !divergence!")"#;

    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description 1"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    // No divergence
    insta::assert_snapshot!(stdout, @r###"
    @  description 1
    ◉
    "###);

    // Create divergence
    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "description 2", "--at-operation", "@-"],
    );
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    ◉  description 2 !divergence!
    │ @  description 1 !divergence!
    ├─╯
    ◉
    "###);
}

#[test]
fn test_log_reversed() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "--reversed"]);
    insta::assert_snapshot!(stdout, @r###"
    ◉
    ◉  first
    @  second
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "--reversed", "--no-graph"],
    );
    insta::assert_snapshot!(stdout, @r###"
    first
    second
    "###);
}

#[test]
fn test_log_filtered_by_path() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "first"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "second"]);
    std::fs::write(repo_path.join("file1"), "foo\nbar\n").unwrap();
    std::fs::write(repo_path.join("file2"), "baz\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    @  second
    ◉  first
    │
    ~
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    @  second
    │
    ~
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-s", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    @  second
    │  M file1
    ◉  first
    │  A file1
    ~
    "###);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "-s", "file2", "--no-graph"],
    );
    insta::assert_snapshot!(stdout, @r###"
    second
    A file2
    "###);

    // file() revset doesn't filter the diff.
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            "description",
            "-s",
            "-rfile(file2)",
            "--no-graph",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    second
    M file1
    A file2
    "###);
}

#[test]
fn test_log_warn_path_might_be_revset() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();

    // Don't warn if the file actually exists.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "file1", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @
    │
    ~
    "###);
    insta::assert_snapshot!(stderr, @"");

    // Warn for `jj log .` specifically, for former Mercurial users.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", ".", "-T", "description"]);
    insta::assert_snapshot!(stdout, @r###"
    @
    │
    ~
    "###);
    insta::assert_snapshot!(stderr, @r###"
    warning: The argument "." is being interpreted as a path, but this is often not useful because all non-empty commits touch '.'.  If you meant to show the working copy commit, pass -r '@' instead.
    "###);

    // ...but checking `jj log .` makes sense in a subdirectory.
    let subdir = repo_path.join("dir");
    std::fs::create_dir_all(&subdir).unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&subdir, &["log", "."]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");

    // Warn for `jj log @` instead of `jj log -r @`.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "@", "-T", "description"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    warning: The argument "@" is being interpreted as a path. To specify a revset, pass -r "@" instead.
    "###);

    // Warn when there's no path with the provided name.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["log", "file2", "-T", "description"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    warning: The argument "file2" is being interpreted as a path. To specify a revset, pass -r "file2" instead.
    "###);

    // If an explicit revision is provided, then suppress the warning.
    let (stdout, stderr) =
        test_env.jj_cmd_ok(&repo_path, &["log", "@", "-r", "@", "-T", "description"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    "###);
}

#[test]
fn test_default_revset() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);

    // Set configuration to only show the root commit.
    test_env.add_config(r#"revsets.log = "root""#);

    // Log should only contain one line (for the root commit), and not show the
    // commit created above.
    assert_eq!(
        1,
        test_env
            .jj_cmd_success(&repo_path, &["log", "-T", "commit_id"])
            .lines()
            .count()
    );
}

#[test]
fn test_default_revset_per_repo() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "add a file"]);

    // Set configuration to only show the root commit.
    std::fs::write(
        repo_path.join(".jj/repo/config.toml"),
        r#"revsets.log = "root""#,
    )
    .unwrap();

    // Log should only contain one line (for the root commit), and not show the
    // commit created above.
    assert_eq!(
        1,
        test_env
            .jj_cmd_success(&repo_path, &["log", "-T", "commit_id"])
            .lines()
            .count()
    );
}

#[test]
fn test_multiple_revsets() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    for name in ["foo", "bar", "baz"] {
        test_env.jj_cmd_success(&repo_path, &["new", "-m", name]);
        test_env.jj_cmd_success(&repo_path, &["branch", "set", name]);
    }

    // Default revset should be overridden if one or more -r options are specified.
    test_env.add_config(r#"revsets.log = "root""#);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "branches", "-rfoo"]),
        @r###"
    ◉  foo
    │
    ~
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "branches", "-rfoo", "-rbar", "-rbaz"]),
        @r###"
    @  baz
    ◉  bar
    ◉  foo
    │
    ~
    "###);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", "branches", "-rfoo", "-rfoo"]),
        @r###"
    ◉  foo
    │
    ~
    "###);
}

#[test]
fn test_graph_template_color() {
    // Test that color codes from a multi-line template don't span the graph lines.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "first line\nsecond line\nthird line"],
    );
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "single line"]);

    test_env.add_config(
        r#"[colors]
        description = "red"
        "working_copy description" = "green"
        "#,
    );

    // First test without color for comparison
    let template = r#"label(if(current_working_copy, "working_copy"), description)"#;
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @  single line
    ◉  first line
    │  second line
    │  third line
    ◉
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["--color=always", "log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @  [1m[38;5;2msingle line[0m
    ◉  [38;5;1mfirst line[39m
    │  [38;5;1msecond line[39m
    │  [38;5;1mthird line[39m
    ◉
    "###);
}

#[test]
fn test_graph_styles() {
    // Test that different graph styles are available.
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "main branch 1"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "main branch 2"]);
    test_env.jj_cmd_success(
        &repo_path,
        &["new", "-m", "side branch\nwith\nlong\ndescription"],
    );
    test_env.jj_cmd_success(
        &repo_path,
        &["new", "-m", "merge", r#"description("main branch 1")"#, "@"],
    );

    // Default (legacy) style
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    merge
    ├─╮
    │ ◉  side branch
    │ │  with
    │ │  long
    │ │  description
    │ ◉  main branch 2
    ├─╯
    ◉  main branch 1
    ◉  initial
    ◉
    "###);

    // ASCII style
    test_env.add_config(r#"ui.graph.style = "ascii""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    merge
    |\
    | o  side branch
    | |  with
    | |  long
    | |  description
    | o  main branch 2
    |/
    o  main branch 1
    o  initial
    o
    "###);

    // Large ASCII style
    test_env.add_config(r#"ui.graph.style = "ascii-large""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @     merge
    |\
    | \
    |  o  side branch
    |  |  with
    |  |  long
    |  |  description
    |  o  main branch 2
    | /
    |/
    o  main branch 1
    o  initial
    o
    "###);

    // Curved style
    test_env.add_config(r#"ui.graph.style = "curved""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    merge
    ├─╮
    │ ◉  side branch
    │ │  with
    │ │  long
    │ │  description
    │ ◉  main branch 2
    ├─╯
    ◉  main branch 1
    ◉  initial
    ◉
    "###);

    // Square style
    test_env.add_config(r#"ui.graph.style = "square""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    merge
    ├─┐
    │ ◉  side branch
    │ │  with
    │ │  long
    │ │  description
    │ ◉  main branch 2
    ├─┘
    ◉  main branch 1
    ◉  initial
    ◉
    "###);
}

#[test]
fn test_log_word_wrap() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
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

    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "main branch 1"]);
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "main branch 2"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "side"]);
    test_env.jj_cmd_success(&repo_path, &["new", "-m", "merge", "@--", "@"]);

    // ui.log-word-wrap option applies to both graph/no-graph outputs
    insta::assert_snapshot!(render(&["log", "-r@"], 40, false), @r###"
    @  mzvwutvlkqwt test.user@example.com 2001-02-03 04:05:11.000 +07:00 68518a7e6c9e
    │  (empty) merge
    ~
    "###);
    insta::assert_snapshot!(render(&["log", "-r@"], 40, true), @r###"
    @  mzvwutvlkqwt test.user@example.com
    │  2001-02-03 04:05:11.000 +07:00
    ~  68518a7e6c9e
       (empty) merge
    "###);
    insta::assert_snapshot!(render(&["log", "--no-graph", "-r@"], 40, false), @r###"
    mzvwutvlkqwt test.user@example.com 2001-02-03 04:05:11.000 +07:00 68518a7e6c9e
    (empty) merge
    "###);
    insta::assert_snapshot!(render(&["log", "--no-graph", "-r@"], 40, true), @r###"
    mzvwutvlkqwt test.user@example.com
    2001-02-03 04:05:11.000 +07:00
    68518a7e6c9e
    (empty) merge
    "###);

    // Color labels should be preserved
    insta::assert_snapshot!(render(&["log", "-r@", "--color=always"], 40, true), @r###"
    @  [1m[38;5;13mm[38;5;8mzvwutvlkqwt[39m [38;5;3mtest.user@example.com[39m[0m
    │  [1m[38;5;14m2001-02-03 04:05:11.000 +07:00[39m[0m
    ~  [1m[38;5;12m6[38;5;8m8518a7e6c9e[39m[0m
       [1m[38;5;10m(empty)[39m merge[0m
    "###);

    // Graph width should be subtracted from the term width
    let template = r#""0 1 2 3 4 5 6 7 8 9""#;
    insta::assert_snapshot!(render(&["log", "-T", template], 10, true), @r###"
    @    0 1 2
    ├─╮  3 4 5
    │ │  6 7 8
    │ │  9
    │ ◉  0 1 2
    │ │  3 4 5
    │ │  6 7 8
    │ │  9
    │ ◉  0 1 2
    ├─╯  3 4 5
    │    6 7 8
    │    9
    ◉  0 1 2 3
    │  4 5 6 7
    │  8 9
    ◉  0 1 2 3
       4 5 6 7
       8 9
    "###);
    insta::assert_snapshot!(
        render(&["log", "-T", template, "--config-toml=ui.graph.style='legacy'"], 9, true),
        @r###"
    @   0 1 2
    |\  3 4 5
    | | 6 7 8
    | | 9
    | o 0 1 2
    | | 3 4 5
    | | 6 7 8
    | | 9
    | o 0 1 2
    |/  3 4 5
    |   6 7 8
    |   9
    o 0 1 2 3
    | 4 5 6 7
    | 8 9
    o 0 1 2 3
      4 5 6 7
      8 9
    "###);

    // Shouldn't panic with $COLUMNS < graph_width
    insta::assert_snapshot!(render(&["log", "-r@"], 0, true), @r###"
    @  mzvwutvlkqwt
    │  test.user@example.com
    ~  2001-02-03
       04:05:11.000
       +07:00
       68518a7e6c9e
       (empty)
       merge
    "###);
    insta::assert_snapshot!(render(&["log", "-r@"], 1, true), @r###"
    @  mzvwutvlkqwt
    │  test.user@example.com
    ~  2001-02-03
       04:05:11.000
       +07:00
       68518a7e6c9e
       (empty)
       merge
    "###);
}
