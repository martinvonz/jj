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
use jujube_lib::evolution::Evolution;
use jujube_lib::repo::RepoRef;
use jujube_lib::store::CommitId;
use jujube_lib::testutils;
use jujube_lib::view::View;
use std::path::Path;
use std::sync::Arc;
use test_case::test_case;

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
        .collect()
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_consecutive_operations(use_git: bool) {
    // Test that consecutive operations result in a single op-head on disk after
    // each operation
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let op_heads_dir = repo.repo_path().join("view").join("op_heads");
    let op_head_id0 = repo.view().base_op_head_id().clone();
    assert_eq!(
        list_dir(&op_heads_dir),
        vec![repo.view().base_op_head_id().hex()]
    );

    let mut tx1 = repo.start_transaction("transaction 1");
    testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx1);
    let op_head_id1 = tx1.commit().id().clone();
    assert_ne!(op_head_id1, op_head_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_head_id1.hex()]);

    Arc::get_mut(&mut repo).unwrap().reload();
    let mut tx2 = repo.start_transaction("transaction 2");
    testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx2);
    let op_head_id2 = tx2.commit().id().clone();
    assert_ne!(op_head_id2, op_head_id0);
    assert_ne!(op_head_id2, op_head_id1);
    assert_eq!(list_dir(&op_heads_dir), vec![op_head_id2.hex()]);

    // Reloading the repo makes no difference (there are no conflicting operations
    // to resolve).
    Arc::get_mut(&mut repo).unwrap().reload();
    assert_eq!(list_dir(&op_heads_dir), vec![op_head_id2.hex()]);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_concurrent_operations(use_git: bool) {
    // Test that consecutive operations result in multiple op-heads on disk until
    // the repo has been reloaded (which currently happens right away).
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let op_heads_dir = repo.repo_path().join("view").join("op_heads");
    let op_head_id0 = repo.view().base_op_head_id().clone();
    assert_eq!(
        list_dir(&op_heads_dir),
        vec![repo.view().base_op_head_id().hex()]
    );

    let mut tx1 = repo.start_transaction("transaction 1");
    testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx1);
    let op_head_id1 = tx1.commit().id().clone();
    assert_ne!(op_head_id1, op_head_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_head_id1.hex()]);

    // After both transactions have committed, we should have two op-heads on disk,
    // since they were run in parallel.
    let mut tx2 = repo.start_transaction("transaction 2");
    testutils::create_random_commit(&settings, &repo).write_to_transaction(&mut tx2);
    let op_head_id2 = tx2.commit().id().clone();
    assert_ne!(op_head_id2, op_head_id0);
    assert_ne!(op_head_id2, op_head_id1);
    let mut actual_heads_on_disk = list_dir(&op_heads_dir);
    actual_heads_on_disk.sort();
    let mut expected_heads_on_disk = vec![op_head_id1.hex(), op_head_id2.hex()];
    expected_heads_on_disk.sort();
    assert_eq!(actual_heads_on_disk, expected_heads_on_disk);

    // Reloading the repo causes the operations to be merged
    Arc::get_mut(&mut repo).unwrap().reload();
    let merged_op_head_id = repo.view().base_op_head_id().clone();
    assert_ne!(merged_op_head_id, op_head_id0);
    assert_ne!(merged_op_head_id, op_head_id1);
    assert_ne!(merged_op_head_id, op_head_id2);
    assert_eq!(list_dir(&op_heads_dir), vec![merged_op_head_id.hex()]);
}

fn assert_heads(repo: RepoRef, expected: Vec<&CommitId>) {
    let expected = expected.iter().cloned().cloned().collect();
    assert_eq!(*repo.view().heads(), expected);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_isolation(use_git: bool) {
    // Test that two concurrent transactions don't see each other's changes.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let wc_id = repo.working_copy_locked().current_commit_id();
    let initial = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write_to_new_transaction(&repo, "test");
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx1 = repo.start_transaction("transaction 1");
    let mut tx2 = repo.start_transaction("transaction 2");

    assert_heads(repo.as_repo_ref(), vec![&wc_id, initial.id()]);
    assert_heads(tx1.as_repo_ref(), vec![&wc_id, initial.id()]);
    assert_heads(tx2.as_repo_ref(), vec![&wc_id, initial.id()]);
    assert!(!repo.evolution().is_obsolete(initial.id()));
    assert!(!tx1.as_repo_ref().evolution().is_obsolete(initial.id()));
    assert!(!tx2.as_repo_ref().evolution().is_obsolete(initial.id()));

    let rewrite1 = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewrite1".to_string())
        .write_to_transaction(&mut tx1);
    let rewrite2 = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewrite2".to_string())
        .write_to_transaction(&mut tx2);

    // Neither transaction has committed yet, so each transaction sees its own
    // commit.
    assert_heads(repo.as_repo_ref(), vec![&wc_id, initial.id()]);
    assert_heads(tx1.as_repo_ref(), vec![&wc_id, initial.id(), rewrite1.id()]);
    assert_heads(tx2.as_repo_ref(), vec![&wc_id, initial.id(), rewrite2.id()]);
    assert!(!repo.evolution().is_obsolete(initial.id()));
    assert!(tx1.as_repo_ref().evolution().is_obsolete(initial.id()));
    assert!(tx2.as_repo_ref().evolution().is_obsolete(initial.id()));

    // The base repo and tx2 don't see the commits from tx1.
    tx1.commit();
    assert_heads(repo.as_repo_ref(), vec![&wc_id, initial.id()]);
    assert_heads(tx2.as_repo_ref(), vec![&wc_id, initial.id(), rewrite2.id()]);

    // The base repo still doesn't see the commits after both transactions commit.
    tx2.commit();
    assert_heads(repo.as_repo_ref(), vec![&wc_id, initial.id()]);
    // After reload, the base repo sees both rewrites.
    Arc::get_mut(&mut repo).unwrap().reload();
    assert_heads(
        repo.as_repo_ref(),
        vec![&wc_id, initial.id(), rewrite1.id(), rewrite2.id()],
    );
}
