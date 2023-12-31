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

use std::path::Path;

use assert_matches::assert_matches;
use itertools::Itertools as _;
use jj_lib::backend::{CommitId, ObjectId};
use jj_lib::op_walk::{self, OpsetEvaluationError, OpsetResolutionError};
use jj_lib::repo::Repo;
use jj_lib::settings::UserSettings;
use testutils::{create_random_commit, write_random_commit, TestRepo};

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn test_unpublished_operation() {
    // Test that the operation doesn't get published until that's requested.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let op_heads_dir = repo.repo_path().join("op_heads").join("heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction(&settings);
    write_random_commit(tx1.mut_repo(), &settings);
    let unpublished_op = tx1.write("transaction 1");
    let op_id1 = unpublished_op.operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id0.hex()]);
    unpublished_op.publish();
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);
}

#[test]
fn test_consecutive_operations() {
    // Test that consecutive operations result in a single op-head on disk after
    // each operation
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let op_heads_dir = repo.repo_path().join("op_heads").join("heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction(&settings);
    write_random_commit(tx1.mut_repo(), &settings);
    let op_id1 = tx1.commit("transaction 1").operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);

    let repo = repo.reload_at_head(&settings).unwrap();
    let mut tx2 = repo.start_transaction(&settings);
    write_random_commit(tx2.mut_repo(), &settings);
    let op_id2 = tx2.commit("transaction 2").operation().id().clone();
    assert_ne!(op_id2, op_id0);
    assert_ne!(op_id2, op_id1);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id2.hex()]);

    // Reloading the repo makes no difference (there are no conflicting operations
    // to resolve).
    let _repo = repo.reload_at_head(&settings).unwrap();
    assert_eq!(list_dir(&op_heads_dir), vec![op_id2.hex()]);
}

#[test]
fn test_concurrent_operations() {
    // Test that consecutive operations result in multiple op-heads on disk until
    // the repo has been reloaded (which currently happens right away).
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let op_heads_dir = repo.repo_path().join("op_heads").join("heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction(&settings);
    write_random_commit(tx1.mut_repo(), &settings);
    let op_id1 = tx1.commit("transaction 1").operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);

    // After both transactions have committed, we should have two op-heads on disk,
    // since they were run in parallel.
    let mut tx2 = repo.start_transaction(&settings);
    write_random_commit(tx2.mut_repo(), &settings);
    let op_id2 = tx2.commit("transaction 2").operation().id().clone();
    assert_ne!(op_id2, op_id0);
    assert_ne!(op_id2, op_id1);
    let mut actual_heads_on_disk = list_dir(&op_heads_dir);
    actual_heads_on_disk.sort();
    let mut expected_heads_on_disk = vec![op_id1.hex(), op_id2.hex()];
    expected_heads_on_disk.sort();
    assert_eq!(actual_heads_on_disk, expected_heads_on_disk);

    // Reloading the repo causes the operations to be merged
    let repo = repo.reload_at_head(&settings).unwrap();
    let merged_op_id = repo.op_id().clone();
    assert_ne!(merged_op_id, op_id0);
    assert_ne!(merged_op_id, op_id1);
    assert_ne!(merged_op_id, op_id2);
    assert_eq!(list_dir(&op_heads_dir), vec![merged_op_id.hex()]);
}

fn assert_heads(repo: &dyn Repo, expected: Vec<&CommitId>) {
    let expected = expected.iter().cloned().cloned().collect();
    assert_eq!(*repo.view().heads(), expected);
}

#[test]
fn test_isolation() {
    // Test that two concurrent transactions don't see each other's changes.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let initial = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write()
        .unwrap();
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let mut_repo1 = tx1.mut_repo();
    let mut tx2 = repo.start_transaction(&settings);
    let mut_repo2 = tx2.mut_repo();

    assert_heads(repo.as_ref(), vec![initial.id()]);
    assert_heads(mut_repo1, vec![initial.id()]);
    assert_heads(mut_repo2, vec![initial.id()]);

    let rewrite1 = mut_repo1
        .rewrite_commit(&settings, &initial)
        .set_description("rewrite1")
        .write()
        .unwrap();
    mut_repo1.rebase_descendants(&settings).unwrap();
    let rewrite2 = mut_repo2
        .rewrite_commit(&settings, &initial)
        .set_description("rewrite2")
        .write()
        .unwrap();
    mut_repo2.rebase_descendants(&settings).unwrap();

    // Neither transaction has committed yet, so each transaction sees its own
    // commit.
    assert_heads(repo.as_ref(), vec![initial.id()]);
    assert_heads(mut_repo1, vec![rewrite1.id()]);
    assert_heads(mut_repo2, vec![rewrite2.id()]);

    // The base repo and tx2 don't see the commits from tx1.
    tx1.commit("transaction 1");
    assert_heads(repo.as_ref(), vec![initial.id()]);
    assert_heads(mut_repo2, vec![rewrite2.id()]);

    // The base repo still doesn't see the commits after both transactions commit.
    tx2.commit("transaction 2");
    assert_heads(repo.as_ref(), vec![initial.id()]);
    // After reload, the base repo sees both rewrites.
    let repo = repo.reload_at_head(&settings).unwrap();
    assert_heads(repo.as_ref(), vec![rewrite1.id(), rewrite2.id()]);
}

