// Copyright 2020 The Jujutsu Authors
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

use std::io::Write;

use crate::common::TestEnvironment;

#[test]
fn test_gitignores() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);

    // Say in core.excludesFiles that we don't want file1, file2, or file3
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace_root.join(".git").join("config"))
        .unwrap();
    // Put the file in "~/my-ignores" so we also test that "~" expands to "$HOME"
    file.write_all(b"[core]\nexcludesFile=~/my-ignores\n")
        .unwrap();
    drop(file);
    std::fs::write(
        test_env.home_dir().join("my-ignores"),
        "file1\nfile2\nfile3",
    )
    .unwrap();

    // Say in .git/info/exclude that we actually do want file2 and file3
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace_root.join(".git").join("info").join("exclude"))
        .unwrap();
    file.write_all(b"!file2\n!file3").unwrap();
    drop(file);

    // Say in .gitignore (in the working copy) that we actually do not want file2
    // (again)
    std::fs::write(workspace_root.join(".gitignore"), "file2").unwrap();

    // Writes some files to the working copy
    std::fs::write(workspace_root.join("file0"), "contents").unwrap();
    std::fs::write(workspace_root.join("file1"), "contents").unwrap();
    std::fs::write(workspace_root.join("file2"), "contents").unwrap();
    std::fs::write(workspace_root.join("file3"), "contents").unwrap();

    let stdout = test_env.jj_cmd_success(&workspace_root, &["diff", "-s"]);
    insta::assert_snapshot!(stdout, @r###"
    A .gitignore
    A file0
    A file3
    "###);
}

#[test]
fn test_gitignores_ignored_file_in_target_commit() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);

    // Create a commit with file "ignored" in it
    std::fs::write(workspace_root.join("ignored"), "committed contents\n").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "with-file"]);
    let target_commit_id = test_env.jj_cmd_success(
        &workspace_root,
        &["log", "--no-graph", "-T=commit_id", "-r=@"],
    );

    // Create another commit where we ignore that path
    test_env.jj_cmd_ok(&workspace_root, &["new", "root()"]);
    std::fs::write(workspace_root.join("ignored"), "contents in working copy\n").unwrap();
    std::fs::write(workspace_root.join(".gitignore"), ".gitignore\nignored\n").unwrap();

    // Update to the commit with the "ignored" file
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["edit", "with-file"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: qpvuntsm 4a703628 with-file | (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 1 files, modified 0 files, removed 0 files
    Warning: 1 of those updates were skipped because there were conflicting changes in the working copy.
    Hint: Inspect the changes compared to the intended target with `jj diff --from 4a703628bcb2`.
    Discard the conflicting changes with `jj restore --from 4a703628bcb2`.
    "###);
    let stdout = test_env.jj_cmd_success(
        &workspace_root,
        &["diff", "--git", "--from", &target_commit_id],
    );
    insta::assert_snapshot!(stdout, @r###"
    diff --git a/ignored b/ignored
    index 8a69467466...4d9be5127b 100644
    --- a/ignored
    +++ b/ignored
    @@ -1,1 +1,1 @@
    -committed contents
    +contents in working copy
    "###);
}
