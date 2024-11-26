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
use jj_lib::config::ConfigLayer;
use jj_lib::config::ConfigSource;
use jj_lib::object_id::ObjectId;
use jj_lib::op_walk;
use jj_lib::op_walk::OpsetEvaluationError;
use jj_lib::op_walk::OpsetResolutionError;
use jj_lib::operation::Operation;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::settings::UserSettings;
use testutils::create_random_commit;
use testutils::write_random_commit;
use testutils::TestRepo;

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

    let op_heads_dir = test_repo.repo_path().join("op_heads").join("heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction(&settings);
    write_random_commit(tx1.repo_mut(), &settings);
    let unpublished_op = tx1.write("transaction 1");
    let op_id1 = unpublished_op.operation().id().clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id0.hex()]);
    unpublished_op.publish().unwrap();
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);
}

#[test]
fn test_consecutive_operations() {
    // Test that consecutive operations result in a single op-head on disk after
    // each operation
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let op_heads_dir = test_repo.repo_path().join("op_heads").join("heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction(&settings);
    write_random_commit(tx1.repo_mut(), &settings);
    let op_id1 = tx1
        .commit("transaction 1")
        .unwrap()
        .operation()
        .id()
        .clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);

    let repo = repo.reload_at_head(&settings).unwrap();
    let mut tx2 = repo.start_transaction(&settings);
    write_random_commit(tx2.repo_mut(), &settings);
    let op_id2 = tx2
        .commit("transaction 2")
        .unwrap()
        .operation()
        .id()
        .clone();
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

    let op_heads_dir = test_repo.repo_path().join("op_heads").join("heads");
    let op_id0 = repo.op_id().clone();
    assert_eq!(list_dir(&op_heads_dir), vec![repo.op_id().hex()]);

    let mut tx1 = repo.start_transaction(&settings);
    write_random_commit(tx1.repo_mut(), &settings);
    let op_id1 = tx1
        .commit("transaction 1")
        .unwrap()
        .operation()
        .id()
        .clone();
    assert_ne!(op_id1, op_id0);
    assert_eq!(list_dir(&op_heads_dir), vec![op_id1.hex()]);

    // After both transactions have committed, we should have two op-heads on disk,
    // since they were run in parallel.
    let mut tx2 = repo.start_transaction(&settings);
    write_random_commit(tx2.repo_mut(), &settings);
    let op_id2 = tx2
        .commit("transaction 2")
        .unwrap()
        .operation()
        .id()
        .clone();
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
    let initial = create_random_commit(tx.repo_mut(), &settings)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write()
        .unwrap();
    let repo = tx.commit("test").unwrap();

    let mut tx1 = repo.start_transaction(&settings);
    let mut_repo1 = tx1.repo_mut();
    let mut tx2 = repo.start_transaction(&settings);
    let mut_repo2 = tx2.repo_mut();

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
    tx1.commit("transaction 1").unwrap();
    assert_heads(repo.as_ref(), vec![initial.id()]);
    assert_heads(mut_repo2, vec![rewrite2.id()]);

    // The base repo still doesn't see the commits after both transactions commit.
    tx2.commit("transaction 2").unwrap();
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
    let loader = repo_0.loader();
    let op_store = repo_0.op_store();

    let read_op = |id| loader.load_operation(id).unwrap();

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
        write_random_commit(tx.repo_mut(), &settings);
        tx
    };
    let repo_a = random_tx(&repo_0).commit("op A").unwrap();
    let repo_b = random_tx(&repo_a).commit("op B").unwrap();
    let repo_c = random_tx(&repo_b).commit("op C").unwrap();
    let repo_d = random_tx(&repo_c).commit("op D").unwrap();

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
fn test_reparent_range_bookmarky() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo_0 = test_repo.repo;
    let loader = repo_0.loader();
    let op_store = repo_0.op_store();

    let read_op = |id| loader.load_operation(id).unwrap();

    fn op_parents<const N: usize>(op: &Operation) -> [Operation; N] {
        let parents: Vec<_> = op.parents().try_collect().unwrap();
        parents.try_into().unwrap()
    }

    // Set up bookmarky operation graph:
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
        write_random_commit(tx.repo_mut(), &settings);
        tx
    };
    let repo_a = random_tx(&repo_0).commit("op A").unwrap();
    let repo_b = random_tx(&repo_a).commit("op B").unwrap();
    let repo_c = random_tx(&repo_b).commit("op C").unwrap();
    let repo_d = random_tx(&repo_c).commit("op D").unwrap();
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
    let mut config = testutils::base_user_config();
    config.add_layer(
        ConfigLayer::parse(
            ConfigSource::User,
            "debug.operation-timestamp = '2001-02-03T04:05:06+07:00'",
        )
        .unwrap(),
    );
    UserSettings::from_config(config)
}

