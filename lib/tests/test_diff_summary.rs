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

use jj_lib::matchers::{EverythingMatcher, FilesMatcher};
use jj_lib::merged_tree::DiffSummary;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use testutils::{create_tree, TestRepo};

fn to_owned_path_vec(paths: &[&RepoPath]) -> Vec<RepoPathBuf> {
    paths.iter().map(|&path| path.to_owned()).collect()
}

#[test]
fn test_types() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let clean_path = RepoPath::from_internal_string("clean");
    let modified_path = RepoPath::from_internal_string("modified");
    let added_path = RepoPath::from_internal_string("added");
    let removed_path = RepoPath::from_internal_string("removed");

    let tree1 = create_tree(
        repo,
        &[
            (clean_path, "clean"),
            (modified_path, "contents before"),
            (removed_path, "removed contents"),
        ],
    );

    let tree2 = create_tree(
        repo,
        &[
            (clean_path, "clean"),
            (modified_path, "contents after"),
            (added_path, "added contents"),
        ],
    );

    assert_eq!(
        tree1.diff_summary(&tree2, &EverythingMatcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[modified_path]),
            added: to_owned_path_vec(&[added_path]),
            removed: to_owned_path_vec(&[removed_path]),
        }
    );
}

#[test]
fn test_tree_file_transition() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let dir_file_path = RepoPath::from_internal_string("dir/file");
    let dir_path = RepoPath::from_internal_string("dir");

    let tree1 = create_tree(repo, &[(dir_file_path, "contents")]);
    let tree2 = create_tree(repo, &[(dir_path, "contents")]);

    assert_eq!(
        tree1.diff_summary(&tree2, &EverythingMatcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[dir_path]),
            removed: to_owned_path_vec(&[dir_file_path]),
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &EverythingMatcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[dir_file_path]),
            removed: to_owned_path_vec(&[dir_path]),
        }
    );
}

#[test]
fn test_sorting() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let a_path = RepoPath::from_internal_string("a");
    let b_path = RepoPath::from_internal_string("b");
    let f_a_path = RepoPath::from_internal_string("f/a");
    let f_b_path = RepoPath::from_internal_string("f/b");
    let f_f_a_path = RepoPath::from_internal_string("f/f/a");
    let f_f_b_path = RepoPath::from_internal_string("f/f/b");
    let n_path = RepoPath::from_internal_string("n");
    let s_b_path = RepoPath::from_internal_string("s/b");
    let z_path = RepoPath::from_internal_string("z");

    let tree1 = create_tree(
        repo,
        &[
            (a_path, "before"),
            (f_a_path, "before"),
            (f_f_a_path, "before"),
        ],
    );

    let tree2 = create_tree(
        repo,
        &[
            (a_path, "after"),
            (b_path, "after"),
            (f_a_path, "after"),
            (f_b_path, "after"),
            (f_f_a_path, "after"),
            (f_f_b_path, "after"),
            (n_path, "after"),
            (s_b_path, "after"),
            (z_path, "after"),
        ],
    );

    assert_eq!(
        tree1.diff_summary(&tree2, &EverythingMatcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[a_path, f_a_path, f_f_a_path]),
            added: to_owned_path_vec(&[b_path, f_b_path, f_f_b_path, n_path, s_b_path, z_path]),
            removed: vec![],
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &EverythingMatcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[a_path, f_a_path, f_f_a_path]),
            added: vec![],
            removed: to_owned_path_vec(&[b_path, f_b_path, f_f_b_path, n_path, s_b_path, z_path]),
        }
    );
}

#[test]
fn test_matcher_dir_file_transition() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let a_path = RepoPath::from_internal_string("a");
    let a_a_path = RepoPath::from_internal_string("a/a");

    let tree1 = create_tree(repo, &[(a_path, "before")]);
    let tree2 = create_tree(repo, &[(a_a_path, "after")]);

    let matcher = FilesMatcher::new([&a_path]);
    assert_eq!(
        tree1.diff_summary(&tree2, &matcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: vec![],
            removed: to_owned_path_vec(&[a_path]),
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &matcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[a_path]),
            removed: vec![],
        }
    );

    let matcher = FilesMatcher::new([a_a_path]);
    assert_eq!(
        tree1.diff_summary(&tree2, &matcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[a_a_path]),
            removed: vec![],
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &matcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: vec![],
            removed: to_owned_path_vec(&[a_a_path]),
        }
    );

    let matcher = FilesMatcher::new([a_path, a_a_path]);
    assert_eq!(
        tree1.diff_summary(&tree2, &matcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[a_a_path]),
            removed: to_owned_path_vec(&[a_path]),
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &matcher).unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[a_path]),
            removed: to_owned_path_vec(&[a_a_path]),
        }
    );
}

#[test]
fn test_matcher_normal_cases() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let a_path = RepoPath::from_internal_string("a");
    let dir1_a_path = RepoPath::from_internal_string("dir1/a");
    let dir2_b_path = RepoPath::from_internal_string("dir2/b");
    let z_path = RepoPath::from_internal_string("z");

    let tree1 = create_tree(repo, &[(a_path, "before"), (dir1_a_path, "before")]);
    // File "a" gets modified
    // File "dir1/a" gets modified
    // File "dir2/b" gets created
    // File "z" gets created
    let tree2 = create_tree(
        repo,
        &[
            (a_path, "after"),
            (dir1_a_path, "after"),
            (dir2_b_path, "after"),
            (z_path, "after"),
        ],
    );

    let matcher = FilesMatcher::new([a_path, z_path]);
    assert_eq!(
        tree1.diff_summary(&tree2, &matcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[a_path]),
            added: to_owned_path_vec(&[z_path]),
            removed: vec![],
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &matcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[a_path]),
            added: vec![],
            removed: to_owned_path_vec(&[z_path]),
        }
    );

    let matcher = FilesMatcher::new([dir1_a_path, dir2_b_path]);
    assert_eq!(
        tree1.diff_summary(&tree2, &matcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[dir1_a_path]),
            added: to_owned_path_vec(&[dir2_b_path]),
            removed: vec![],
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1, &matcher).unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[dir1_a_path]),
            added: vec![],
            removed: to_owned_path_vec(&[dir2_b_path]),
        }
    );
}
