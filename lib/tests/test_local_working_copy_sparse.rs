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
use jj_lib::local_working_copy::LocalWorkingCopy;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::working_copy::{CheckoutStats, WorkingCopy};
use testutils::{commit_with_tree, create_tree, TestWorkspace};

fn to_owned_path_vec(paths: &[&RepoPath]) -> Vec<RepoPathBuf> {
    paths.iter().map(|&path| path.to_owned()).collect()
}

#[test]
fn test_sparse_checkout() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let working_copy_path = test_workspace.workspace.workspace_root().clone();

    let root_file1_path = RepoPath::from_internal_string("file1");
    let root_file2_path = RepoPath::from_internal_string("file2");
    let dir1_path = RepoPath::from_internal_string("dir1");
    let dir1_file1_path = RepoPath::from_internal_string("dir1/file1");
    let dir1_file2_path = RepoPath::from_internal_string("dir1/file2");
    let dir1_subdir1_path = RepoPath::from_internal_string("dir1/subdir1");
    let dir1_subdir1_file1_path = RepoPath::from_internal_string("dir1/subdir1/file1");
    let dir2_path = RepoPath::from_internal_string("dir2");
    let dir2_file1_path = RepoPath::from_internal_string("dir2/file1");

    let tree = create_tree(
        repo,
        &[
            (root_file1_path, "contents"),
            (root_file2_path, "contents"),
            (dir1_file1_path, "contents"),
            (dir1_file2_path, "contents"),
            (dir1_subdir1_file1_path, "contents"),
            (dir2_file1_path, "contents"),
        ],
    );
    let commit = commit_with_tree(repo.store(), tree.id());

    test_workspace
        .workspace
        .check_out(repo.op_id().clone(), None, &commit)
        .unwrap();
    let ws = &mut test_workspace.workspace;

    // Set sparse patterns to only dir1/
    let mut locked_ws = ws.start_working_copy_mutation().unwrap();
    let sparse_patterns = to_owned_path_vec(&[dir1_path]);
    let stats = locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns.clone())
        .unwrap();
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 3,
            skipped_files: 0,
        }
    );
    assert_eq!(
        locked_ws.locked_wc().sparse_patterns().unwrap(),
        sparse_patterns
    );
    assert!(!root_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(!root_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_subdir1_file1_path
        .to_fs_path(&working_copy_path)
        .exists());
    assert!(!dir2_file1_path.to_fs_path(&working_copy_path).exists());

    // Write the new state to disk
    locked_ws.finish(repo.op_id().clone()).unwrap();
    let wc: &LocalWorkingCopy = ws.working_copy().as_any().downcast_ref().unwrap();
    assert_eq!(
        wc.file_states().unwrap().paths().collect_vec(),
        vec![dir1_file1_path, dir1_file2_path, dir1_subdir1_file1_path]
    );
    assert_eq!(wc.sparse_patterns().unwrap(), sparse_patterns);

    // Reload the state to check that it was persisted
    let wc = LocalWorkingCopy::load(
        repo.store().clone(),
        wc.path().to_path_buf(),
        wc.state_path().to_path_buf(),
    );
    assert_eq!(
        wc.file_states().unwrap().paths().collect_vec(),
        vec![dir1_file1_path, dir1_file2_path, dir1_subdir1_file1_path]
    );
    assert_eq!(wc.sparse_patterns().unwrap(), sparse_patterns);

    // Set sparse patterns to file2, dir1/subdir1/ and dir2/
    let mut locked_wc = wc.start_mutation().unwrap();
    let sparse_patterns = to_owned_path_vec(&[root_file1_path, dir1_subdir1_path, dir2_path]);
    let stats = locked_wc
        .set_sparse_patterns(sparse_patterns.clone())
        .unwrap();
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 2,
            removed_files: 2,
            skipped_files: 0,
        }
    );
    assert_eq!(locked_wc.sparse_patterns().unwrap(), sparse_patterns);
    assert!(root_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(!root_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(!dir1_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(!dir1_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_subdir1_file1_path
        .to_fs_path(&working_copy_path)
        .exists());
    assert!(dir2_file1_path.to_fs_path(&working_copy_path).exists());
    let wc = locked_wc.finish(repo.op_id().clone()).unwrap();
    let wc: &LocalWorkingCopy = wc.as_any().downcast_ref().unwrap();
    assert_eq!(
        wc.file_states().unwrap().paths().collect_vec(),
        vec![dir1_subdir1_file1_path, dir2_file1_path, root_file1_path]
    );
}

/// Test that sparse patterns are respected on commit
#[test]
fn test_sparse_commit() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let op_id = repo.op_id().clone();
    let working_copy_path = test_workspace.workspace.workspace_root().clone();

    let root_file1_path = RepoPath::from_internal_string("file1");
    let dir1_path = RepoPath::from_internal_string("dir1");
    let dir1_file1_path = RepoPath::from_internal_string("dir1/file1");
    let dir2_path = RepoPath::from_internal_string("dir2");
    let dir2_file1_path = RepoPath::from_internal_string("dir2/file1");

    let tree = create_tree(
        repo,
        &[
            (root_file1_path, "contents"),
            (dir1_file1_path, "contents"),
            (dir2_file1_path, "contents"),
        ],
    );

    let commit = commit_with_tree(repo.store(), tree.id());
    test_workspace
        .workspace
        .check_out(repo.op_id().clone(), None, &commit)
        .unwrap();

    // Set sparse patterns to only dir1/
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .unwrap();
    let sparse_patterns = to_owned_path_vec(&[dir1_path]);
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .unwrap();
    locked_ws.finish(repo.op_id().clone()).unwrap();

    // Write modified version of all files, including files that are not in the
    // sparse patterns.
    std::fs::write(root_file1_path.to_fs_path(&working_copy_path), "modified").unwrap();
    std::fs::write(dir1_file1_path.to_fs_path(&working_copy_path), "modified").unwrap();
    std::fs::create_dir(dir2_path.to_fs_path(&working_copy_path)).unwrap();
    std::fs::write(dir2_file1_path.to_fs_path(&working_copy_path), "modified").unwrap();

    // Create a tree from the working copy. Only dir1/file1 should be updated in the
    // tree.
    let modified_tree = test_workspace.snapshot().unwrap();
    let diff = tree.diff(&modified_tree, &EverythingMatcher).collect_vec();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].0.as_ref(), dir1_file1_path);

    // Set sparse patterns to also include dir2/
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .unwrap();
    let sparse_patterns = to_owned_path_vec(&[dir1_path, dir2_path]);
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .unwrap();
    locked_ws.finish(op_id).unwrap();

    // Create a tree from the working copy. Only dir1/file1 and dir2/file1 should be
    // updated in the tree.
    let modified_tree = test_workspace.snapshot().unwrap();
    let diff = tree.diff(&modified_tree, &EverythingMatcher).collect_vec();
    assert_eq!(diff.len(), 2);
    assert_eq!(diff[0].0.as_ref(), dir1_file1_path);
    assert_eq!(diff[1].0.as_ref(), dir2_file1_path);
}

