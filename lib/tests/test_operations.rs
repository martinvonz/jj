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
use std::slice;
use std::sync::Arc;
use std::time::SystemTime;

use assert_matches::assert_matches;
use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::object_id::ObjectId;
use jj_lib::op_walk::{self, OpsetEvaluationError, OpsetResolutionError};
use jj_lib::operation::Operation;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::settings::UserSettings;
use testutils::{create_random_commit, write_random_commit, TestRepo};

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
        .sorted()
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

#[test]
fn test_reparent_range_linear() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo_0 = test_repo.repo;
    let op_store = repo_0.op_store();

    let read_op = |id| {
        let data = op_store.read_operation(id).unwrap();
        Operation::new(op_store.clone(), id.clone(), data)
    };

    fn op_parents<const N: usize>(op: &Operation) -> [Operation; N] {
        let parents: Vec<_> = op.parents().try_collect().unwrap();
        parents.try_into().unwrap()
    }

    // Set up linear operation graph:
    // D
    // C
    // B
    // A
    // 0 (initial)
    let random_tx = |repo: &Arc<ReadonlyRepo>| {
        let mut tx = repo.start_transaction(&settings);
        write_random_commit(tx.mut_repo(), &settings);
        tx
    };
    let repo_a = random_tx(&repo_0).commit("op A");
    let repo_b = random_tx(&repo_a).commit("op B");
    let repo_c = random_tx(&repo_b).commit("op C");
    let repo_d = random_tx(&repo_c).commit("op D");

    // Reparent B..D (=C|D) onto A:
    // D'
    // C'
    // A
    // 0 (initial)
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        slice::from_ref(repo_b.operation()),
        slice::from_ref(repo_d.operation()),
        repo_a.operation(),
    )
    .unwrap();
    assert_eq!(stats.new_head_ids.len(), 1);
    assert_eq!(stats.rewritten_count, 2);
    assert_eq!(stats.unreachable_count, 1);
    let new_op_d = read_op(&stats.new_head_ids[0]);
    assert_eq!(new_op_d.metadata(), repo_d.operation().metadata());
    assert_eq!(new_op_d.view_id(), repo_d.operation().view_id());
    let [new_op_c] = op_parents(&new_op_d);
    assert_eq!(new_op_c.metadata(), repo_c.operation().metadata());
    assert_eq!(new_op_c.view_id(), repo_c.operation().view_id());
    assert_eq!(new_op_c.parent_ids(), slice::from_ref(repo_a.op_id()));

    // Reparent empty range onto A
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        slice::from_ref(repo_d.operation()),
        slice::from_ref(repo_d.operation()),
        repo_a.operation(),
    )
    .unwrap();
    assert_eq!(stats.new_head_ids, vec![repo_a.op_id().clone()]);
    assert_eq!(stats.rewritten_count, 0);
    assert_eq!(stats.unreachable_count, 3);
}

