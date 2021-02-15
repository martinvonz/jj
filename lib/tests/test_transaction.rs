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

use jujube_lib::commit_builder::CommitBuilder;
use jujube_lib::repo_path::FileRepoPath;
use jujube_lib::store::{Conflict, ConflictId, ConflictPart, TreeValue};
use jujube_lib::store_wrapper::StoreWrapper;
use jujube_lib::testutils;
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
    // Test that Transaction::check_out() creates a child if the requested
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
    assert_eq!(actual_checkout.parents().len(), 1);
    assert_eq!(actual_checkout.parents()[0].id(), requested_checkout.id());
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
    assert!(!tx.evolution().is_obsolete(old_checkout.id()));
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
    assert!(tx.evolution().is_obsolete(old_checkout.id()));
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
    let successors = tx.evolution().successors(old_checkout.id());
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
    assert!(tx.evolution().successors(old_checkout.id()).is_empty());
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_add_head_success(use_git: bool) {
    // Test that Transaction::add_head() adds the head, and that it's still there
    // after commit. It should also be indexed.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    // Create a commit outside of the repo by using a temporary transaction. Then
    // add that as a head.
    let mut tx = repo.start_transaction("test");
    let new_commit =
        testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    tx.discard();

    let mut tx = repo.start_transaction("test");
    assert!(!tx.view().heads().contains(new_commit.id()));
    assert!(!tx.index().has_id(new_commit.id()));
    tx.add_head(&new_commit);
    assert!(tx.view().heads().contains(new_commit.id()));
    assert!(tx.index().has_id(new_commit.id()));
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert!(repo.view().heads().contains(new_commit.id()));
    assert!(repo.index().has_id(new_commit.id()));
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_add_head_ancestor(use_git: bool) {
    // Test that Transaction::add_head() does not add a head if it's an ancestor of
    // an existing head.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_transaction(&mut tx);
    let _commit3 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit2.id().clone()])
        .write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    tx.add_head(&commit1);
    assert!(!tx.view().heads().contains(commit1.id()));
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_add_head_not_immediate_child(use_git: bool) {
    // Test that Transaction::add_head() can be used for adding a head that is not
    // an immediate child of a current head.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let initial = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    // Create some commit outside of the repo by using a temporary transaction. Then
    // add one of them as a head.
    let mut tx = repo.start_transaction("test");
    let rewritten = testutils::create_random_commit(&settings, &repo)
        .set_change_id(initial.change_id().clone())
        .set_predecessors(vec![initial.id().clone()])
        .write_to_transaction(&mut tx);
    let child = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![rewritten.id().clone()])
        .write_to_transaction(&mut tx);
    tx.discard();

    let mut tx = repo.start_transaction("test");
    tx.add_head(&child);
    assert!(tx.view().heads().contains(initial.id()));
    assert!(!tx.view().heads().contains(rewritten.id()));
    assert!(tx.view().heads().contains(child.id()));
    assert!(tx.index().has_id(initial.id()));
    assert!(tx.index().has_id(rewritten.id()));
    assert!(tx.index().has_id(child.id()));
    assert!(tx.evolution().is_obsolete(initial.id()));
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_remove_head(use_git: bool) {
    // Test that Transaction::remove_head() removes the head, and that it's still
    // removed after commit. It should remain in the index, since we otherwise would
    // have to reindex everything.
    // TODO: Consider if it's better to have the index be exactly the commits
    // reachable from the view's heads. We would probably want to add tombstones
    // for commits no longer visible in that case so we don't have to reindex e.g.
    // when the user does `jj op undo`.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_transaction(&mut tx);
    let commit3 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit2.id().clone()])
        .write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    assert!(tx.view().heads().contains(commit3.id()));
    tx.remove_head(&commit3);
    let heads = tx.view().heads().clone();
    assert!(!heads.contains(commit3.id()));
    assert!(!heads.contains(commit2.id()));
    assert!(!heads.contains(commit1.id()));
    assert!(tx.index().has_id(commit1.id()));
    assert!(tx.index().has_id(commit2.id()));
    assert!(tx.index().has_id(commit3.id()));
    tx.commit();

    Arc::get_mut(&mut repo).unwrap().reload();
    let heads = repo.view().heads().clone();
    assert!(!heads.contains(commit3.id()));
    assert!(!heads.contains(commit2.id()));
    assert!(!heads.contains(commit1.id()));
    assert!(repo.index().has_id(commit1.id()));
    assert!(repo.index().has_id(commit2.id()));
    assert!(repo.index().has_id(commit3.id()));
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_remove_head_ancestor_git_ref(use_git: bool) {
    // Test that Transaction::remove_head() does not leave the view with a git ref
    // pointing to a commit that's not reachable by any head.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_transaction(&mut tx);
    let commit3 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit2.id().clone()])
        .write_to_transaction(&mut tx);
    tx.insert_git_ref("refs/heads/main".to_string(), commit1.id().clone());
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    let heads = tx.view().heads().clone();
    assert!(heads.contains(commit3.id()));
    tx.remove_head(&commit3);
    let heads = tx.view().heads().clone();
    assert!(!heads.contains(commit3.id()));
    assert!(!heads.contains(commit2.id()));
    assert!(heads.contains(commit1.id()));
    tx.commit();

    Arc::get_mut(&mut repo).unwrap().reload();
    let heads = repo.view().heads().clone();
    assert!(!heads.contains(commit3.id()));
    assert!(!heads.contains(commit2.id()));
    assert!(heads.contains(commit1.id()));
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_add_public_head(use_git: bool) {
    // Test that Transaction::add_public_head() adds the head, and that it's still
    // there after commit.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    assert!(!tx.view().public_heads().contains(commit1.id()));
    tx.add_public_head(&commit1);
    assert!(tx.view().public_heads().contains(commit1.id()));
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert!(repo.view().public_heads().contains(commit1.id()));
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_add_public_head_ancestor(use_git: bool) {
    // Test that Transaction::add_public_head() does not add a public head if it's
    // an ancestor of an existing public head.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_transaction(&mut tx);
    tx.add_public_head(&commit2);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    assert!(!tx.view().public_heads().contains(commit1.id()));
    tx.add_public_head(&commit1);
    assert!(!tx.view().public_heads().contains(commit1.id()));
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert!(!repo.view().public_heads().contains(commit1.id()));
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_remove_public_head(use_git: bool) {
    // Test that Transaction::remove_public_head() removes the head, and that it's
    // still removed after commit.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx);
    tx.add_public_head(&commit1);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    assert!(tx.view().public_heads().contains(commit1.id()));
    tx.remove_public_head(&commit1);
    assert!(!tx.view().public_heads().contains(commit1.id()));
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();
    assert!(!repo.view().public_heads().contains(commit1.id()));
}
