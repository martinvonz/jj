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

use std::path::Path;

use jujutsu_lib::backend::CommitId;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::testutils;
use test_case::test_case;

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
        .collect()
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_unpublished_operation(use_git: bool) {
    // Test that the operation doesn't get published until that's requested.
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_repo(&settings, use_git);
    let repo = &test_workspace.repo;

    let op_heads_dir = repo.repo_path().join("op_heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction("transaction 1");
    testutils::create_random_commit(&settings, repo).write_to_repo(tx1.mut_repo());
    let unpublished_op = tx1.write();
    let op_id1 = unpublished_op.operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id0.hex()]);
    unpublished_op.publish();
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_consecutive_operations(use_git: bool) {
    // Test that consecutive operations result in a single op-head on disk after
    // each operation
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_repo(&settings, use_git);
    let repo = &test_workspace.repo;

    let op_heads_dir = repo.repo_path().join("op_heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction("transaction 1");
    testutils::create_random_commit(&settings, repo).write_to_repo(tx1.mut_repo());
    let op_id1 = tx1.commit().operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);

    let repo = repo.reload();
    let mut tx2 = repo.start_transaction("transaction 2");
    testutils::create_random_commit(&settings, &repo).write_to_repo(tx2.mut_repo());
    let op_id2 = tx2.commit().operation().id().clone();
    assert_ne!(op_id2, op_id0);
    assert_ne!(op_id2, op_id1);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id2.hex()]);

    // Reloading the repo makes no difference (there are no conflicting operations
    // to resolve).
    let _repo = repo.reload();
    assert_eq!(list_dir(&op_heads_dir), vec![op_id2.hex()]);
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_concurrent_operations(use_git: bool) {
    // Test that consecutive operations result in multiple op-heads on disk until
    // the repo has been reloaded (which currently happens right away).
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_repo(&settings, use_git);
    let repo = &test_workspace.repo;

    let op_heads_dir = repo.repo_path().join("op_heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction("transaction 1");
    testutils::create_random_commit(&settings, repo).write_to_repo(tx1.mut_repo());
    let op_id1 = tx1.commit().operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);

    // After both transactions have committed, we should have two op-heads on disk,
    // since they were run in parallel.
    let mut tx2 = repo.start_transaction("transaction 2");
    testutils::create_random_commit(&settings, repo).write_to_repo(tx2.mut_repo());
    let op_id2 = tx2.commit().operation().id().clone();
    assert_ne!(op_id2, op_id0);
    assert_ne!(op_id2, op_id1);
    let mut actual_heads_on_disk = list_dir(&op_heads_dir);
    actual_heads_on_disk.sort();
    let mut expected_heads_on_disk = vec![op_id1.hex(), op_id2.hex()];
    expected_heads_on_disk.sort();
    assert_eq!(actual_heads_on_disk, expected_heads_on_disk);

    // Reloading the repo causes the operations to be merged
    let repo = repo.reload();
    let merged_op_id = repo.op_id().clone();
    assert_ne!(merged_op_id, op_id0);
    assert_ne!(merged_op_id, op_id1);
    assert_ne!(merged_op_id, op_id2);
    assert_eq!(list_dir(&op_heads_dir), vec![merged_op_id.hex()]);
}

fn assert_heads(repo: RepoRef, expected: Vec<&CommitId>) {
    let expected = expected.iter().cloned().cloned().collect();
    assert_eq!(*repo.view().heads(), expected);
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_isolation(use_git: bool) {
    // Test that two concurrent transactions don't see each other's changes.
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_repo(&settings, use_git);
    let repo = &test_workspace.repo;

    let checkout_id = repo.view().checkout().clone();
    let mut tx = repo.start_transaction("test");
    let initial = testutils::create_random_commit(&settings, repo)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let mut tx1 = repo.start_transaction("transaction 1");
    let mut_repo1 = tx1.mut_repo();
    let mut tx2 = repo.start_transaction("transaction 2");
    let mut_repo2 = tx2.mut_repo();

    assert_heads(repo.as_repo_ref(), vec![&checkout_id, initial.id()]);
    assert_heads(mut_repo1.as_repo_ref(), vec![&checkout_id, initial.id()]);
    assert_heads(mut_repo2.as_repo_ref(), vec![&checkout_id, initial.id()]);

    let rewrite1 = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewrite1".to_string())
        .write_to_repo(mut_repo1);
    let rewrite2 = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewrite2".to_string())
        .write_to_repo(mut_repo2);

    // Neither transaction has committed yet, so each transaction sees its own
    // commit.
    assert_heads(repo.as_repo_ref(), vec![&checkout_id, initial.id()]);
    assert_heads(
        mut_repo1.as_repo_ref(),
        vec![&checkout_id, initial.id(), rewrite1.id()],
    );
    assert_heads(
        mut_repo2.as_repo_ref(),
        vec![&checkout_id, initial.id(), rewrite2.id()],
    );

    // The base repo and tx2 don't see the commits from tx1.
    tx1.commit();
    assert_heads(repo.as_repo_ref(), vec![&checkout_id, initial.id()]);
    assert_heads(
        mut_repo2.as_repo_ref(),
        vec![&checkout_id, initial.id(), rewrite2.id()],
    );

    // The base repo still doesn't see the commits after both transactions commit.
    tx2.commit();
    assert_heads(repo.as_repo_ref(), vec![&checkout_id, initial.id()]);
    // After reload, the base repo sees both rewrites.
    let repo = repo.reload();
    assert_heads(
        repo.as_repo_ref(),
        vec![&checkout_id, initial.id(), rewrite1.id(), rewrite2.id()],
    );
}