#[test]
fn test_reparent_range_branchy() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo_0 = test_repo.repo;
    let op_store = repo_0.op_store();

    let read_op = |id| {
        let data = op_store.read_operation(id).unwrap();
        Operation::new(op_store.clone(), id.clone(), data)
    };

    fn op_parents<const N: usize>(op: &Operation) -> [Operation; N] {
        let parents: Vec<_> = op.parents().try_collect().unwrap();
        parents.try_into().unwrap()
    }

    // Set up branchy operation graph:
    // G
    // |\
    // | F
    // E |
    // D |
    // |/
    // C
    // B
    // A
    // 0 (initial)
    let random_tx = |repo: &Arc<ReadonlyRepo>| {
        let mut tx = repo.start_transaction(&settings);
        write_random_commit(tx.mut_repo(), &settings);
        tx
    };
    let repo_a = random_tx(&repo_0).commit("op A");
    let repo_b = random_tx(&repo_a).commit("op B");
    let repo_c = random_tx(&repo_b).commit("op C");
    let repo_d = random_tx(&repo_c).commit("op D");
    let tx_e = random_tx(&repo_d);
    let tx_f = random_tx(&repo_c);
    let repo_g = testutils::commit_transactions(&settings, vec![tx_e, tx_f]);
    let [op_e, op_f] = op_parents(repo_g.operation());

    // Reparent D..G (= E|F|G) onto B:
    // G'
    // |\
    // | F'
    // E'|
    // |/
    // B
    // A
    // 0 (initial)
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        slice::from_ref(repo_d.operation()),
        slice::from_ref(repo_g.operation()),
        repo_b.operation(),
    )
    .unwrap();
    assert_eq!(stats.new_head_ids.len(), 1);
    assert_eq!(stats.rewritten_count, 3);
    assert_eq!(stats.unreachable_count, 2);
    let new_op_g = read_op(&stats.new_head_ids[0]);
    assert_eq!(new_op_g.metadata(), repo_g.operation().metadata());
    assert_eq!(new_op_g.view_id(), repo_g.operation().view_id());
    let [new_op_e, new_op_f] = op_parents(&new_op_g);
    assert_eq!(new_op_e.parent_ids(), slice::from_ref(repo_b.op_id()));
    assert_eq!(new_op_f.parent_ids(), slice::from_ref(repo_b.op_id()));

    // Reparent B..G (=C|D|E|F|G) onto A:
    // G'
    // |\
    // | F'
    // E'|
    // D'|
    // |/
    // C'
    // A
    // 0 (initial)
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        slice::from_ref(repo_b.operation()),
        slice::from_ref(repo_g.operation()),
        repo_a.operation(),
    )
    .unwrap();
    assert_eq!(stats.new_head_ids.len(), 1);
    assert_eq!(stats.rewritten_count, 5);
    assert_eq!(stats.unreachable_count, 1);
    let new_op_g = read_op(&stats.new_head_ids[0]);
    assert_eq!(new_op_g.metadata(), repo_g.operation().metadata());
    assert_eq!(new_op_g.view_id(), repo_g.operation().view_id());
    let [new_op_e, new_op_f] = op_parents(&new_op_g);
    let [new_op_d] = op_parents(&new_op_e);
    assert_eq!(new_op_d.parent_ids(), new_op_f.parent_ids());
    let [new_op_c] = op_parents(&new_op_d);
    assert_eq!(new_op_c.parent_ids(), slice::from_ref(repo_a.op_id()));

    // Reparent (E|F)..G (=G) onto D:
    // G'
    // D
    // C
    // B
    // A
    // 0 (initial)
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        &[op_e.clone(), op_f.clone()],
        slice::from_ref(repo_g.operation()),
        repo_d.operation(),
    )
    .unwrap();
    assert_eq!(stats.new_head_ids.len(), 1);
    assert_eq!(stats.rewritten_count, 1);
    assert_eq!(stats.unreachable_count, 2);
    let new_op_g = read_op(&stats.new_head_ids[0]);
    assert_eq!(new_op_g.metadata(), repo_g.operation().metadata());
    assert_eq!(new_op_g.view_id(), repo_g.operation().view_id());
    assert_eq!(new_op_g.parent_ids(), slice::from_ref(repo_d.op_id()));

    // Reparent C..F (=F) onto D (ignoring G):
    // F'
    // D
    // C
    // B
    // A
    // 0 (initial)
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        slice::from_ref(repo_c.operation()),
        slice::from_ref(&op_f),
        repo_d.operation(),
    )
    .unwrap();
    assert_eq!(stats.new_head_ids.len(), 1);
    assert_eq!(stats.rewritten_count, 1);
    assert_eq!(stats.unreachable_count, 0);
    let new_op_f = read_op(&stats.new_head_ids[0]);
    assert_eq!(new_op_f.metadata(), op_f.metadata());
    assert_eq!(new_op_f.view_id(), op_f.view_id());
    assert_eq!(new_op_f.parent_ids(), slice::from_ref(repo_d.op_id()));
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
    let repo = test_repo.repo;
    let op_store = repo.op_store();

    let mut operations = Vec::new();
    // The actual value of `i` doesn't matter, we just need to make sure we end
    // up with hashes with ambiguous prefixes.
    for i in (1..7).chain([16]) {
        let tx = repo.start_transaction(&settings);
        let repo = tx.commit(format!("transaction {i}"));
        operations.push(repo.operation().clone());
    }
    // "2" and "0" are ambiguous
    insta::assert_debug_snapshot!(operations.iter().map(|op| op.id().hex()).collect_vec(), @r###"
    [
        "5aebb24d08d6f5282d9e06bded4b51febbb4ff4bc822cdd4db9043961339955d6af912e783c3864245867d0cf6d609cb004c5ff0cef5e914c15a415ba92e38a2",
        "ad1cda629b220f2651d972475fba75e47c30cd57862c51a727e57b7b6e2fbaa937d0bcb881cf6fcff30ba1a088a4ad588cf880765b9d9680b551cdc446f3489e",
        "2feaf9eb61232582d35f1cb0203b69425f7a7d07140a5f8ff0d0bf98dd9f433b941e4c9e0882f65a007266affee275e235ae1614b159ceadbf9fe6879077a5da",
        "0b183be5767c3ff9945c8a9e3ac7639d249ff9873985a551f4c2070782aea8b5018ebff906036d1658038708ab9de0d867be385b1181aa7a81669e2ef6852355",
        "d1c3031dff7b1db4db1bfb592a9aa0ea6faae9300033ef20cb6da488e4b90524c22af8e2541cdb99ca0ee2be3299c213c0f8d48390cd5f11462c4f80c5790f68",
        "2369ba98e2596715606978a72608970287aaa064ec890f87c29b7e7df64fdf02b7f24b8b03ee845e132f3e19e3929de359b6cfe1328b42946c45ac5ff80705ca",
        "00271842a189d274a2c97ac28f937584a47b84463d5b408c6f998089131e0e1329a287667b2ac5d63f8c576e95323bcf992c99caa4ef4612a1c3798fe8a3f74f",
    ]
    "###);

    let repo_loader = repo.loader();
    let resolve = |op_str: &str| op_walk::resolve_op_for_load(&repo_loader, op_str);

    // Full id
    assert_eq!(resolve(&operations[0].id().hex()).unwrap(), operations[0]);
    // Short id, odd length
    assert_eq!(
        resolve(&operations[0].id().hex()[..3]).unwrap(),
        operations[0]
    );
    // Short id, even length
    assert_eq!(
        resolve(&operations[1].id().hex()[..2]).unwrap(),
        operations[1]
    );
    // Ambiguous id
    assert_matches!(
        resolve("2"),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::AmbiguousIdPrefix(_)
        ))
    );
    // Empty id
    assert_matches!(
        resolve(""),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::InvalidIdPrefix(_)
        ))
    );
    // Unknown id
    assert_matches!(
        resolve("deadbee"),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::NoSuchOperation(_)
        ))
    );
    // Virtual root id
    let root_operation = {
        let id = op_store.root_operation_id();
        let data = op_store.read_operation(id).unwrap();
        Operation::new(op_store.clone(), id.clone(), data)
    };
    assert_eq!(resolve(&root_operation.id().hex()).unwrap(), root_operation);
    assert_eq!(resolve("000").unwrap(), root_operation);
    assert_eq!(resolve("002").unwrap(), operations[6]);
    assert_matches!(
        resolve("0"),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::AmbiguousIdPrefix(_)
        ))
    );
}