fn stable_op_id_settings() -> UserSettings {
    UserSettings::from_config(
        testutils::base_config()
            .add_source(config::File::from_str(
                "debug.operation-timestamp = '2001-02-03T04:05:06+07:00'",
                config::FileFormat::Toml,
            ))
            .build()
            .unwrap(),
    )
}

#[test]
fn test_resolve_op_id() {
    let settings = stable_op_id_settings();
    let test_repo = TestRepo::init_with_settings(&settings);
    let mut repo = test_repo.repo;

    let mut operations = Vec::new();
    for i in 0..6 {
        let tx = repo.start_transaction(&settings);
        repo = tx.commit(format!("transaction {i}"));
        operations.push(repo.operation().clone());
    }
    // "2" is ambiguous
    insta::assert_debug_snapshot!(operations.iter().map(|op| op.id().hex()).collect_vec(), @r###"
    [
        "27f8c802c8378c5c85825365e83928936ae84d7ae3b5bd26d1cd046aa9a2f791dd7b272338d0d2da8a4359523f25daf217f99128a155ba4bc728d279fc3d8f7f",
        "8a6b19ed474dfad1efa49d64d265f80f74c1d10bf77439900d92d8e6a29fdb64ad1137a92928bfd409096bf84b6fbfb50ebdcc6a28323f9f8e5893548f21b7fb",
        "65198b538e0f6558d875c49712b0b3570e3a0eb697fd22f5817e39139937b4498e9e9080df1353e116880e36c683f5dddc39d048007ef50da83690a94502bc68",
        "59da2544953d8d5851e8f64ed5949c8c26f676b87ab84e9fe153bca76912de3753dee8c9cb641f53f57c51a0e876cd43f08c28ca651ad312e5bc09354e9ec40f",
        "f40d12f62b921bdf96c2d191a4d04845fa26043d131ea1e69eb06fa7a4bbfed6668ab48bed7ec728f7e2c9e675d394b382a332c68399d7f4c446450610479ecf",
        "2b45a4f90854dd3d4833d998f4fa2e4d4c4eda5212edd3845e8ccb3618d9d538d7a98c173791995898e68d272697ffed1b69838cf839d96cb770856cf499eea8",
    ]
    "###);

    // Full id
    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, &operations[0].id().hex()).unwrap(),
        operations[0]
    );
    // Short id, odd length
    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, &operations[0].id().hex()[..3]).unwrap(),
        operations[0]
    );
    // Short id, even length
    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, &operations[1].id().hex()[..2]).unwrap(),
        operations[1]
    );
    // Ambiguous id
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, "2"),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::AmbiguousIdPrefix(_)
        ))
    );
    // Empty id
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, ""),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::InvalidIdPrefix(_)
        ))
    );
    // Unknown id
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, "deadbee"),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::NoSuchOperation(_)
        ))
    );
    // Current op
    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, "@").unwrap(),
        *repo.operation()
    );
}

#[test]
fn test_resolve_op_parents() {
    // Use monotonic timestamp to stabilize merge order of transactions
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_settings(&settings);
    let mut repo = test_repo.repo;

    let mut operations = Vec::new();
    for _ in 0..3 {
        let tx = repo.start_transaction(&settings);
        repo = tx.commit("test");
        operations.push(repo.operation().clone());
    }

    let op2_id_hex = operations[2].id().hex();
    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op2_id_hex}-")).unwrap(),
        operations[1]
    );
    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op2_id_hex}--")).unwrap(),
        operations[0]
    );
    // "{op2_id_hex}---" is the operation to initialize the repo.
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op2_id_hex}----")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::EmptyOperations(_)
        ))
    );

    let tx1 = repo.start_transaction(&settings);
    let tx2 = repo.start_transaction(&settings);
    repo = testutils::commit_transactions(&settings, vec![tx1, tx2]);
    let op5_id_hex = repo.operation().id().hex();
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op5_id_hex}-")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::MultipleOperations(_)
        ))
    );
}
