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

use std::cmp::max;
use std::thread;

use assert_matches::assert_matches;
use jj_lib::backend::{ObjectId, TreeId};
use jj_lib::op_store::OperationId;
use jj_lib::repo::{Repo, StoreFactories};
use jj_lib::repo_path::RepoPath;
use jj_lib::working_copy::{CheckoutError, SnapshotOptions};
use jj_lib::workspace::Workspace;
use testutils::{write_working_copy_file, TestWorkspace};

#[test]
fn test_concurrent_checkout() {
    // Test that we error out if a concurrent checkout is detected (i.e. if the
    // working-copy commit changed on disk after we read it).
    let settings = testutils::user_settings();
    let mut test_workspace1 = TestWorkspace::init(&settings, true);
    let repo1 = test_workspace1.repo.clone();
    let workspace1_root = test_workspace1.workspace.workspace_root().clone();

    let tree_id1 = testutils::create_random_tree(&repo1);
    let tree_id2 = testutils::create_random_tree(&repo1);
    let tree_id3 = testutils::create_random_tree(&repo1);
    let tree1 = repo1
        .store()
        .get_tree(&RepoPath::root(), &tree_id1)
        .unwrap();
    let tree2 = repo1
        .store()
        .get_tree(&RepoPath::root(), &tree_id2)
        .unwrap();
    let tree3 = repo1
        .store()
        .get_tree(&RepoPath::root(), &tree_id3)
        .unwrap();

    // Check out tree1
    let wc1 = test_workspace1.workspace.working_copy_mut();
    // The operation ID is not correct, but that doesn't matter for this test
    wc1.check_out(repo1.op_id().clone(), None, &tree1).unwrap();

    // Check out tree2 from another process (simulated by another workspace
    // instance)
    let mut workspace2 =
        Workspace::load(&settings, &workspace1_root, &StoreFactories::default()).unwrap();
    workspace2
        .working_copy_mut()
        .check_out(repo1.op_id().clone(), Some(&tree_id1), &tree2)
        .unwrap();

    // Checking out another tree (via the first repo instance) should now fail.
    assert_matches!(
        wc1.check_out(repo1.op_id().clone(), Some(&tree_id1), &tree3),
        Err(CheckoutError::ConcurrentCheckout)
    );

    // Check that the tree2 is still checked out on disk.
    let workspace3 =
        Workspace::load(&settings, &workspace1_root, &StoreFactories::default()).unwrap();
    assert_eq!(
        workspace3.working_copy().current_tree_id().unwrap(),
        &tree_id2
    );
}

#[test]
fn test_checkout_parallel() {
    // Test that concurrent checkouts by different processes (simulated by using
    // different repo instances) is safe.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, true);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let num_threads = max(num_cpus::get(), 4);
    let mut tree_ids = vec![];
    for i in 0..num_threads {
        let path = RepoPath::from_internal_string(format!("file{i}").as_str());
        let tree = testutils::create_tree(repo, &[(&path, "contents")]);
        tree_ids.push(tree.id().clone());
    }

    // Create another tree just so we can test the update stats reliably from the
    // first update
    let tree = testutils::create_tree(
        repo,
        &[(&RepoPath::from_internal_string("other file"), "contents")],
    );
    test_workspace
        .workspace
        .working_copy_mut()
        .check_out(repo.op_id().clone(), None, &tree)
        .unwrap();

    thread::scope(|s| {
        for tree_id in &tree_ids {
            let op_id = repo.op_id().clone();
            let tree_ids = tree_ids.clone();
            let tree_id = tree_id.clone();
            let settings = settings.clone();
            let workspace_root = workspace_root.clone();
            s.spawn(move || {
                let mut workspace =
                    Workspace::load(&settings, &workspace_root, &StoreFactories::default())
                        .unwrap();
                let tree = workspace
                    .repo_loader()
                    .store()
                    .get_tree(&RepoPath::root(), &tree_id)
                    .unwrap();
                // The operation ID is not correct, but that doesn't matter for this test
                let stats = workspace
                    .working_copy_mut()
                    .check_out(op_id, None, &tree)
                    .unwrap();
                assert_eq!(stats.updated_files, 0);
                assert_eq!(stats.added_files, 1);
                assert_eq!(stats.removed_files, 1);
                // Check that the working copy contains one of the trees. We may see a
                // different tree than the one we just checked out, but since
                // write_tree() should take the same lock as check_out(), write_tree()
                // should never produce a different tree.
                let mut locked_wc = workspace.working_copy_mut().start_mutation().unwrap();
                let new_tree_id = locked_wc
                    .snapshot(SnapshotOptions::empty_for_test())
                    .unwrap();
                locked_wc.discard();
                assert!(tree_ids.contains(&new_tree_id));
            });
        }
    });
}

#[test]
fn test_racy_checkout() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, true);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let path = RepoPath::from_internal_string("file");
    let tree = testutils::create_tree(repo, &[(&path, "1")]);

    fn snapshot(workspace: &mut Workspace) -> TreeId {
        let mut locked_wc = workspace.working_copy_mut().start_mutation().unwrap();
        let tree_id = locked_wc
            .snapshot(SnapshotOptions::empty_for_test())
            .unwrap();
        locked_wc.finish(OperationId::from_hex("abc123")).unwrap();
        tree_id
    }

    let mut num_matches = 0;
    for _ in 0..100 {
        let wc = test_workspace.workspace.working_copy_mut();
        wc.check_out(repo.op_id().clone(), None, &tree).unwrap();
        assert_eq!(
            std::fs::read(path.to_fs_path(&workspace_root)).unwrap(),
            b"1".to_vec()
        );
        // A file written right after checkout (hopefully, from the test's perspective,
        // within the file system timestamp granularity) is detected as changed.
        write_working_copy_file(&workspace_root, &path, "x");
        let modified_tree_id = snapshot(&mut test_workspace.workspace);
        if modified_tree_id == *tree.id() {
            num_matches += 1;
        }
        // Reset the state for the next round
        write_working_copy_file(&workspace_root, &path, "1");
    }
    assert_eq!(num_matches, 0);
}