#[test]
fn test_resolve_op_id() {
    let settings = stable_op_id_settings();
    let test_repo = TestRepo::init_with_settings(&settings);
    let repo = test_repo.repo;
    let loader = repo.loader();

    let mut operations = Vec::new();
    // The actual value of `i` doesn't matter, we just need to make sure we end
    // up with hashes with ambiguous prefixes.
    for i in (1..5).chain([39, 62]) {
        let tx = repo.start_transaction(&settings);
        let repo = tx.commit(format!("transaction {i}")).unwrap();
        operations.push(repo.operation().clone());
    }
    // "b" and "0" are ambiguous
    insta::assert_debug_snapshot!(operations.iter().map(|op| op.id().hex()).collect_vec(), @r#"
    [
        "bb1ea76bb194556214b1259568d5f3381fb4209f10b86d6c3c7d162a9b8ee1a5d98da57cf21ceadeecd2416c20508348ed4c1a24226c708f035b138fc7a97d5b",
        "5c35c6506eedd9c74ffab46940129cb3b66e5e1968b4eea5bb38701d6d3462b4a34d78efcaa81d41fabf6937d79c4431e2adc4361095c9fb795004da420d8a26",
        "b43387cf7a5808ebb6cdacd5c95de9d4b315c6edc465a49ff290b731da1c3d57315af49686e5ffd4c2fc4478af40b4a70cba7334bbca8e3d4e69176de807a916",
        "fcd828a3033f9a9f44c8f06cd0d7f79570d53895c9d7d794ea51a7ee4b7871c8fe245ec18d2ece76ec7b51a998b04da811c232668c7c2c53f72b5baf0ad20797",
        "091574d16d89ab848ac08c9a8e35276484c5e332ea97f1fad7b794763aa280ce5b663d835b555b5b763cbdbb6d8dba5a35ad1f2780ebdca5e598f07f82dcd3c7",
        "06e9f38473578a4b1a8672ab474eb2741269fffb2f765a610de47fddafc60a88c002f7cdb9d82a9d1dfdbdd3b4045cd62e34215e7a781ed149332980e90227f1",
    ]
    "#);

    let repo_loader = repo.loader();
    let resolve = |op_str: &str| op_walk::resolve_op_for_load(repo_loader, op_str);

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
        resolve("b"),
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
    let root_operation = loader.root_operation();
    assert_eq!(resolve(&root_operation.id().hex()).unwrap(), root_operation);
    assert_eq!(resolve("00").unwrap(), root_operation);
    assert_eq!(resolve("09").unwrap(), operations[4]);
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
        repos.push(tx.commit("test").unwrap());
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
            OpsetResolutionError::MultipleOperations { .. }
        ))
    );
    let op2_id_hex = operations[2].id().hex();
    assert_matches!(
        op_walk::resolve_op_with_repo(&repo, &format!("{op2_id_hex}+")),
        Err(OpsetEvaluationError::OpsetResolution(
            OpsetResolutionError::MultipleOperations { .. }
        ))
    );
}

#[test]
fn test_gc() {
    let settings = stable_op_id_settings();
    let test_repo = TestRepo::init();
    let op_dir = test_repo.repo_path().join("op_store").join("operations");
    let view_dir = test_repo.repo_path().join("op_store").join("views");
    let repo_0 = test_repo.repo;
    let op_store = repo_0.op_store();

    // Set up operation graph:
    //
    //   F
    //   E (empty)
    // D |
    // C |
    // |/
    // B
    // A
    // 0 (root)
    let empty_tx = |repo: &Arc<ReadonlyRepo>| repo.start_transaction(&settings);
    let random_tx = |repo: &Arc<ReadonlyRepo>| {
        let mut tx = repo.start_transaction(&settings);
        write_random_commit(tx.repo_mut(), &settings);
        tx
    };
    let repo_a = random_tx(&repo_0).commit("op A").unwrap();
    let repo_b = random_tx(&repo_a).commit("op B").unwrap();
    let repo_c = random_tx(&repo_b).commit("op C").unwrap();
    let repo_d = random_tx(&repo_c).commit("op D").unwrap();
    let repo_e = empty_tx(&repo_b).commit("op E").unwrap();
    let repo_f = random_tx(&repo_e).commit("op F").unwrap();

    // Sanity check for the original state
    let mut expected_op_entries = list_dir(&op_dir);
    let mut expected_view_entries = list_dir(&view_dir);
    assert_eq!(expected_op_entries.len(), 6);
    assert_eq!(expected_view_entries.len(), 5);

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
    assert_eq!(expected_op_entries.len(), 1);
    assert_eq!(expected_view_entries.len(), 1);
}
