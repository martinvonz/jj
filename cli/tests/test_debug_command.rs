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

use insta::assert_snapshot;
use regex::Regex;

use crate::common::TestEnvironment;

#[test]
fn test_debug_revset() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "revset", "root()"]);
    insta::with_settings!({filters => vec![
        (r"(?m)(^    .*\n)+", "    ..\n"),
    ]}, {
        assert_snapshot!(stdout, @r###"
        -- Parsed:
        CommitRef(
            ..
        )

        -- Optimized:
        CommitRef(
            ..
        )

        -- Resolved:
        Commits(
            ..
        )

        -- Evaluated:
        RevsetImpl {
            ..
        }

        -- Commit IDs:
        0000000000000000000000000000000000000000
        "###);
    });
}

#[test]
fn test_debug_index() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "index"]);
    assert_snapshot!(filter_index_stats(&stdout), @r###"
    Number of commits: 2
    Number of merges: 0
    Max generation number: 1
    Number of heads: 1
    Number of changes: 2
    Stats per level:
      Level 0:
        Number of commits: 2
        Name: [hash]
    "###
    );
}

#[test]
fn test_debug_reindex() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&workspace_path, &["new"]);
    test_env.jj_cmd_ok(&workspace_path, &["new"]);
    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "index"]);
    assert_snapshot!(filter_index_stats(&stdout), @r###"
    Number of commits: 4
    Number of merges: 0
    Max generation number: 3
    Number of heads: 1
    Number of changes: 4
    Stats per level:
      Level 0:
        Number of commits: 3
        Name: [hash]
      Level 1:
        Number of commits: 1
        Name: [hash]
    "###
    );
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_path, &["debug", "reindex"]);
    assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Finished indexing 4 commits.
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "index"]);
    assert_snapshot!(filter_index_stats(&stdout), @r###"
    Number of commits: 4
    Number of merges: 0
    Max generation number: 3
    Number of heads: 1
    Number of changes: 4
    Stats per level:
      Level 0:
        Number of commits: 4
        Name: [hash]
    "###
    );
}

#[test]
fn test_debug_tree() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    let subdir = workspace_path.join("dir").join("subdir");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(subdir.join("file1"), "contents 1").unwrap();
    test_env.jj_cmd_ok(&workspace_path, &["new"]);
    std::fs::write(subdir.join("file2"), "contents 2").unwrap();

    // Defaults to showing the tree at the current commit
    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "tree"]);
    assert_snapshot!(stdout.replace('\\',"/"), @r###"
    dir/subdir/file1: Resolved(Some(File { id: FileId("498e9b01d79cb8d31cdf0df1a663cc1fcefd9de3"), executable: false }))
    dir/subdir/file2: Resolved(Some(File { id: FileId("b2496eaffe394cd50a9db4de5787f45f09fd9722"), executable: false }))
    "###
    );

    // Can show the tree at another commit
    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "tree", "-r@-"]);
    assert_snapshot!(stdout.replace('\\',"/"), @r###"
    dir/subdir/file1: Resolved(Some(File { id: FileId("498e9b01d79cb8d31cdf0df1a663cc1fcefd9de3"), executable: false }))
    "###
    );

    // Can filter by paths
    let stdout = test_env.jj_cmd_success(&workspace_path, &["debug", "tree", "dir/subdir/file2"]);
    assert_snapshot!(stdout.replace('\\',"/"), @r###"
    dir/subdir/file2: Resolved(Some(File { id: FileId("b2496eaffe394cd50a9db4de5787f45f09fd9722"), executable: false }))
    "###
    );

    // Can a show the root tree by id
    let stdout = test_env.jj_cmd_success(
        &workspace_path,
        &[
            "debug",
            "tree",
            "--id=0958358e3f80e794f032b25ed2be96cf5825da6c",
        ],
    );
    assert_snapshot!(stdout.replace('\\',"/"), @r###"
    dir/subdir/file1: Resolved(Some(File { id: FileId("498e9b01d79cb8d31cdf0df1a663cc1fcefd9de3"), executable: false }))
    dir/subdir/file2: Resolved(Some(File { id: FileId("b2496eaffe394cd50a9db4de5787f45f09fd9722"), executable: false }))
    "###
    );

    // Can a show non-root tree by id
    let stdout = test_env.jj_cmd_success(
        &workspace_path,
        &[
            "debug",
            "tree",
            "--dir=dir",
            "--id=6ac232efa713535ae518a1a898b77e76c0478184",
        ],
    );
    assert_snapshot!(stdout.replace('\\',"/"), @r###"
    dir/subdir/file1: Resolved(Some(File { id: FileId("498e9b01d79cb8d31cdf0df1a663cc1fcefd9de3"), executable: false }))
    dir/subdir/file2: Resolved(Some(File { id: FileId("b2496eaffe394cd50a9db4de5787f45f09fd9722"), executable: false }))
    "###
    );

    // Can filter by paths when showing non-root tree (matcher applies from root)
    let stdout = test_env.jj_cmd_success(
        &workspace_path,
        &[
            "debug",
            "tree",
            "--dir=dir",
            "--id=6ac232efa713535ae518a1a898b77e76c0478184",
            "dir/subdir/file2",
        ],
    );
    assert_snapshot!(stdout.replace('\\',"/"), @r###"
    dir/subdir/file2: Resolved(Some(File { id: FileId("b2496eaffe394cd50a9db4de5787f45f09fd9722"), executable: false }))
    "###
    );
}

#[test]
fn test_debug_operation_id() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let workspace_path = test_env.env_root().join("repo");
    let stdout =
        test_env.jj_cmd_success(&workspace_path, &["debug", "operation", "--display", "id"]);
    assert_snapshot!(filter_index_stats(&stdout), @r###"
    b51416386f2685fd5493f2b20e8eec3c24a1776d9e1a7cb5ed7e30d2d9c88c0c1e1fe71b0b7358cba60de42533d1228ed9878f2f89817d892c803395ccf9fe92
    "###
    );
}

fn filter_index_stats(text: &str) -> String {
    let regex = Regex::new(r"    Name: [0-9a-z]+").unwrap();
    regex.replace_all(text, "    Name: [hash]").to_string()
}