#[test]
fn test_resolve_current_op() {
    let settings = stable_op_id_settings();
    let test_repo = TestRepo::init_with_settings(&settings);
    let repo = test_repo.repo;

    assert_eq!(
        op_walk::resolve_op_with_repo(&repo, "@").unwrap(),
        *repo.operation()
    );
}

#[test]
fn test_resolve_op_parents_children() {
    // Use monotonic timestamp to stabilize merge order of transactions
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_settings(&settings);
    let mut repo = &test_repo.repo;

    let mut repos = Vec::new();
    for _ in 0..3 {
        let tx = repo.start_transaction(&settings);
        repos.push(tx.commit("test"));
        repo = repos.last().unwrap();
    }
    let operations = repos.iter().map(|repo| repo.operation()).collect_vec();

    // Parent
    let op2_id_hex = operations[2].id().hex();
    assert_eq!(
        op_walk::resolve_op_with_repo(repo, &format!("{op2_id_hex}-")).unwrap(),
        *operations[1]
    );
    assert_eq!(
        op_walk::resolve_op_with_repo(repo, &format!("{op2_id_hex}--")).unwrap(),
        *operations[0]
    );
    // "{op2_id_hex}----" is the root operation
    assert_matches!(
        op_walk::resolve_op_with_repo(repo, &format!("{op2_id_hex}-----")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::EmptyOperations(_)
        ))
    );

    // Child
    let op0_id_hex = operations[0].id().hex();
    assert_eq!(
        op_walk::resolve_op_with_repo(repo, &format!("{op0_id_hex}+")).unwrap(),
        *operations[1]
    );
    assert_eq!(
        op_walk::resolve_op_with_repo(repo, &format!("{op0_id_hex}++")).unwrap(),
        *operations[2]
    );
    assert_matches!(
        op_walk::resolve_op_with_repo(repo, &format!("{op0_id_hex}+++")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::EmptyOperations(_)
        ))
    );

    // Child of parent
    assert_eq!(
        op_walk::resolve_op_with_repo(repo, &format!("{op2_id_hex}--+")).unwrap(),
        *operations[1]
    );

    // Child at old repo: new operations shouldn't be visible
    assert_eq!(
        op_walk::resolve_op_with_repo(&repos[1], &format!("{op0_id_hex}+")).unwrap(),
        *operations[1]
    );
    assert_matches!(
        op_walk::resolve_op_with_repo(&repos[0], &format!("{op0_id_hex}+")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::EmptyOperations(_)
        ))
    );

    // Merge and fork
    let tx1 = repo.start_transaction(&settings);
    let tx2 = repo.start_transaction(&settings);
    let repo = testutils::commit_transactions(&settings, vec![tx1, tx2]);
    let op5_id_hex = repo.operation().id().hex();
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op5_id_hex}-")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::MultipleOperations(_)
        ))
    );
    let op2_id_hex = operations[2].id().hex();
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op2_id_hex}+")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::MultipleOperations(_)
        ))
    );
}