#[test]
fn test_sparse_commit_gitignore() {
    // Test that (untracked) .gitignore files in parent directories are respected
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let working_copy_path = test_workspace.workspace.workspace_root().clone();

    let dir1_path = RepoPath::from_internal_string("dir1");
    let dir1_file1_path = RepoPath::from_internal_string("dir1/file1");
    let dir1_file2_path = RepoPath::from_internal_string("dir1/file2");

    // Set sparse patterns to only dir1/
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .unwrap();
    let sparse_patterns = to_owned_path_vec(&[dir1_path]);
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .unwrap();
    locked_ws.finish(repo.op_id().clone()).unwrap();

    // Write dir1/file1 and dir1/file2 and a .gitignore saying to ignore dir1/file1
    std::fs::write(working_copy_path.join(".gitignore"), "dir1/file1").unwrap();
    std::fs::create_dir(dir1_path.to_fs_path(&working_copy_path)).unwrap();
    std::fs::write(dir1_file1_path.to_fs_path(&working_copy_path), "contents").unwrap();
    std::fs::write(dir1_file2_path.to_fs_path(&working_copy_path), "contents").unwrap();

    // Create a tree from the working copy. Only dir1/file2 should be updated in the
    // tree because dir1/file1 is ignored.
    let modified_tree = test_workspace.snapshot().unwrap();
    let entries = modified_tree.entries().collect_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0.as_ref(), dir1_file2_path);
}
