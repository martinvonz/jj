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

use common::{get_stderr_string, get_stdout_string, TestEnvironment};

pub mod common;

#[test]
fn test_log_with_empty_revision() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["log", "-r="]);
    insta::assert_snapshot!(stderr, @r###"
    error: The argument '--revisions <REVISIONS>' requires a value but none was supplied

    For more information try '--help'
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
    @ a new commit
    o add a file
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-p"]);
    insta::assert_snapshot!(stdout, @r###"
    @ a new commit
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    o add a file
    | Added regular file file1:
    |         1: foo
    o 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "--no-graph"]);
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    add a file
    "###);

    // `-p` for default diff output, `-s` for summary
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-p", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @ a new commit
    | M file1
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    o add a file
    | A file1
    | Added regular file file1:
    |         1: foo
    o 
    "###);

    // `-s` for summary, `--git` for git diff (which implies `-p`)
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-s", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    @ a new commit
    | M file1
    | diff --git a/file1 b/file1
    | index 257cc5642c...3bd1f0e297 100644
    | --- a/file1
    | +++ b/file1
    | @@ -1,1 +1,2 @@
    |  foo
    | +bar
    o add a file
    | A file1
    | diff --git a/file1 b/file1
    | new file mode 100644
    | index 0000000000..257cc5642c
    | --- /dev/null
    | +++ b/file1
    | @@ -1,0 +1,1 @@
    | +foo
    o 
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
    @ a new commit
    | M file1
    o add a file
    | A file1
    o 
    "###);

    // `-p` enables default "color-words" diff output, so `--color-words` is noop
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-T", "description", "-p", "--color-words"],
    );
    insta::assert_snapshot!(stdout, @r###"
    @ a new commit
    | Modified regular file file1:
    |    1    1: foo
    |         2: bar
    o add a file
    | Added regular file file1:
    |         1: foo
    o 
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

    // Both formats enabled if `--git` and `--color-words` are explicitly specified
    let stdout = test_env.jj_cmd_success(
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
    insta::assert_snapshot!(stdout, @r###"
    a new commit
    diff --git a/file1 b/file1
    index 257cc5642c...3bd1f0e297 100644
    --- a/file1
    +++ b/file1
    @@ -1,1 +1,2 @@
     foo
    +bar
    Modified regular file file1:
       1    1: foo
            2: bar
    add a file
    diff --git a/file1 b/file1
    new file mode 100644
    index 0000000000..257cc5642c
    --- /dev/null
    +++ b/file1
    @@ -1,0 +1,1 @@
    +foo
    Added regular file file1:
            1: foo
    "###);

    // `-s` with or without graph
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    @ a new commit
    | M file1
    o add a file
    | A file1
    o 
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
    @ a new commit
    ~ diff --git a/file1 b/file1
      index 257cc5642c...3bd1f0e297 100644
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
    @ a new commit
    ~ Modified regular file file1:
         1    1: foo
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
fn test_log_prefix_highlight_brackets() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    fn prefix_format(len: Option<usize>) -> String {
        format!(
            r#"
      "Change " change_id.shortest_prefix_and_brackets({0}) " " description.first_line()
      " " commit_id.shortest_prefix_and_brackets({0}) " " branches
    "#,
            len.map(|l| l.to_string()).unwrap_or(String::default())
        )
    }

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c", "original"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "original", "-T", &prefix_format(Some(12))]),
        @r###"
    @ Change 9[a45c67d3e96] initial b[a1a30916d29] original
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
    o Change 9a4[5c67d3e96] initial ba1[a30916d29] original
    ~ 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", ":@", "-T", &prefix_format(Some(12))]),
        @r###"
    @ Change 39c[3fb0af576] commit9 03f[51310b83e] 
    o Change fd[f57e73a939] commit8 f7[7fb1909080] 
    o Change fa9[213bcf78e] commit7 e7[15ad5db646] 
    o Change 0cff[a7997ffe] commit6 38[622e54e2e5] 
    o Change 1b[76972398e6] commit5 0cf4[2f60199c] 
    o Change 48[523d946ad2] commit4 9e[6015e4e622] 
    o Change 19[b790168e73] commit3 06f[34d9b1475] 
    o Change 8b12[d1f268f8] commit2 1f[99a5e19891] 
    o Change d0[43564ef936] commit1 7b[1f7dee65b4] 
    o Change 9a4[5c67d3e96] initial ba1[a30916d29] original
    o Change 000[000000000]  000[000000000] 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", ":@", "-T", &prefix_format(Some(3))]),
        @r###"
    @ Change 39c commit9 03f 
    o Change fd[f] commit8 f7[7] 
    o Change fa9 commit7 e7[1] 
    o Change 0cff commit6 38[6] 
    o Change 1b[7] commit5 0cf4 
    o Change 48[5] commit4 9e[6] 
    o Change 19[b] commit3 06f 
    o Change 8b12 commit2 1f[9] 
    o Change d0[4] commit1 7b[1] 
    o Change 9a4 initial ba1 original
    o Change 000  000 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", ":@", "-T", &prefix_format(None)]),
        @r###"
    @ Change 39c commit9 03f 
    o Change fd commit8 f7 
    o Change fa9 commit7 e7 
    o Change 0cff commit6 38 
    o Change 1b commit5 0cf4 
    o Change 48 commit4 9e 
    o Change 19 commit3 06f 
    o Change 8b12 commit2 1f 
    o Change d0 commit1 7b 
    o Change 9a4 initial ba1 original
    o Change 000  000 
    "###
    );
}

#[test]
fn test_log_prefix_highlight_styled() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    fn prefix_format(len: Option<usize>) -> String {
        format!(
            r#"
      "Change " change_id.shortest({0}) " " description.first_line()
      " " commit_id.shortest({0}) " " branches
    "#,
            len.map(|l| l.to_string()).unwrap_or(String::default())
        )
    }

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c", "original"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "original", "-T", &prefix_format(Some(12))]),
        @r###"
    @ Change 9a45c67d3e96 initial ba1a30916d29 original
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
    o Change 9a45c67d3e96 initial ba1a30916d29 original
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
    @ Change [1m[38;5;5m39c[0m[38;5;8m3fb0af576[39m commit9 [1m[38;5;4m03f[0m[38;5;8m51310b83e[39m 
    o Change [1m[38;5;5mfd[0m[38;5;8mf57e73a939[39m commit8 [1m[38;5;4mf7[0m[38;5;8m7fb1909080[39m 
    o Change [1m[38;5;5mfa9[0m[38;5;8m213bcf78e[39m commit7 [1m[38;5;4me7[0m[38;5;8m15ad5db646[39m 
    o Change [1m[38;5;5m0cff[0m[38;5;8ma7997ffe[39m commit6 [1m[38;5;4m38[0m[38;5;8m622e54e2e5[39m 
    o Change [1m[38;5;5m1b[0m[38;5;8m76972398e6[39m commit5 [1m[38;5;4m0cf4[0m[38;5;8m2f60199c[39m 
    o Change [1m[38;5;5m48[0m[38;5;8m523d946ad2[39m commit4 [1m[38;5;4m9e[0m[38;5;8m6015e4e622[39m 
    o Change [1m[38;5;5m19[0m[38;5;8mb790168e73[39m commit3 [1m[38;5;4m06f[0m[38;5;8m34d9b1475[39m 
    o Change [1m[38;5;5m8b12[0m[38;5;8md1f268f8[39m commit2 [1m[38;5;4m1f[0m[38;5;8m99a5e19891[39m 
    o Change [1m[38;5;5md0[0m[38;5;8m43564ef936[39m commit1 [1m[38;5;4m7b[0m[38;5;8m1f7dee65b4[39m 
    o Change [1m[38;5;5m9a4[0m[38;5;8m5c67d3e96[39m initial [1m[38;5;4mba1[0m[38;5;8ma30916d29[39m [38;5;5moriginal[39m
    o Change [1m[38;5;5m000[0m[38;5;8m000000000[39m  [1m[38;5;4m000[0m[38;5;8m000000000[39m 
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
    @ Change [1m[38;5;5m39c[0m commit9 [1m[38;5;4m03f[0m 
    o Change [1m[38;5;5mfd[0m[38;5;8mf[39m commit8 [1m[38;5;4mf7[0m[38;5;8m7[39m 
    o Change [1m[38;5;5mfa9[0m commit7 [1m[38;5;4me7[0m[38;5;8m1[39m 
    o Change [1m[38;5;5m0cff[0m commit6 [1m[38;5;4m38[0m[38;5;8m6[39m 
    o Change [1m[38;5;5m1b[0m[38;5;8m7[39m commit5 [1m[38;5;4m0cf4[0m 
    o Change [1m[38;5;5m48[0m[38;5;8m5[39m commit4 [1m[38;5;4m9e[0m[38;5;8m6[39m 
    o Change [1m[38;5;5m19[0m[38;5;8mb[39m commit3 [1m[38;5;4m06f[0m 
    o Change [1m[38;5;5m8b12[0m commit2 [1m[38;5;4m1f[0m[38;5;8m9[39m 
    o Change [1m[38;5;5md0[0m[38;5;8m4[39m commit1 [1m[38;5;4m7b[0m[38;5;8m1[39m 
    o Change [1m[38;5;5m9a4[0m initial [1m[38;5;4mba1[0m [38;5;5moriginal[39m
    o Change [1m[38;5;5m000[0m  [1m[38;5;4m000[0m 
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
    @ Change [1m[38;5;5m39c[0m commit9 [1m[38;5;4m03f[0m 
    o Change [1m[38;5;5mfd[0m commit8 [1m[38;5;4mf7[0m 
    o Change [1m[38;5;5mfa9[0m commit7 [1m[38;5;4me7[0m 
    o Change [1m[38;5;5m0cff[0m commit6 [1m[38;5;4m38[0m 
    o Change [1m[38;5;5m1b[0m commit5 [1m[38;5;4m0cf4[0m 
    o Change [1m[38;5;5m48[0m commit4 [1m[38;5;4m9e[0m 
    o Change [1m[38;5;5m19[0m commit3 [1m[38;5;4m06f[0m 
    o Change [1m[38;5;5m8b12[0m commit2 [1m[38;5;4m1f[0m 
    o Change [1m[38;5;5md0[0m commit1 [1m[38;5;4m7b[0m 
    o Change [1m[38;5;5m9a4[0m initial [1m[38;5;4mba1[0m [38;5;5moriginal[39m
    o Change [1m[38;5;5m000[0m  [1m[38;5;4m000[0m 
    "###
    );
}

#[test]
fn test_log_prefix_highlight_counts_hidden_commits() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let prefix_format = r#"
      "Change " change_id.shortest_prefix_and_brackets(12) " " description.first_line()
      " " commit_id.shortest_prefix_and_brackets(12) " " branches
    "#;

    std::fs::write(repo_path.join("file"), "original file\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "initial"]);
    test_env.jj_cmd_success(&repo_path, &["branch", "c", "original"]);
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", prefix_format]),
        @r###"
    @ Change 9[a45c67d3e96] initial b[a1a30916d29] original
    o Change 0[00000000000]  0[00000000000] 
    "###
    );

    // Create 2^7 hidden commits
    test_env.jj_cmd_success(&repo_path, &["new", "root", "-m", "extra"]);
    for _ in 0..7 {
        test_env.jj_cmd_success(&repo_path, &["duplicate", "description(extra)"]);
    }
    test_env.jj_cmd_success(&repo_path, &["abandon", "description(extra)"]);

    // The first commit's unique prefix became longer.
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-T", prefix_format]),
        @r###"
    @ Change 39c3[fb0af576]  44[4c3c5066d3] 
    | o Change 9a[45c67d3e96] initial ba[1a30916d29] original
    |/  
    o Change 00[0000000000]  00[0000000000] 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["log", "-r", "d", "-T", prefix_format]),
        @r###"
    Error: Commit or change id prefix "d" is ambiguous
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "d0", "-T", prefix_format]),
        @r###"
    o Change a70[78fc7d293] extra d0[947f34cec4] 
    ~ 
    "###
    );
}

#[test]
fn test_log_divergence() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "description 1"]);
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            r#"description.first_line() if(divergent, " !divergence!")"#,
        ],
    );
    // No divergence
    insta::assert_snapshot!(stdout, @r###"
    @ description 1
    o 
    "###);

    // Create divergence
    test_env.jj_cmd_success(
        &repo_path,
        &["describe", "-m", "description 2", "--at-operation", "@-"],
    );
    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-T",
            r#"description.first_line() if(divergent, " !divergence!")"#,
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    Concurrent modification detected, resolving automatically.
    o description 2 !divergence!
    | @ description 1 !divergence!
    |/  
    o 
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
    o 
    o first
    @ second
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
    @ second
    o first
    ~ 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "file2"]);
    insta::assert_snapshot!(stdout, @r###"
    @ second
    ~ 
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T", "description", "-s", "file1"]);
    insta::assert_snapshot!(stdout, @r###"
    @ second
    | M file1
    o first
    ~ A file1
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
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "file1", "-T", "description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    @ 
    ~ 
    "###);
    insta::assert_snapshot!(get_stderr_string(&assert), @"");

    // Warn for `jj log .` specifically, for former Mercurial users.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", ".", "-T", "description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @r###"
    @ 
    ~ 
    "###);
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"warning: The argument "." is being interpreted as a path, but this is often not useful because all non-empty commits touch '.'.  If you meant to show the working copy commit, pass -r '@' instead."###);

    // ...but checking `jj log .` makes sense in a subdirectory.
    let subdir = repo_path.join("dir");
    std::fs::create_dir_all(&subdir).unwrap();
    let assert = test_env.jj_cmd(&subdir, &["log", "."]).assert().success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @"");

    // Warn for `jj log @` instead of `jj log -r @`.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "@", "-T", "description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    warning: The argument "@" is being interpreted as a path. To specify a revset, pass -r "@" instead.
    "###);

    // Warn when there's no path with the provided name.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "file2", "-T", "description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    warning: The argument "file2" is being interpreted as a path. To specify a revset, pass -r "file2" instead.
    "###);

    // If an explicit revision is provided, then suppress the warning.
    let assert = test_env
        .jj_cmd(&repo_path, &["log", "@", "-r", "@", "-T", "description"])
        .assert()
        .success();
    insta::assert_snapshot!(get_stdout_string(&assert), @"");
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
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
    test_env.add_config(r#"ui.default-revset = "root""#);

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
        r#"ui.default-revset = "root""#,
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
    @ single line
    o first line
    | second line
    | third line
    o 
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["--color=always", "log", "-T", template]);
    insta::assert_snapshot!(stdout, @r###"
    @ [1m[38;5;2msingle line[0m
    o [38;5;1mfirst line[39m
    | [38;5;1msecond line[39m
    | [38;5;1mthird line[39m
    o 
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
    @   merge
    |\  
    o | side branch
    | | with
    | | long
    | | description
    o | main branch 2
    |/  
    o main branch 1
    o initial
    o 
    "###);

    // ASCII style
    test_env.add_config(r#"ui.graph.style = "ascii""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    merge
    |\
    o |  side branch
    | |  with
    | |  long
    | |  description
    o |  main branch 2
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
    o  |  side branch
    |  |  with
    |  |  long
    |  |  description
    o  |  main branch 2
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
    o │  side branch
    │ │  with
    │ │  long
    │ │  description
    o │  main branch 2
    ├─╯
    o  main branch 1
    o  initial
    o
    "###);

    // Square style
    test_env.add_config(r#"ui.graph.style = "square""#);
    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-T=description"]);
    insta::assert_snapshot!(stdout, @r###"
    @    merge
    ├─┐
    o │  side branch
    │ │  with
    │ │  long
    │ │  description
    o │  main branch 2
    ├─┘
    o  main branch 1
    o  initial
    o
    "###);
}