#[test]
fn test_gc() {
    let settings = stable_op_id_settings();
    let test_repo = TestRepo::init();
    let repo_0 = test_repo.repo;
    let op_store = repo_0.op_store();
    let op_dir = repo_0.repo_path().join("op_store").join("operations");
    let view_dir = repo_0.repo_path().join("op_store").join("views");

    // Set up operation graph:
    //
    //   F
    //   E (empty)
    // D |
    // C |
    // |/
    // B
    // A
    // 0 (initial)
    let empty_tx = |repo: &Arc<ReadonlyRepo>| repo.start_transaction(&settings);
    let random_tx = |repo: &Arc<ReadonlyRepo>| {
        let mut tx = repo.start_transaction(&settings);
        write_random_commit(tx.mut_repo(), &settings);
        tx
    };
    let repo_a = random_tx(&repo_0).commit("op A");
    let repo_b = random_tx(&repo_a).commit("op B");
    let repo_c = random_tx(&repo_b).commit("op C");
    let repo_d = random_tx(&repo_c).commit("op D");
    let repo_e = empty_tx(&repo_b).commit("op E");
    let repo_f = random_tx(&repo_e).commit("op F");

    // Sanity check for the original state
    let mut expected_op_entries = list_dir(&op_dir);
    let mut expected_view_entries = list_dir(&view_dir);
    assert_eq!(expected_op_entries.len(), 7);
    assert_eq!(expected_view_entries.len(), 6);

    // No heads, but all kept by file modification time
    op_store.gc(&[], SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!(list_dir(&op_dir), expected_op_entries);
    assert_eq!(list_dir(&view_dir), expected_view_entries);

    // All reachable from heads
    let now = SystemTime::now();
    let head_ids = [repo_d.op_id().clone(), repo_f.op_id().clone()];
    op_store.gc(&head_ids, now).unwrap();
    assert_eq!(list_dir(&op_dir), expected_op_entries);
    assert_eq!(list_dir(&view_dir), expected_view_entries);

    // E|F are no longer reachable, but E's view is still reachable
    op_store.gc(slice::from_ref(repo_d.op_id()), now).unwrap();
    expected_op_entries
        .retain(|name| *name != repo_e.op_id().hex() && *name != repo_f.op_id().hex());
    expected_view_entries.retain(|name| *name != repo_f.operation().view_id().hex());
    assert_eq!(list_dir(&op_dir), expected_op_entries);
    assert_eq!(list_dir(&view_dir), expected_view_entries);

    // B|C|D are no longer reachable
    op_store.gc(slice::from_ref(repo_a.op_id()), now).unwrap();
    expected_op_entries.retain(|name| {
        *name != repo_b.op_id().hex()
            && *name != repo_c.op_id().hex()
            && *name != repo_d.op_id().hex()
    });
    expected_view_entries.retain(|name| {
        *name != repo_b.operation().view_id().hex()
            && *name != repo_c.operation().view_id().hex()
            && *name != repo_d.operation().view_id().hex()
    });
    assert_eq!(list_dir(&op_dir), expected_op_entries);
    assert_eq!(list_dir(&view_dir), expected_view_entries);

    // Sanity check for the last state
    assert_eq!(expected_op_entries.len(), 2);
    assert_eq!(expected_view_entries.len(), 2);
}
