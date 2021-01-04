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

use std::thread;

use jujube_lib::commit_builder::CommitBuilder;
use jujube_lib::repo::ReadonlyRepo;
use jujube_lib::repo_path::FileRepoPath;
use jujube_lib::store::CommitId;
use jujube_lib::testutils;
use jujube_lib::working_copy::CheckoutError;
use std::collections::HashSet;
use std::sync::Arc;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_concurrent_checkout(use_git: bool) {
    // Test that we error out if a concurrent checkout is detected (i.e. if the
    // current checkout changed on disk after we read it).
    let settings = testutils::user_settings();
    let (_temp_dir, repo1) = testutils::init_repo(&settings, use_git);

    let commit1 = testutils::create_random_commit(&settings, &repo1)
        .set_open(true)
        .write_to_new_transaction(&repo1, "test");
    let commit2 = testutils::create_random_commit(&settings, &repo1)
        .set_open(true)
        .write_to_new_transaction(&repo1, "test");
    let commit3 = testutils::create_random_commit(&settings, &repo1)
        .set_open(true)
        .write_to_new_transaction(&repo1, "test");

    // Check out commit1
    let wc1 = repo1.working_copy_locked();
    wc1.check_out(commit1).unwrap();

    // Check out commit2 from another process (simulated by another repo instance)
    let repo2 = ReadonlyRepo::load(&settings, repo1.working_copy_path().clone()).unwrap();
    repo2
        .working_copy_locked()
        .check_out(commit2.clone())
        .unwrap();

    // Checking out another commit (via the first repo instance) should now fail.
    assert_eq!(
        wc1.check_out(commit3),
        Err(CheckoutError::ConcurrentCheckout)
    );

    // Check that the commit2 is still checked out on disk.
    let repo3 = ReadonlyRepo::load(&settings, repo1.working_copy_path().clone()).unwrap();
    assert_eq!(
        repo3.working_copy_locked().current_tree_id(),
        commit2.tree().id().clone()
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_concurrent_commit(use_git: bool) {
    // Test that concurrent working copy commits result in a chain of successors
    // instead of divergence.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo1) = testutils::init_repo(&settings, use_git);

    let owned_wc1 = repo1.working_copy().clone();
    let wc1 = owned_wc1.lock().unwrap();
    let commit1 = wc1.current_commit();

    // Commit from another process (simulated by another repo instance)
    let mut repo2 = ReadonlyRepo::load(&settings, repo1.working_copy_path().clone()).unwrap();
    testutils::write_working_copy_file(&repo2, &FileRepoPath::from("file2"), "contents2");
    let owned_wc2 = repo2.working_copy().clone();
    let wc2 = owned_wc2.lock().unwrap();
    let commit2 = wc2.commit(&settings, Arc::get_mut(&mut repo2).unwrap());

    assert_eq!(commit2.predecessors(), vec![commit1]);

    // Creating another commit  (via the first repo instance)  should result in a
    // successor of the commit created from the other process.
    testutils::write_working_copy_file(&repo1, &FileRepoPath::from("file3"), "contents3");
    let commit3 = wc1.commit(&settings, Arc::get_mut(&mut repo1).unwrap());
    assert_eq!(commit3.predecessors(), vec![commit2]);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_checkout_parallel(use_git: bool) {
    // Test that concurrent checkouts by different processes (simulated by using
    // different repo instances) is safe.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let mut commit_ids = vec![];
    for i in 0..100 {
        let path = FileRepoPath::from(format!("file{}", i).as_str());
        let tree = testutils::create_tree(&repo, &[(&path, "contents")]);
        let commit = CommitBuilder::for_new_commit(&settings, store, tree.id().clone())
            .set_open(true)
            .write_to_new_transaction(&repo, "test");
        commit_ids.push(commit.id().clone());
    }

    // Create another commit just so we can test the update stats reliably from the
    // first update
    let tree = testutils::create_tree(&repo, &[(&FileRepoPath::from("other file"), "contents")]);
    let mut tx = repo.start_transaction("test");
    let commit = CommitBuilder::for_new_commit(&settings, store, tree.id().clone())
        .set_open(true)
        .write_to_transaction(&mut tx);
    repo.working_copy_locked().check_out(commit).unwrap();
    tx.commit();

    let mut threads = vec![];
    let commit_ids_set: HashSet<CommitId> = commit_ids.iter().cloned().collect();
    for commit_id in &commit_ids {
        let commit_ids_set = commit_ids_set.clone();
        let commit_id = commit_id.clone();
        let settings = settings.clone();
        let working_copy_path = repo.working_copy_path().clone();
        let handle = thread::spawn(move || {
            let mut repo = ReadonlyRepo::load(&settings, working_copy_path).unwrap();
            let owned_wc = repo.working_copy().clone();
            let wc = owned_wc.lock().unwrap();
            let commit = repo.store().get_commit(&commit_id).unwrap();
            let stats = wc.check_out(commit).unwrap();
            assert_eq!(stats.updated_files, 0);
            assert_eq!(stats.added_files, 1);
            assert_eq!(stats.removed_files, 1);
            // Check that the working copy contains one of the commits. We may see a
            // different commit than the one we just checked out, but since
            // commit() should take the same lock as check_out(), commit()
            // should never produce a different tree (resulting in a different commit).
            let commit_after = wc.commit(&settings, Arc::get_mut(&mut repo).unwrap());
            assert!(commit_ids_set.contains(commit_after.id()));
        });
        threads.push(handle);
    }
    for thread in threads {
        thread.join().ok().unwrap();
    }
}
