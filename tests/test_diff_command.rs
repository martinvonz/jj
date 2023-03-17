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

use std::path::PathBuf;

use common::TestEnvironment;

pub mod common;

#[test]
fn test_diff_basic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "foo\nbar\n").unwrap();
    std::fs::write(repo_path.join("file3"), "foo\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @r###"
    Removed regular file file1:
       1     : foo
    Modified regular file file2:
       1    1: foo
            2: bar
    Added regular file file3:
            1: foo
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    A file3
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--types"]);
    insta::assert_snapshot!(stdout, @r###"
    F- file1
    FF file2
    -F file3
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file1 b/file1
    deleted file mode 100644
    index 257cc5642c..0000000000
    --- a/file1
    +++ /dev/null
    @@ -1,1 +1,0 @@
    -foo
    diff --git a/file2 b/file2
    index 257cc5642c...3bd1f0e297 100644
    --- a/file2
    +++ b/file2
    @@ -1,1 +1,2 @@
     foo
    +bar
    diff --git a/file3 b/file3
    new file mode 100644
    index 0000000000..257cc5642c
    --- /dev/null
    +++ b/file3
    @@ -1,0 +1,1 @@
    +foo
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "-s", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    R file1
    M file2
    A file3
    diff --git a/file1 b/file1
    deleted file mode 100644
    index 257cc5642c..0000000000
    --- a/file1
    +++ /dev/null
    @@ -1,1 +1,0 @@
    -foo
    diff --git a/file2 b/file2
    index 257cc5642c...3bd1f0e297 100644
    --- a/file2
    +++ b/file2
    @@ -1,1 +1,2 @@
     foo
    +bar
    diff --git a/file3 b/file3
    new file mode 100644
    index 0000000000..257cc5642c
    --- /dev/null
    +++ b/file3
    @@ -1,0 +1,1 @@
    +foo
    "###);
}

#[test]
fn test_diff_types() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let file_path = repo_path.join("foo");

    // Missing
    test_env.jj_cmd_success(&repo_path, &["new", "root", "-m=missing"]);

    // Normal file
    test_env.jj_cmd_success(&repo_path, &["new", "root", "-m=file"]);
    std::fs::write(&file_path, "foo").unwrap();

    // Conflict (add/add)
    test_env.jj_cmd_success(&repo_path, &["new", "root", "-m=conflict"]);
    std::fs::write(&file_path, "foo").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "root"]);
    std::fs::write(&file_path, "bar").unwrap();
    test_env.jj_cmd_success(&repo_path, &["move", r#"--to=description("conflict")"#]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Executable
        test_env.jj_cmd_success(&repo_path, &["new", "root", "-m=executable"]);
        std::fs::write(&file_path, "foo").unwrap();
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Symlink
        test_env.jj_cmd_success(&repo_path, &["new", "root", "-m=symlink"]);
        std::os::unix::fs::symlink(PathBuf::from("."), &file_path).unwrap();
    }

    let diff = |from: &str, to: &str| {
        test_env.jj_cmd_success(
            &repo_path,
            &[
                "diff",
                "--types",
                &format!(r#"--from=description("{}")"#, from),
                &format!(r#"--to=description("{}")"#, to),
            ],
        )
    };
    insta::assert_snapshot!(diff("missing", "file"), @r###"
    -F foo
    "###);
    insta::assert_snapshot!(diff("file", "conflict"), @r###"
    FC foo
    "###);
    insta::assert_snapshot!(diff("conflict", "missing"), @r###"
    C- foo
    "###);

    #[cfg(unix)]
    {
        insta::assert_snapshot!(diff("symlink", "file"), @r###"
        LF foo
        "###);
        insta::assert_snapshot!(diff("missing", "executable"), @r###"
        -F foo
        "###);
    }
}

#[test]
fn test_diff_bad_args() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["diff", "-s", "--types"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--summary' cannot be used with '--types'

    Usage: jj diff --summary [PATHS]...

    For more information, try '--help'.
    "###);

    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["diff", "--color-words", "--git"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the argument '--color-words' cannot be used with '--git'

    Usage: jj diff --color-words [PATHS]...

    For more information, try '--help'.
    "###);
}

#[test]
fn test_diff_relative_paths() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::create_dir_all(repo_path.join("dir1").join("subdir1")).unwrap();
    std::fs::create_dir(repo_path.join("dir2")).unwrap();
    std::fs::write(repo_path.join("file1"), "foo1\n").unwrap();
    std::fs::write(repo_path.join("dir1").join("file2"), "foo2\n").unwrap();
    std::fs::write(
        repo_path.join("dir1").join("subdir1").join("file3"),
        "foo3\n",
    )
    .unwrap();
    std::fs::write(repo_path.join("dir2").join("file4"), "foo4\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "bar1\n").unwrap();
    std::fs::write(repo_path.join("dir1").join("file2"), "bar2\n").unwrap();
    std::fs::write(
        repo_path.join("dir1").join("subdir1").join("file3"),
        "bar3\n",
    )
    .unwrap();
    std::fs::write(repo_path.join("dir2").join("file4"), "bar4\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path.join("dir1"), &["diff"]);
    #[cfg(unix)]
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file2:
       1    1: foo2bar2
    Modified regular file subdir1/file3:
       1    1: foo3bar3
    Modified regular file ../dir2/file4:
       1    1: foo4bar4
    Modified regular file ../file1:
       1    1: foo1bar1
    "###);
    #[cfg(windows)]
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file2:
       1    1: foo2bar2
    Modified regular file subdir1\file3:
       1    1: foo3bar3
    Modified regular file ..\dir2\file4:
       1    1: foo4bar4
    Modified regular file ..\file1:
       1    1: foo1bar1
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path.join("dir1"), &["diff", "-s"]);
    #[cfg(unix)]
    insta::assert_snapshot!(stdout, @r###"
    M file2
    M subdir1/file3
    M ../dir2/file4
    M ../file1
    "###);
    #[cfg(windows)]
    insta::assert_snapshot!(stdout, @r###"
    M file2
    M subdir1\file3
    M ..\dir2\file4
    M ..\file1
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path.join("dir1"), &["diff", "--types"]);
    #[cfg(unix)]
    insta::assert_snapshot!(stdout, @r###"
    FF file2
    FF subdir1/file3
    FF ../dir2/file4
    FF ../file1
    "###);
    #[cfg(windows)]
    insta::assert_snapshot!(stdout, @r###"
    FF file2
    FF subdir1\file3
    FF ..\dir2\file4
    FF ..\file1
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path.join("dir1"), &["diff", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/dir1/file2 b/dir1/file2
    index 54b060eee9...1fe912cdd8 100644
    --- a/dir1/file2
    +++ b/dir1/file2
    @@ -1,1 +1,1 @@
    -foo2
    +bar2
    diff --git a/dir1/subdir1/file3 b/dir1/subdir1/file3
    index c1ec6c6f12...f3c8b75ec6 100644
    --- a/dir1/subdir1/file3
    +++ b/dir1/subdir1/file3
    @@ -1,1 +1,1 @@
    -foo3
    +bar3
    diff --git a/dir2/file4 b/dir2/file4
    index a0016dbc4c...17375f7a12 100644
    --- a/dir2/file4
    +++ b/dir2/file4
    @@ -1,1 +1,1 @@
    -foo4
    +bar4
    diff --git a/file1 b/file1
    index 1715acd6a5...05c4fe6772 100644
    --- a/file1
    +++ b/file1
    @@ -1,1 +1,1 @@
    -foo1
    +bar1
    "###);
}

#[test]
fn test_diff_missing_newline() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\nbar").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "foo\nbar").unwrap();
    std::fs::write(repo_path.join("file2"), "foo").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file1:
       1    1: foo
            2: bar
    Modified regular file file2:
       1    1: foo
       2     : bar
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--git"]);
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/file1 b/file1
    index 1910281566...a907ec3f43 100644
    --- a/file1
    +++ b/file1
    @@ -1,1 +1,2 @@
    -foo
    \ No newline at end of file
    +foo
    +bar
    \ No newline at end of file
    diff --git a/file2 b/file2
    index a907ec3f43...1910281566 100644
    --- a/file2
    +++ b/file2
    @@ -1,2 +1,1 @@
    -foo
    -bar
    \ No newline at end of file
    +foo
    \ No newline at end of file
    "###);
}

#[test]
fn test_color_words_diff_missing_newline() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Empty"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\ne\nf\ng\nh\ni").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Add no newline"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\ne\nf\ng\nh\ni").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Modify first line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\ni").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Modify middle line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\nI").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Modify last line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\nI\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Append newline"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\nI").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Remove newline"]);
    std::fs::write(repo_path.join("file1"), "").unwrap();
    test_env.jj_cmd_success(&repo_path, &["commit", "-m", "=== Empty"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-Tdescription", "-pr:@-", "--no-graph", "--reversed"],
    );
    insta::assert_snapshot!(stdout, @r###"
    === Empty
    Added regular file file1:
    === Add no newline
    Modified regular file file1:
            1: a
            2: b
            3: c
            4: d
            5: e
            6: f
            7: g
            8: h
            9: i
    === Modify first line
    Modified regular file file1:
       1    1: aA
       2    2: b
       3    3: c
       4    4: d
        ...
    === Modify middle line
    Modified regular file file1:
       1    1: A
       2    2: b
       3    3: c
       4    4: d
       5    5: eE
       6    6: f
       7    7: g
       8    8: h
       9    9: i
    === Modify last line
    Modified regular file file1:
        ...
       6    6: f
       7    7: g
       8    8: h
       9    9: iI
    === Append newline
    Modified regular file file1:
        ...
       6    6: f
       7    7: g
       8    8: h
       9    9: I
    === Remove newline
    Modified regular file file1:
        ...
       6    6: f
       7    7: g
       8    8: h
       9    9: I
    === Empty
    Modified regular file file1:
       1     : A
       2     : b
       3     : c
       4     : d
       5     : E
       6     : f
       7     : g
       8     : h
       9     : I
    "###);
}

#[test]
fn test_diff_skipped_context() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj").unwrap();
    test_env.jj_cmd_success(&repo_path, &["describe", "-m", "=== Left side of diffs"]);

    test_env.jj_cmd_success(&repo_path, &["new", "@", "-m", "=== Must skip 2 lines"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "@-", "-m", "=== Don't skip 1 line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\ne\nf\ng\nh\nI\nj").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "@-", "-m", "=== No gap to skip"]);
    std::fs::write(repo_path.join("file1"), "a\nB\nc\nd\ne\nf\ng\nh\nI\nj").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "@-", "-m", "=== No gap to skip"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nC\nd\ne\nf\ng\nh\nI\nj").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "@-", "-m", "=== 1 line at start"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\nE\nf\ng\nh\ni\nj").unwrap();
    test_env.jj_cmd_success(&repo_path, &["new", "@-", "-m", "=== 1 line at end"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\ne\nF\ng\nh\ni\nj").unwrap();

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &["log", "-Tdescription", "-p", "--no-graph", "--reversed"],
    );
    insta::assert_snapshot!(stdout, @r###"
    === Left side of diffs
    Added regular file file1:
            1: a
            2: b
            3: c
            4: d
            5: e
            6: f
            7: g
            8: h
            9: i
           10: j
    === Must skip 2 lines
    Modified regular file file1:
       1    1: aA
       2    2: b
       3    3: c
       4    4: d
        ...
       7    7: g
       8    8: h
       9    9: i
      10   10: jJ
    === Don't skip 1 line
    Modified regular file file1:
       1    1: aA
       2    2: b
       3    3: c
       4    4: d
       5    5: e
       6    6: f
       7    7: g
       8    8: h
       9    9: iI
      10   10: j
    === No gap to skip
    Modified regular file file1:
       1    1: a
       2    2: bB
       3    3: c
       4    4: d
       5    5: e
       6    6: f
       7    7: g
       8    8: h
       9    9: iI
      10   10: j
    === No gap to skip
    Modified regular file file1:
       1    1: a
       2    2: b
       3    3: cC
       4    4: d
       5    5: e
       6    6: f
       7    7: g
       8    8: h
       9    9: iI
      10   10: j
    === 1 line at start
    Modified regular file file1:
       1    1: a
       2    2: b
       3    3: c
       4    4: d
       5    5: eE
       6    6: f
       7    7: g
       8    8: h
        ...
    === 1 line at end
    Modified regular file file1:
        ...
       3    3: c
       4    4: d
       5    5: e
       6    6: fF
       7    7: g
       8    8: h
       9    9: i
      10   10: j
    "###);
}
