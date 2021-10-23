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

use std::sync::Arc;

use jujutsu_lib::backend::{Conflict, ConflictId, ConflictPart, TreeValue};
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::store::Store;
use jujutsu_lib::testutils;
use jujutsu_lib::testutils::{assert_rebased, CommitGraphBuilder};
use test_case::test_case;

// TODO Many of the tests here are not run with Git because they end up creating
// two commits with the same contents.

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_checkout_open(use_git: bool) {
    // Test that MutableRepo::check_out() uses the requested commit if it's open
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let requested_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.mut_repo().check_out(&settings, &requested_checkout);
    assert_eq!(actual_checkout.id(), requested_checkout.id());
    let repo = tx.commit();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_checkout_closed(use_git: bool) {
    // Test that MutableRepo::check_out() creates a child if the requested commit is
    // closed
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let requested_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(false)
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.mut_repo().check_out(&settings, &requested_checkout);
    assert_eq!(actual_checkout.tree().id(), requested_checkout.tree().id());
    assert_eq!(actual_checkout.parents().len(), 1);
    assert_eq!(actual_checkout.parents()[0].id(), requested_checkout.id());
    let repo = tx.commit();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_checkout_open_with_conflict(use_git: bool) {
    // Test that MutableRepo::check_out() creates a child if the requested
    // commit is open and has conflicts
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let file_path = RepoPath::from_internal_string("file");
    let conflict_id = write_conflict(store, &file_path);
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    tree_builder.set(file_path.clone(), TreeValue::Conflict(conflict_id));
    let tree_id = tree_builder.write_tree();

    let mut tx = repo.start_transaction("test");
    let requested_checkout = CommitBuilder::for_new_commit(&settings, store, tree_id)
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.mut_repo().check_out(&settings, &requested_checkout);
    let file_value = actual_checkout.tree().path_value(&file_path);
    match file_value {
        Some(TreeValue::Normal {
            id: _,
            executable: false,
        }) => {}
        _ => panic!("unexpected tree value: {:?}", file_value),
    }
    assert_eq!(actual_checkout.parents().len(), 1);
    assert_eq!(actual_checkout.parents()[0].id(), requested_checkout.id());
    let repo = tx.commit();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_checkout_closed_with_conflict(use_git: bool) {
    // Test that MutableRepo::check_out() creates a child if the requested commit is
    // closed and has conflicts
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let file_path = RepoPath::from_internal_string("file");
    let conflict_id = write_conflict(store, &file_path);
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    tree_builder.set(file_path.clone(), TreeValue::Conflict(conflict_id));
    let tree_id = tree_builder.write_tree();

    let mut tx = repo.start_transaction("test");
    let requested_checkout = CommitBuilder::for_new_commit(&settings, store, tree_id)
        .set_open(false)
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let actual_checkout = tx.mut_repo().check_out(&settings, &requested_checkout);
    let file_value = actual_checkout.tree().path_value(&file_path);
    match file_value {
        Some(TreeValue::Normal {
            id: _,
            executable: false,
        }) => {}
        _ => panic!("unexpected tree value: {:?}", file_value),
    }
    assert_eq!(actual_checkout.parents().len(), 1);
    assert_eq!(actual_checkout.parents()[0].id(), requested_checkout.id());
    let repo = tx.commit();
    assert_eq!(repo.view().checkout(), actual_checkout.id());
}

fn write_conflict(store: &Arc<Store>, file_path: &RepoPath) -> ConflictId {
    let file_id1 = testutils::write_file(store, file_path, "a\n");
    let file_id2 = testutils::write_file(store, file_path, "b\n");
    let file_id3 = testutils::write_file(store, file_path, "c\n");
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

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_checkout_previous_not_empty(use_git: bool) {
    // Test that MutableRepo::check_out() does not usually abandon the previous
    // commit.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let old_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_repo(mut_repo);
    mut_repo.check_out(&settings, &old_checkout);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let new_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_repo(mut_repo);
    mut_repo.check_out(&settings, &new_checkout);
    mut_repo.create_descendant_rebaser(&settings).rebase_all();
    assert!(mut_repo.view().heads().contains(old_checkout.id()));
    tx.discard();
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_checkout_previous_empty(use_git: bool) {
    // Test that MutableRepo::check_out() abandons the previous commit if it was
    // empty.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let old_checkout = CommitBuilder::for_open_commit(
        &settings,
        repo.store(),
        repo.store().root_commit_id().clone(),
        repo.store().empty_tree_id().clone(),
    )
    .write_to_repo(mut_repo);
    mut_repo.check_out(&settings, &old_checkout);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let new_checkout = testutils::create_random_commit(&settings, &repo)
        .set_open(true)
        .write_to_repo(mut_repo);
    mut_repo.check_out(&settings, &new_checkout);
    mut_repo.create_descendant_rebaser(&settings).rebase_all();
    assert!(!mut_repo.view().heads().contains(old_checkout.id()));
    tx.discard();
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_add_head_success(use_git: bool) {
    // Test that MutableRepo::add_head() adds the head, and that it's still there
    // after commit. It should also be indexed.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    // Create a commit outside of the repo by using a temporary transaction. Then
    // add that as a head.
    let mut tx = repo.start_transaction("test");
    let new_commit = testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    tx.discard();

    let index_stats = repo.index().stats();
    assert_eq!(index_stats.num_heads, 1);
    assert_eq!(index_stats.num_commits, 2);
    assert_eq!(index_stats.max_generation_number, 1);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    assert!(!mut_repo.view().heads().contains(new_commit.id()));
    assert!(!mut_repo.index().has_id(new_commit.id()));
    mut_repo.add_head(&new_commit);
    assert!(mut_repo.view().heads().contains(new_commit.id()));
    assert!(mut_repo.index().has_id(new_commit.id()));
    let repo = tx.commit();
    assert!(repo.view().heads().contains(new_commit.id()));
    assert!(repo.index().has_id(new_commit.id()));
    let index_stats = repo.index().stats();
    assert_eq!(index_stats.num_heads, 2);
    assert_eq!(index_stats.num_commits, 3);
    assert_eq!(index_stats.max_generation_number, 1);
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_add_head_ancestor(use_git: bool) {
    // Test that MutableRepo::add_head() does not add a head if it's an ancestor of
    // an existing head.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let _commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let repo = tx.commit();

    let index_stats = repo.index().stats();
    assert_eq!(index_stats.num_heads, 2);
    assert_eq!(index_stats.num_commits, 5);
    assert_eq!(index_stats.max_generation_number, 3);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    mut_repo.add_head(&commit1);
    assert!(!mut_repo.view().heads().contains(commit1.id()));
    let index_stats = mut_repo.index().stats();
    assert_eq!(index_stats.num_heads, 2);
    assert_eq!(index_stats.num_commits, 5);
    assert_eq!(index_stats.max_generation_number, 3);
    tx.discard();
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_add_head_not_immediate_child(use_git: bool) {
    // Test that MutableRepo::add_head() can be used for adding a head that is not
    // an immediate child of a current head.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let initial = testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    // Create some commit outside of the repo by using a temporary transaction. Then
    // add one of them as a head.
    let mut tx = repo.start_transaction("test");
    let rewritten = testutils::create_random_commit(&settings, &repo)
        .set_change_id(initial.change_id().clone())
        .set_predecessors(vec![initial.id().clone()])
        .write_to_repo(tx.mut_repo());
    let child = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![rewritten.id().clone()])
        .write_to_repo(tx.mut_repo());
    tx.discard();

    let index_stats = repo.index().stats();
    assert_eq!(index_stats.num_heads, 2);
    assert_eq!(index_stats.num_commits, 3);
    assert_eq!(index_stats.max_generation_number, 1);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    mut_repo.add_head(&child);
    assert!(mut_repo.view().heads().contains(initial.id()));
    assert!(!mut_repo.view().heads().contains(rewritten.id()));
    assert!(mut_repo.view().heads().contains(child.id()));
    assert!(mut_repo.index().has_id(initial.id()));
    assert!(mut_repo.index().has_id(rewritten.id()));
    assert!(mut_repo.index().has_id(child.id()));
    let index_stats = mut_repo.index().stats();
    assert_eq!(index_stats.num_heads, 3);
    assert_eq!(index_stats.num_commits, 5);
    assert_eq!(index_stats.max_generation_number, 2);
    tx.discard();
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_remove_head(use_git: bool) {
    // Test that MutableRepo::remove_head() removes the head, and that it's still
    // removed after commit. It should remain in the index, since we otherwise would
    // have to reindex everything.
    // TODO: Consider if it's better to have the index be exactly the commits
    // reachable from the view's heads. We would probably want to add tombstones
    // for commits no longer visible in that case so we don't have to reindex e.g.
    // when the user does `jj op undo`.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    assert!(mut_repo.view().heads().contains(commit3.id()));
    mut_repo.remove_head(commit3.id());
    let heads = mut_repo.view().heads().clone();
    assert!(!heads.contains(commit3.id()));
    assert!(!heads.contains(commit2.id()));
    assert!(!heads.contains(commit1.id()));
    assert!(mut_repo.index().has_id(commit1.id()));
    assert!(mut_repo.index().has_id(commit2.id()));
    assert!(mut_repo.index().has_id(commit3.id()));
    let repo = tx.commit();
    let heads = repo.view().heads().clone();
    assert!(!heads.contains(commit3.id()));
    assert!(!heads.contains(commit2.id()));
    assert!(!heads.contains(commit1.id()));
    assert!(repo.index().has_id(commit1.id()));
    assert!(repo.index().has_id(commit2.id()));
    assert!(repo.index().has_id(commit3.id()));
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_add_public_head(use_git: bool) {
    // Test that MutableRepo::add_public_head() adds the head, and that it's still
    // there after commit.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    assert!(!mut_repo.view().public_heads().contains(commit1.id()));
    mut_repo.add_public_head(&commit1);
    assert!(mut_repo.view().public_heads().contains(commit1.id()));
    let repo = tx.commit();
    assert!(repo.view().public_heads().contains(commit1.id()));
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_add_public_head_ancestor(use_git: bool) {
    // Test that MutableRepo::add_public_head() does not add a public head if it's
    // an ancestor of an existing public head.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    tx.mut_repo().add_public_head(&commit2);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    assert!(!mut_repo.view().public_heads().contains(commit1.id()));
    mut_repo.add_public_head(&commit1);
    assert!(!mut_repo.view().public_heads().contains(commit1.id()));
    let repo = tx.commit();
    assert!(!repo.view().public_heads().contains(commit1.id()));
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_remove_public_head(use_git: bool) {
    // Test that MutableRepo::remove_public_head() removes the head, and that it's
    // still removed after commit.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    mut_repo.add_public_head(&commit1);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    assert!(mut_repo.view().public_heads().contains(commit1.id()));
    mut_repo.remove_public_head(commit1.id());
    assert!(!mut_repo.view().public_heads().contains(commit1.id()));
    let repo = tx.commit();
    assert!(!repo.view().public_heads().contains(commit1.id()));
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_rebase_descendants_simple(use_git: bool) {
    // Tests that MutableRepo::create_descendant_rebaser() creates a
    // DescendantRebaser that rebases descendants of rewritten and abandoned
    // commits.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1]);
    let commit5 = graph_builder.commit_with_parents(&[&commit4]);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit6 = graph_builder.commit_with_parents(&[&commit1]);
    mut_repo.record_rewritten_commit(commit2.id().clone(), commit6.id().clone());
    mut_repo.record_abandoned_commit(commit4.id().clone());
    let mut rebaser = mut_repo.create_descendant_rebaser(&settings);
    // Commit 3 got rebased onto commit 2's replacement, i.e. commit 6
    assert_rebased(rebaser.rebase_next(), &commit3, &[&commit6]);
    // Commit 5 got rebased onto commit 4's parent, i.e. commit 1
    assert_rebased(rebaser.rebase_next(), &commit5, &[&commit1]);
    assert!(rebaser.rebase_next().is_none());
    // No more descendants to rebase if we try again.
    assert!(mut_repo
        .create_descendant_rebaser(&settings)
        .rebase_next()
        .is_none());
    tx.discard();
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_rebase_descendants_conflicting_rewrite(use_git: bool) {
    // Tests MutableRepo::create_descendant_rebaser() when a commit has been marked
    // as rewritten to several other commits.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let _commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit4 = graph_builder.commit_with_parents(&[&commit1]);
    let commit5 = graph_builder.commit_with_parents(&[&commit1]);
    mut_repo.record_rewritten_commit(commit2.id().clone(), commit4.id().clone());
    mut_repo.record_rewritten_commit(commit2.id().clone(), commit5.id().clone());
    let mut rebaser = mut_repo.create_descendant_rebaser(&settings);
    // Commit 3 does *not* get rebased because it's unclear if it should go onto
    // commit 4 or commit 5
    assert!(rebaser.rebase_next().is_none());
    // No more descendants to rebase if we try again.
    assert!(mut_repo
        .create_descendant_rebaser(&settings)
        .rebase_next()
        .is_none());
    tx.discard();
}
