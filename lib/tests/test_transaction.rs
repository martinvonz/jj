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

use jj_lib::commit_builder::CommitBuilder;
use jj_lib::repo::Repo;
use jj_lib::repo_path::FileRepoPath;
use jj_lib::store::{Conflict, ConflictId, ConflictPart, TreeValue};
use jj_lib::store_wrapper::StoreWrapper;
use jj_lib::testutils;
use std::sync::Arc;
use test_case::test_case;

// TODO Many of the tests here are not run with Git because they end up creating
// two commits with the same contents.

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_open(use_git: bool) {
    // Test that Transaction::check_out() uses the requested commit if it's open
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let requested_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.check_out(&settings, &requested_checkout);
    assert_eq!(actual_checkout.id(), requested_checkout.id());
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_closed(use_git: bool) {
    // Test that Transaction::check_out() creates a child if the requested commit is
    // closed
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let requested_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(false)
        .write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.check_out(&settings, &requested_checkout);
    assert_eq!(actual_checkout.tree().id(), requested_checkout.tree().id());
    assert_eq!(actual_checkout.parents().len(), 1);
    assert_eq!(actual_checkout.parents()[0].id(), requested_checkout.id());
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_open_with_conflict(use_git: bool) {
    // Test that Transaction::check_out() creates a successor if the requested
    // commit is open and has conflicts
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let file_path = FileRepoPath::from("file");
    let conflict_id = write_conflict(store, &file_path);
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    tree_builder.set(file_path.to_repo_path(), TreeValue::Conflict(conflict_id));
    let tree_id = tree_builder.write_tree();

    let mut tx = repo.start_transaction("test");
    let requested_checkout = CommitBuilder::for_new_commit(&settings, store, tree_id)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.check_out(&settings, &requested_checkout);
    let file_value = actual_checkout.tree().path_value(&file_path.to_repo_path());
    match file_value {
        Some(TreeValue::Normal {
            id: _,
            executable: false,
        }) => {}
        _ => panic!("unexpected tree value: {:?}", file_value),
    }
    assert_eq!(actual_checkout.predecessors().len(), 1);
    assert_eq!(
        actual_checkout.predecessors()[0].id(),
        requested_checkout.id()
    );
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_closed_with_conflict(use_git: bool) {
    // Test that Transaction::check_out() creates a child if the requested commit is
    // closed and has conflicts
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let file_path = FileRepoPath::from("file");
    let conflict_id = write_conflict(store, &file_path);
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    tree_builder.set(file_path.to_repo_path(), TreeValue::Conflict(conflict_id));
    let tree_id = tree_builder.write_tree();

    let mut tx = repo.start_transaction("test");
    let requested_checkout = CommitBuilder::for_new_commit(&settings, store, tree_id)
        .set_open(false)
        .write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.check_out(&settings, &requested_checkout);
    let file_value = actual_checkout.tree().path_value(&file_path.to_repo_path());
    match file_value {
        Some(TreeValue::Normal {
            id: _,
            executable: false,
        }) => {}
        _ => panic!("unexpected tree value: {:?}", file_value),
    }
    assert_eq!(actual_checkout.parents().len(), 1);
    assert_eq!(actual_checkout.parents()[0].id(), requested_checkout.id());
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

fn write_conflict(store: &Arc<StoreWrapper>, file_path: &FileRepoPath) -> ConflictId {
    let file_id1 = testutils::write_file(store, &file_path, "a\n");
    let file_id2 = testutils::write_file(store, &file_path, "b\n");
    let file_id3 = testutils::write_file(store, &file_path, "c\n");
    let conflict = Conflict {
        removes: vec![ConflictPart {
            value: TreeValue::Normal {
                id: file_id1,
                executable: false,
            },
        }],
        adds: vec![
            ConflictPart {
                value: TreeValue::Normal {
                    id: file_id2,
                    executable: false,
                },
            },
            ConflictPart {
                value: TreeValue::Normal {
                    id: file_id3,
                    executable: false,
                },
            },
        ],
    };
    store.write_conflict(&conflict).unwrap()
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_previous_not_empty(use_git: bool) {
    // Test that Transaction::check_out() does not usually prune the previous
    // commit.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let old_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &old_checkout);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let new_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &new_checkout);
    assert!(!tx.as_repo().evolution().is_obsolete(old_checkout.id()));
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_previous_empty(use_git: bool) {
    // Test that Transaction::check_out() prunes the previous commit if it was
    // empty.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let old_checkout = CommitBuilder::for_open_commit(
        &settings,
        repo.store(),
        repo.store().root_commit_id().clone(),
        repo.store().empty_tree_id().clone(),
    )
    .write_to_transaction(&mut tx);
    tx.check_out(&settings, &old_checkout);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let new_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &new_checkout);
    assert!(tx.as_repo().evolution().is_obsolete(old_checkout.id()));
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_previous_empty_and_obsolete(use_git: bool) {
    // Test that Transaction::check_out() does not unnecessarily prune the previous
    // commit if it was empty but already obsolete.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let old_checkout = CommitBuilder::for_open_commit(
        &settings,
        repo.store(),
        repo.store().root_commit_id().clone(),
        repo.store().empty_tree_id().clone(),
    )
    .write_to_transaction(&mut tx);
    let successor = CommitBuilder::for_rewrite_from(&settings, repo.store(), &old_checkout)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &old_checkout);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let new_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &new_checkout);
    let successors = tx.as_repo().evolution().successors(old_checkout.id());
    assert_eq!(successors.len(), 1);
    assert_eq!(successors.iter().next().unwrap(), successor.id());
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_checkout_previous_empty_and_pruned(use_git: bool) {
    // Test that Transaction::check_out() does not unnecessarily prune the previous
    // commit if it was empty but already obsolete.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let old_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .set_pruned(true)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &old_checkout);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let new_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_transaction(&mut tx);
    tx.check_out(&settings, &new_checkout);
    assert!(tx
        .as_repo()
        .evolution()
        .successors(old_checkout.id())
        .is_empty());
    tx.discard();
}
