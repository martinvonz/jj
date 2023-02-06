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
            "--config-toml=diff.format='summary'",
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
    for i in 1..50 {
        test_env.jj_cmd_success(&repo_path, &["new", "-m", &format!("commit{i}")]);
        std::fs::write(repo_path.join("file"), format!("file {i}\n")).unwrap();
    }
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "original", "-T", &prefix_format(Some(12))]),
        @r###"
    o Change 9a4[5c67d3e96] initial ba1[a30916d29] original
    ~ 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "@-----------..@", "-T", &prefix_format(Some(12))]),
        @r###"
    @ Change 4c9[32da80131] commit49 d8[3437a2ceff] 
    o Change 0d[58f15eaba6] commit48 f3[abb4ea0ac3] 
    o Change fc[e6c2c59123] commit47 38e[891bea27b] 
    o Change d5[1defcac305] commit46 1c[04d947707a] 
    o Change 4f[13b1391d68] commit45 747[24ae22b1e] 
    o Change 6a[de2950a042] commit44 c7a[a67cf7bbd] 
    o Change 06c[482e452d3] commit43 8e[c99dfcb6c7] 
    o Change 392[beeb018eb] commit42 8f0[e60411b78] 
    o Change a1[b73d3ff916] commit41 71[d6937a66c3] 
    o Change 708[8f461291f] commit40 db[5720490266] 
    o Change c49[f7f006c77] commit39 d94[54fec8a69] 
    ~ 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "@-----------..@", "-T", &prefix_format(Some(3))]),
        @r###"
    @ Change 4c9 commit49 d8[3] 
    o Change 0d[5] commit48 f3[a] 
    o Change fc[e] commit47 38e 
    o Change d5[1] commit46 1c[0] 
    o Change 4f[1] commit45 747 
    o Change 6a[d] commit44 c7a 
    o Change 06c commit43 8e[c] 
    o Change 392 commit42 8f0 
    o Change a1[b] commit41 71[d] 
    o Change 708 commit40 db[5] 
    o Change c49 commit39 d94 
    ~ 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "@-----------..@", "-T", &prefix_format(None)]),
        @r###"
    @ Change 4c9 commit49 d8 
    o Change 0d commit48 f3 
    o Change fc commit47 38e 
    o Change d5 commit46 1c 
    o Change 4f commit45 747 
    o Change 6a commit44 c7a 
    o Change 06c commit43 8e 
    o Change 392 commit42 8f0 
    o Change a1 commit41 71 
    o Change 708 commit40 db 
    o Change c49 commit39 d94 
    ~ 
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
      "Change " change_id.shortest_styled_prefix({0}) " " description.first_line()
      " " commit_id.shortest_styled_prefix({0}) " " branches
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
    for i in 1..50 {
        test_env.jj_cmd_success(&repo_path, &["new", "-m", &format!("commit{i}")]);
        std::fs::write(repo_path.join("file"), format!("file {i}\n")).unwrap();
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
    @ Change [1m[38;5;5m4c9[0m[38;5;8m32da80131[39m commit49 [1m[38;5;4md8[0m[38;5;8m3437a2ceff[39m 
    o Change [1m[38;5;5m0d[0m[38;5;8m58f15eaba6[39m commit48 [1m[38;5;4mf3[0m[38;5;8mabb4ea0ac3[39m 
    o Change [1m[38;5;5mfc[0m[38;5;8me6c2c59123[39m commit47 [1m[38;5;4m38e[0m[38;5;8m891bea27b[39m 
    o Change [1m[38;5;5md5[0m[38;5;8m1defcac305[39m commit46 [1m[38;5;4m1c[0m[38;5;8m04d947707a[39m 
    o Change [1m[38;5;5m4f[0m[38;5;8m13b1391d68[39m commit45 [1m[38;5;4m747[0m[38;5;8m24ae22b1e[39m 
    o Change [1m[38;5;5m6a[0m[38;5;8mde2950a042[39m commit44 [1m[38;5;4mc7a[0m[38;5;8ma67cf7bbd[39m 
    o Change [1m[38;5;5m06c[0m[38;5;8m482e452d3[39m commit43 [1m[38;5;4m8e[0m[38;5;8mc99dfcb6c7[39m 
    o Change [1m[38;5;5m392[0m[38;5;8mbeeb018eb[39m commit42 [1m[38;5;4m8f0[0m[38;5;8me60411b78[39m 
    o Change [1m[38;5;5ma1[0m[38;5;8mb73d3ff916[39m commit41 [1m[38;5;4m71[0m[38;5;8md6937a66c3[39m 
    o Change [1m[38;5;5m708[0m[38;5;8m8f461291f[39m commit40 [1m[38;5;4mdb[0m[38;5;8m5720490266[39m 
    o Change [1m[38;5;5mc49[0m[38;5;8mf7f006c77[39m commit39 [1m[38;5;4md94[0m[38;5;8m54fec8a69[39m 
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
            &prefix_format(Some(3)),
        ],
    );
    insta::assert_snapshot!(stdout,
        @r###"
    @ Change [1m[38;5;5m4c9[0m commit49 [1m[38;5;4md8[0m[38;5;8m3[39m 
    o Change [1m[38;5;5m0d[0m[38;5;8m5[39m commit48 [1m[38;5;4mf3[0m[38;5;8ma[39m 
    o Change [1m[38;5;5mfc[0m[38;5;8me[39m commit47 [1m[38;5;4m38e[0m 
    o Change [1m[38;5;5md5[0m[38;5;8m1[39m commit46 [1m[38;5;4m1c[0m[38;5;8m0[39m 
    o Change [1m[38;5;5m4f[0m[38;5;8m1[39m commit45 [1m[38;5;4m747[0m 
    o Change [1m[38;5;5m6a[0m[38;5;8md[39m commit44 [1m[38;5;4mc7a[0m 
    o Change [1m[38;5;5m06c[0m commit43 [1m[38;5;4m8e[0m[38;5;8mc[39m 
    o Change [1m[38;5;5m392[0m commit42 [1m[38;5;4m8f0[0m 
    o Change [1m[38;5;5ma1[0m[38;5;8mb[39m commit41 [1m[38;5;4m71[0m[38;5;8md[39m 
    o Change [1m[38;5;5m708[0m commit40 [1m[38;5;4mdb[0m[38;5;8m5[39m 
    o Change [1m[38;5;5mc49[0m commit39 [1m[38;5;4md94[0m 
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
            &prefix_format(None),
        ],
    );
    insta::assert_snapshot!(stdout,
        @r###"
    @ Change [1m[38;5;5m4c9[0m commit49 [1m[38;5;4md8[0m 
    o Change [1m[38;5;5m0d[0m commit48 [1m[38;5;4mf3[0m 
    o Change [1m[38;5;5mfc[0m commit47 [1m[38;5;4m38e[0m 
    o Change [1m[38;5;5md5[0m commit46 [1m[38;5;4m1c[0m 
    o Change [1m[38;5;5m4f[0m commit45 [1m[38;5;4m747[0m 
    o Change [1m[38;5;5m6a[0m commit44 [1m[38;5;4mc7a[0m 
    o Change [1m[38;5;5m06c[0m commit43 [1m[38;5;4m8e[0m 
    o Change [1m[38;5;5m392[0m commit42 [1m[38;5;4m8f0[0m 
    o Change [1m[38;5;5ma1[0m commit41 [1m[38;5;4m71[0m 
    o Change [1m[38;5;5m708[0m commit40 [1m[38;5;4mdb[0m 
    o Change [1m[38;5;5mc49[0m commit39 [1m[38;5;4md94[0m 
    ~ 
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
    for i in 1..100 {
        test_env.jj_cmd_success(&repo_path, &["describe", "-m", &format!("commit{i}")]);
        std::fs::write(repo_path.join("file"), format!("file {i}\n")).unwrap();
    }
    // The first commit still exists. Its unique prefix became longer.
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "ba1", "-T", prefix_format]),
        @r###"
    o Change 9a4[5c67d3e96] initial ba1[a30916d29] 
    ~ 
    "###
    );
    // The first commit is no longer visible
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "all()", "-T", prefix_format]),
        @r###"
    @ Change 9a4[5c67d3e96] commit99 de[3177d2acf2] original
    o Change 000[000000000]  000[000000000] 
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_failure(&repo_path, &["log", "-r", "d", "-T", prefix_format]),
        @r###"
    Error: Commit or change id prefix "d" is ambiguous
    "###
    );
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-r", "de", "-T", prefix_format]),
        @r###"
    @ Change 9a4[5c67d3e96] commit99 de[3177d2acf2] original
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
