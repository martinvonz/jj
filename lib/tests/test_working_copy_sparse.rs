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

use itertools::Itertools;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::testutils;
use jujutsu_lib::working_copy::{CheckoutStats, WorkingCopy};

#[test]
fn test_sparse_checkout() {
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, false);
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

    let tree = testutils::create_tree(
        repo,
        &[
            (&root_file1_path, "contents"),
            (&root_file2_path, "contents"),
            (&dir1_file1_path, "contents"),
            (&dir1_file2_path, "contents"),
            (&dir1_subdir1_file1_path, "contents"),
            (&dir2_file1_path, "contents"),
        ],
    );

    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, &tree).unwrap();

    // Set sparse patterns to only dir1/
    let mut locked_wc = wc.start_mutation();
    let sparse_patterns = vec![dir1_path];
    let stats = locked_wc
        .set_sparse_patterns(sparse_patterns.clone())
        .unwrap();
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 3
        }
    );
    assert_eq!(locked_wc.sparse_patterns(), sparse_patterns);
    assert!(!root_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(!root_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_subdir1_file1_path
        .to_fs_path(&working_copy_path)
        .exists());
    assert!(!dir2_file1_path.to_fs_path(&working_copy_path).exists());

    // Write the new state to disk
    locked_wc.finish(repo.op_id().clone());
    assert_eq!(
        wc.file_states().keys().collect_vec(),
        vec![&dir1_file1_path, &dir1_file2_path, &dir1_subdir1_file1_path]
    );
    assert_eq!(wc.sparse_patterns(), sparse_patterns);

    // Reload the state to check that it was persisted
    let mut wc = WorkingCopy::load(
        repo.store().clone(),
        wc.working_copy_path().to_path_buf(),
        wc.state_path().to_path_buf(),
    );
    assert_eq!(
        wc.file_states().keys().collect_vec(),
        vec![&dir1_file1_path, &dir1_file2_path, &dir1_subdir1_file1_path]
    );
    assert_eq!(wc.sparse_patterns(), sparse_patterns);

    // Set sparse patterns to file2, dir1/subdir1/ and dir2/
    let mut locked_wc = wc.start_mutation();
    let sparse_patterns = vec![root_file1_path.clone(), dir1_subdir1_path, dir2_path];
    let stats = locked_wc
        .set_sparse_patterns(sparse_patterns.clone())
        .unwrap();
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 2,
            removed_files: 2
        }
    );
    assert_eq!(locked_wc.sparse_patterns(), sparse_patterns);
    assert!(root_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(!root_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(!dir1_file1_path.to_fs_path(&working_copy_path).exists());
    assert!(!dir1_file2_path.to_fs_path(&working_copy_path).exists());
    assert!(dir1_subdir1_file1_path
        .to_fs_path(&working_copy_path)
        .exists());
    assert!(dir2_file1_path.to_fs_path(&working_copy_path).exists());
    locked_wc.finish(repo.op_id().clone());
    assert_eq!(
        wc.file_states().keys().collect_vec(),
        vec![&dir1_subdir1_file1_path, &dir2_file1_path, &root_file1_path]
    );
}
