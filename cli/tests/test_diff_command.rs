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

use itertools::Itertools;

use crate::common::{escaped_fake_diff_editor_path, strip_last_line, TestEnvironment};

#[test]
fn test_diff_basic() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
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

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--context=0"]);
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
    D file1
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

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--git", "--context=0"]);
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
    @@ -2,0 +2,1 @@
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
    D file1
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

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    file1 | 1 -
    file2 | 1 +
    file3 | 1 +
    3 files changed, 2 insertions(+), 1 deletion(-)
    "###);
}

#[test]
fn test_diff_empty() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "").unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @r###"
    Added regular file file1:
        (empty)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @r###"
    Removed regular file file1:
        (empty)
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    file1 | 0
    1 file changed, 0 insertions(+), 0 deletions(-)
    "###);
}

#[test]
fn test_diff_types() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let file_path = repo_path.join("foo");

    // Missing
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=missing"]);

    // Normal file
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=file"]);
    std::fs::write(&file_path, "foo").unwrap();

    // Conflict (add/add)
    test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=conflict"]);
    std::fs::write(&file_path, "foo").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "root()"]);
    std::fs::write(&file_path, "bar").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["move", r#"--to=description("conflict")"#]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;

        // Executable
        test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=executable"]);
        std::fs::write(&file_path, "foo").unwrap();
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Symlink
        test_env.jj_cmd_ok(&repo_path, &["new", "root()", "-m=symlink"]);
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
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
    test_env.jj_cmd_ok(&repo_path, &["new"]);
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

    let stdout = test_env.jj_cmd_success(&repo_path.join("dir1"), &["diff", "--stat"]);
    #[cfg(unix)]
    insta::assert_snapshot!(stdout, @r###"
    file2         | 2 +-
    subdir1/file3 | 2 +-
    ../dir2/file4 | 2 +-
    ../file1      | 2 +-
    4 files changed, 4 insertions(+), 4 deletions(-)
    "###);
    #[cfg(windows)]
    insta::assert_snapshot!(stdout, @r###"
    file2         | 2 +-
    subdir1\file3 | 2 +-
    ..\dir2\file4 | 2 +-
    ..\file1      | 2 +-
    4 files changed, 4 insertions(+), 4 deletions(-)
    "###);
}

#[test]
fn test_diff_missing_newline() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\nbar").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
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

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    file1 | 3 ++-
    file2 | 3 +--
    2 files changed, 3 insertions(+), 3 deletions(-)
    "###);
}

#[test]
fn test_color_words_diff_missing_newline() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Empty"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\ne\nf\ng\nh\ni").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Add no newline"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\ne\nf\ng\nh\ni").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Modify first line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\ni").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Modify middle line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\nI").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Modify last line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\nI\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Append newline"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\nE\nf\ng\nh\nI").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Remove newline"]);
    std::fs::write(repo_path.join("file1"), "").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "=== Empty"]);

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-Tdescription",
            "-pr::@-",
            "--no-graph",
            "--reversed",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    === Empty
    Added regular file file1:
        (empty)
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
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "=== Left side of diffs"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "@", "-m", "=== Must skip 2 lines"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== Don't skip 1 line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nd\ne\nf\ng\nh\nI\nj").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== No gap to skip"]);
    std::fs::write(repo_path.join("file1"), "a\nB\nc\nd\ne\nf\ng\nh\nI\nj").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== No gap to skip"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nC\nd\ne\nf\ng\nh\nI\nj").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== 1 line at start"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd\nE\nf\ng\nh\ni\nj").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== 1 line at end"]);
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

#[test]
fn test_diff_skipped_context_nondefault() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "a\nb\nc\nd").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m", "=== Left side of diffs"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "@", "-m", "=== Must skip 2 lines"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nc\nD").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== Don't skip 1 line"]);
    std::fs::write(repo_path.join("file1"), "A\nb\nC\nd").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== No gap to skip"]);
    std::fs::write(repo_path.join("file1"), "a\nB\nC\nd").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== 1 line at start"]);
    std::fs::write(repo_path.join("file1"), "a\nB\nc\nd").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new", "@-", "-m", "=== 1 line at end"]);
    std::fs::write(repo_path.join("file1"), "a\nb\nC\nd").unwrap();

    let stdout = test_env.jj_cmd_success(
        &repo_path,
        &[
            "log",
            "-Tdescription",
            "-p",
            "--no-graph",
            "--reversed",
            "--context=0",
        ],
    );
    insta::assert_snapshot!(stdout, @r###"
    === Left side of diffs
    Added regular file file1:
            1: a
            2: b
            3: c
            4: d
    === Must skip 2 lines
    Modified regular file file1:
       1    1: aA
        ...
       4    4: dD
    === Don't skip 1 line
    Modified regular file file1:
       1    1: aA
       2    2: b
       3    3: cC
       4    4: d
    === No gap to skip
    Modified regular file file1:
       1    1: a
       2    2: bB
       3    3: cC
       4    4: d
    === 1 line at start
    Modified regular file file1:
       1    1: a
       2    2: bB
        ...
    === 1 line at end
    Modified regular file file1:
        ...
       3    3: cC
       4    4: d
    "###);
}

