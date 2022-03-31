// Copyright 2022 Google LLC
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
    // TODO: We should probably copy git's `\ No newline at end of file` solution
    insta::assert_snapshot!(stdout, @r###"
    Modified regular file file1:
       1    1: foo
            2: barModified regular file file2:
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
