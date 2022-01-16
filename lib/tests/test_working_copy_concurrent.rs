// Copyright 2020 Google LLC
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
use std::collections::HashSet;
use std::thread;

use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::testutils;
use jujutsu_lib::working_copy::CheckoutError;
use jujutsu_lib::workspace::Workspace;
use test_case::test_case;

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_concurrent_checkout(use_git: bool) {
    // Test that we error out if a concurrent checkout is detected (i.e. if the
    // current checkout changed on disk after we read it).
    let settings = testutils::user_settings();
    let mut test_workspace1 = testutils::init_repo(&settings, use_git);
    let repo1 = test_workspace1.repo.clone();
    let workspace1_root = test_workspace1.workspace.workspace_root().clone();

    let mut tx1 = repo1.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo1)
        .set_open(true)
        .write_to_repo(tx1.mut_repo());
    let commit2 = testutils::create_random_commit(&settings, &repo1)
        .set_open(true)
        .write_to_repo(tx1.mut_repo());
    let commit3 = testutils::create_random_commit(&settings, &repo1)
        .set_open(true)
        .write_to_repo(tx1.mut_repo());
    tx1.commit();

    // Check out commit1
    let wc1 = test_workspace1.workspace.working_copy_mut();
    let commit_id1 = commit1.id().clone();
    wc1.check_out(None, commit1).unwrap();

    // Check out commit2 from another process (simulated by another workspace
    // instance)
    let mut workspace2 = Workspace::load(&settings, workspace1_root.clone()).unwrap();
    workspace2
        .working_copy_mut()
        .check_out(Some(&commit_id1), commit2.clone())
        .unwrap();

    // Checking out another commit (via the first repo instance) should now fail.
    assert_eq!(
        wc1.check_out(Some(&commit_id1), commit3),
        Err(CheckoutError::ConcurrentCheckout)
    );

    // Check that the commit2 is still checked out on disk.
    let workspace3 = Workspace::load(&settings, workspace1_root).unwrap();
    assert_eq!(
        workspace3.working_copy().current_tree_id(),
        commit2.tree().id().clone()
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_checkout_parallel(use_git: bool) {
    // Test that concurrent checkouts by different processes (simulated by using
    // different repo instances) is safe.
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_repo(&settings, use_git);
    let repo = &test_workspace.repo;
    let store = repo.store();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let num_threads = max(num_cpus::get(), 4);
    let mut tree_ids = HashSet::new();
    let mut commit_ids = vec![];
    let mut tx = repo.start_transaction("test");
    for i in 0..num_threads {
        let path = RepoPath::from_internal_string(format!("file{}", i).as_str());
        let tree = testutils::create_tree(repo, &[(&path, "contents")]);
        tree_ids.insert(tree.id().clone());
        let commit = CommitBuilder::for_new_commit(&settings, store, tree.id().clone())
            .set_open(true)
            .write_to_repo(tx.mut_repo());
        commit_ids.push(commit.id().clone());
    }

    // Create another commit just so we can test the update stats reliably from the
    // first update
    let tree = testutils::create_tree(
        repo,
        &[(&RepoPath::from_internal_string("other file"), "contents")],
    );
    let commit = CommitBuilder::for_new_commit(&settings, store, tree.id().clone())
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    tx.commit();
    test_workspace
        .workspace
        .working_copy_mut()
        .check_out(None, commit)
        .unwrap();

    let mut threads = vec![];
    for commit_id in &commit_ids {
        let tree_ids = tree_ids.clone();
        let commit_id = commit_id.clone();
        let settings = settings.clone();
        let workspace_root = workspace_root.clone();
        let handle = thread::spawn(move || {
            let mut workspace = Workspace::load(&settings, workspace_root).unwrap();
            let commit = workspace
                .repo_loader()
                .store()
                .get_commit(&commit_id)
                .unwrap();
            let stats = workspace
                .working_copy_mut()
                .check_out(None, commit)
                .unwrap();
            assert_eq!(stats.updated_files, 0);
            assert_eq!(stats.added_files, 1);
            assert_eq!(stats.removed_files, 1);
            // Check that the working copy contains one of the trees. We may see a
            // different tree than the one we just checked out, but since
            // write_tree() should take the same lock as check_out(), write_tree()
            // should never produce a different tree.
            let mut locked_wc = workspace.working_copy_mut().start_mutation();
            let new_tree_id = locked_wc.write_tree();
            locked_wc.discard();
            assert!(tree_ids.contains(&new_tree_id));
        });
        threads.push(handle);
    }
    for thread in threads {
        thread.join().ok().unwrap();
    }
}