#[test]
fn test_diff_external_tool() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();
    std::fs::write(repo_path.join("file2"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1")).unwrap();
    std::fs::write(repo_path.join("file2"), "foo\nbar\n").unwrap();
    std::fs::write(repo_path.join("file3"), "foo\n").unwrap();

    let edit_script = test_env.set_up_fake_diff_editor();
    std::fs::write(
        &edit_script,
        "print-files-before\0print --\0print-files-after",
    )
    .unwrap();

    // diff without file patterns
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["diff", "--tool=fake-diff-editor"]), @r###"
    file1
    file2
    --
    file2
    file3
    "###);

    // diff with file patterns
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["diff", "--tool=fake-diff-editor", "file1"]), @r###"
    file1
    --
    "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["log", "-p", "--tool=fake-diff-editor"]), @r###"
    @  rlvkpnrz test.user@example.com 2001-02-03 08:05:09 0cba70c7
    │  (no description set)
    │  file1
    │  file2
    │  --
    │  file2
    │  file3
    ◉  qpvuntsm test.user@example.com 2001-02-03 08:05:08 39b5a56f
    │  (no description set)
    │  --
    │  file1
    │  file2
    ◉  zzzzzzzz root() 00000000
       --
    "###);

    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["show", "--tool=fake-diff-editor"]), @r###"
    Commit ID: 0cba70c72186eabb5a2f91be63a8366b9f6da6c6
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:08)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    file1
    file2
    --
    file2
    file3
    "###);

    // Enabled by default, looks up the merge-tools table
    let config = "--config-toml=ui.diff.tool='fake-diff-editor'";
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", config]), @r###"
    file1
    file2
    --
    file2
    file3
    "###);

    // Inlined command arguments
    let command = escaped_fake_diff_editor_path();
    let config = format!(r#"--config-toml=ui.diff.tool=["{command}", "$right", "$left"]"#);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", &config]), @r###"
    file2
    file3
    --
    file1
    file2
    "###);

    // Output of external diff tool shouldn't be escaped
    std::fs::write(&edit_script, "print \x1b[1;31mred").unwrap();
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["diff", "--color=always", "--tool=fake-diff-editor"]),
        @r###"
    [1;31mred
    "###);

    // Non-zero exit code isn't an error
    std::fs::write(&edit_script, "print diff\0fail").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["show", "--tool=fake-diff-editor"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: 0cba70c72186eabb5a2f91be63a8366b9f6da6c6
    Change ID: rlvkpnrzqnoowoytxnquwvuryrwnrmlp
    Author: Test User <test.user@example.com> (2001-02-03 08:05:08)
    Committer: Test User <test.user@example.com> (2001-02-03 08:05:09)

        (no description set)

    diff
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Tool exited with a non-zero code (run with --debug to see the exact invocation). Exit code: 1.
    "###);

    // --tool=:builtin shouldn't be ignored
    let stderr = test_env.jj_cmd_failure(&repo_path, &["diff", "--tool=:builtin"]);
    insta::assert_snapshot!(strip_last_line(&stderr), @r###"
    Error: Failed to generate diff
    Caused by:
    1: Error executing ':builtin' (run with --debug to see the exact invocation)
    "###);
}

#[cfg(unix)]
#[test]
fn test_diff_external_tool_symlink() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let external_file_path = test_env.env_root().join("external-file");
    std::fs::write(&external_file_path, "").unwrap();
    let external_file_permissions = external_file_path.symlink_metadata().unwrap().permissions();

    std::os::unix::fs::symlink("non-existent1", repo_path.join("dead")).unwrap();
    std::os::unix::fs::symlink(&external_file_path, repo_path.join("file")).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("dead")).unwrap();
    std::os::unix::fs::symlink("non-existent2", repo_path.join("dead")).unwrap();
    std::fs::remove_file(repo_path.join("file")).unwrap();
    std::fs::write(repo_path.join("file"), "").unwrap();

    let edit_script = test_env.set_up_fake_diff_editor();
    std::fs::write(
        edit_script,
        "print-files-before\0print --\0print-files-after",
    )
    .unwrap();

    // Shouldn't try to change permission of symlinks
    insta::assert_snapshot!(
        test_env.jj_cmd_success(&repo_path, &["diff", "--tool=fake-diff-editor"]), @r###"
    dead
    file
    --
    dead
    file
    "###);

    // External file should be intact
    assert_eq!(
        external_file_path.symlink_metadata().unwrap().permissions(),
        external_file_permissions
    );
}

#[test]
fn test_diff_stat() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    std::fs::write(repo_path.join("file1"), "foo\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    file1 | 1 +
    1 file changed, 1 insertion(+), 0 deletions(-)
    "###);

    test_env.jj_cmd_ok(&repo_path, &["new"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @"0 files changed, 0 insertions(+), 0 deletions(-)");

    std::fs::write(repo_path.join("file1"), "foo\nbar\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file1"), "bar\n").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    file1 | 1 -
    1 file changed, 0 insertions(+), 1 deletion(-)
    "###);
}

#[test]
fn test_diff_stat_long_name_or_stat() {
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("COLUMNS", "30");
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let get_stat = |test_env: &TestEnvironment, path_length: usize, stat_size: usize| {
        test_env.jj_cmd_ok(&repo_path, &["new", "root()"]);
        let ascii_name = "1234567890".chars().cycle().take(path_length).join("");
        let han_name = "一二三四五六七八九十"
            .chars()
            .cycle()
            .take(path_length)
            .join("");
        let content = "content line\n".repeat(stat_size);
        std::fs::write(repo_path.join(ascii_name), &content).unwrap();
        std::fs::write(repo_path.join(han_name), &content).unwrap();
        test_env.jj_cmd_success(&repo_path, &["diff", "--stat"])
    };

    insta::assert_snapshot!(get_stat(&test_env, 1, 1), @r###"
    1   | 1 +
    一  | 1 +
    2 files changed, 2 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 1, 10), @r###"
    1   | 10 ++++++++++
    一  | 10 ++++++++++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 1, 100), @r###"
    1   | 100 +++++++++++++++++
    一  | 100 +++++++++++++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 10, 1), @r###"
    1234567890      | 1 +
    ...五六七八九十 | 1 +
    2 files changed, 2 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 10, 10), @r###"
    1234567890     | 10 +++++++
    ...六七八九十  | 10 +++++++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 10, 100), @r###"
    1234567890     | 100 ++++++
    ...六七八九十  | 100 ++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 50, 1), @r###"
    ...901234567890 | 1 +
    ...五六七八九十 | 1 +
    2 files changed, 2 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 50, 10), @r###"
    ...01234567890 | 10 +++++++
    ...六七八九十  | 10 +++++++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 50, 100), @r###"
    ...01234567890 | 100 ++++++
    ...六七八九十  | 100 ++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);

    // Lengths around where we introduce the ellipsis
    insta::assert_snapshot!(get_stat(&test_env, 13, 100), @r###"
    1234567890123  | 100 ++++++
    ...九十一二三  | 100 ++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 14, 100), @r###"
    12345678901234 | 100 ++++++
    ...十一二三四  | 100 ++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 15, 100), @r###"
    ...56789012345 | 100 ++++++
    ...一二三四五  | 100 ++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 16, 100), @r###"
    ...67890123456 | 100 ++++++
    ...二三四五六  | 100 ++++++
    2 files changed, 200 insertions(+), 0 deletions(-)
    "###);

    // Very narrow terminal (doesn't have to fit, just don't crash)
    test_env.add_env_var("COLUMNS", "10");
    insta::assert_snapshot!(get_stat(&test_env, 10, 10), @r###"
    ... | 10 ++
    ... | 10 ++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
    test_env.add_env_var("COLUMNS", "3");
    insta::assert_snapshot!(get_stat(&test_env, 10, 10), @r###"
    ... | 10 ++
    ... | 10 ++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 3, 10), @r###"
    123 | 10 ++
    ... | 10 ++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
    insta::assert_snapshot!(get_stat(&test_env, 1, 10), @r###"
    1   | 10 ++
    一  | 10 ++
    2 files changed, 20 insertions(+), 0 deletions(-)
    "###);
}

#[test]
fn test_diff_binary() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1.png"), b"\x89PNG\r\n\x1a\nabcdefg\0").unwrap();
    std::fs::write(repo_path.join("file2.png"), b"\x89PNG\r\n\x1a\n0123456\0").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::remove_file(repo_path.join("file1.png")).unwrap();
    std::fs::write(repo_path.join("file2.png"), "foo\nbar\n").unwrap();
    std::fs::write(repo_path.join("file3.png"), b"\x89PNG\r\n\x1a\nxyz\0").unwrap();
    // try a file that's valid UTF-8 but contains control characters
    std::fs::write(repo_path.join("file4.png"), b"\0\0\0").unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff"]);
    insta::assert_snapshot!(stdout, @r###"
    Removed regular file file1.png:
        (binary)
    Modified regular file file2.png:
        (binary)
    Added regular file file3.png:
        (binary)
    Added regular file file4.png:
        (binary)
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout, @r###"
    file1.png | 3 ---
    file2.png | 5 ++---
    file3.png | 3 +++
    file4.png | 1 +
    4 files changed, 6 insertions(+), 6 deletions(-)
    "###);
}
