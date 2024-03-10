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

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use assert_matches::assert_matches;
use jj_lib::backend::{ChangeId, CommitId};
use jj_lib::commit::Commit;
use jj_lib::commit_builder::CommitBuilder;
use jj_lib::default_index::{
    AsCompositeIndex as _, CompositeIndex, DefaultIndexStore, DefaultIndexStoreError,
    DefaultMutableIndex, DefaultReadonlyIndex,
};
use jj_lib::index::Index as _;
use jj_lib::object_id::{HexPrefix, ObjectId as _, PrefixResolution};
use jj_lib::op_store::{RefTarget, RemoteRef};
use jj_lib::repo::{MutableRepo, ReadonlyRepo, Repo};
use jj_lib::revset::{ResolvedExpression, GENERATION_RANGE_FULL};
use jj_lib::settings::UserSettings;
use maplit::hashset;
use testutils::test_backend::TestBackend;
use testutils::{
    commit_transactions, create_random_commit, load_repo_at_head, write_random_commit,
    CommitGraphBuilder, TestRepo,
};

fn child_commit<'repo>(
    mut_repo: &'repo mut MutableRepo,
    settings: &UserSettings,
    commit: &Commit,
) -> CommitBuilder<'repo> {
    create_random_commit(mut_repo, settings).set_parents(vec![commit.id().clone()])
}

// Helper just to reduce line wrapping
fn generation_number(index: &CompositeIndex, commit_id: &CommitId) -> u32 {
    index.entry_by_id(commit_id).unwrap().generation_number()
}

#[test]
fn test_index_commits_empty_repo() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let index = as_readonly_composite(repo);
    // There should be just the root commit
    assert_eq!(index.num_commits(), 1);

    // Check the generation numbers of the root and the working copy
    assert_eq!(generation_number(index, repo.store().root_commit_id()), 0);
}

#[test]
fn test_index_commits_standard_cases() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    //   o H
    // o | G
    // o | F
    // |\|
    // | o E
    // | o D
    // | o C
    // o | B
    // |/
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit_id = repo.store().root_commit_id();
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_b, &commit_e]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_f]);
    let commit_h = graph_builder.commit_with_parents(&[&commit_e]);
    let repo = tx.commit("test");

    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 8 more
    assert_eq!(index.num_commits(), 1 + 8);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 1 + 8);
    assert_eq!(stats.num_merges, 1);
    assert_eq!(stats.max_generation_number, 6);

    assert_eq!(generation_number(index, root_commit_id), 0);
    assert_eq!(generation_number(index, commit_a.id()), 1);
    assert_eq!(generation_number(index, commit_b.id()), 2);
    assert_eq!(generation_number(index, commit_c.id()), 2);
    assert_eq!(generation_number(index, commit_d.id()), 3);
    assert_eq!(generation_number(index, commit_e.id()), 4);
    assert_eq!(generation_number(index, commit_f.id()), 5);
    assert_eq!(generation_number(index, commit_g.id()), 6);
    assert_eq!(generation_number(index, commit_h.id()), 5);

    assert!(index.is_ancestor(root_commit_id, commit_a.id()));
    assert!(!index.is_ancestor(commit_a.id(), root_commit_id));

    assert!(index.is_ancestor(root_commit_id, commit_b.id()));
    assert!(!index.is_ancestor(commit_b.id(), root_commit_id));

    assert!(!index.is_ancestor(commit_b.id(), commit_c.id()));

    assert!(index.is_ancestor(commit_a.id(), commit_b.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_e.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_f.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_g.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_h.id()));
}

#[test]
fn test_index_commits_criss_cross() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let num_generations = 50;

    // Create a long chain of criss-crossed merges. If they were traversed without
    // keeping track of visited nodes, it would be 2^50 visits, so if this test
    // finishes in reasonable time, we know that we don't do a naive traversal.
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let mut left_commits = vec![graph_builder.initial_commit()];
    let mut right_commits = vec![graph_builder.initial_commit()];
    for gen in 1..num_generations {
        let new_left =
            graph_builder.commit_with_parents(&[&left_commits[gen - 1], &right_commits[gen - 1]]);
        let new_right =
            graph_builder.commit_with_parents(&[&left_commits[gen - 1], &right_commits[gen - 1]]);
        left_commits.push(new_left);
        right_commits.push(new_right);
    }
    let repo = tx.commit("test");

    let index = as_readonly_composite(&repo);
    // There should the root commit, plus 2 for each generation
    assert_eq!(index.num_commits(), 1 + 2 * (num_generations as u32));

    let stats = index.stats();
    assert_eq!(stats.num_commits, 1 + 2 * (num_generations as u32));
    // The first generations are not merges
    assert_eq!(stats.num_merges, 2 * (num_generations as u32 - 1));
    assert_eq!(stats.max_generation_number, num_generations as u32);

    // Check generation numbers
    for gen in 0..num_generations {
        assert_eq!(
            generation_number(index, left_commits[gen].id()),
            (gen as u32) + 1
        );
        assert_eq!(
            generation_number(index, right_commits[gen].id()),
            (gen as u32) + 1
        );
    }

    // The left and right commits of the same generation should not be ancestors of
    // each other
    for gen in 0..num_generations {
        assert!(!index.is_ancestor(left_commits[gen].id(), right_commits[gen].id()));
        assert!(!index.is_ancestor(right_commits[gen].id(), left_commits[gen].id()));
    }

    // Both sides of earlier generations should be ancestors. Check a few different
    // earlier generations.
    for gen in 1..num_generations {
        for ancestor_side in &[&left_commits, &right_commits] {
            for descendant_side in &[&left_commits, &right_commits] {
                assert!(index.is_ancestor(ancestor_side[0].id(), descendant_side[gen].id()));
                assert!(index.is_ancestor(ancestor_side[gen - 1].id(), descendant_side[gen].id()));
                assert!(index.is_ancestor(ancestor_side[gen / 2].id(), descendant_side[gen].id()));
            }
        }
    }

    let count_revs = |wanted: &[CommitId], unwanted: &[CommitId], generation| {
        // Constructs ResolvedExpression directly to bypass tree optimization.
        let expression = ResolvedExpression::Range {
            roots: ResolvedExpression::Commits(unwanted.to_vec()).into(),
            heads: ResolvedExpression::Commits(wanted.to_vec()).into(),
            generation,
        };
        let revset = index.evaluate_revset(&expression, repo.store()).unwrap();
        // Don't switch to more efficient .count() implementation. Here we're
        // testing the iterator behavior.
        revset.iter().count()
    };

    // RevWalk deduplicates chains by entry.
    assert_eq!(
        count_revs(
            &[left_commits[num_generations - 1].id().clone()],
            &[],
            GENERATION_RANGE_FULL,
        ),
        2 * num_generations
    );
    assert_eq!(
        count_revs(
            &[right_commits[num_generations - 1].id().clone()],
            &[],
            GENERATION_RANGE_FULL,
        ),
        2 * num_generations
    );
    assert_eq!(
        count_revs(
            &[left_commits[num_generations - 1].id().clone()],
            &[left_commits[num_generations - 2].id().clone()],
            GENERATION_RANGE_FULL,
        ),
        2
    );
    assert_eq!(
        count_revs(
            &[right_commits[num_generations - 1].id().clone()],
            &[right_commits[num_generations - 2].id().clone()],
            GENERATION_RANGE_FULL,
        ),
        2
    );

    // RevWalkGenerationRange deduplicates chains by (entry, generation), which may
    // be more expensive than RevWalk, but should still finish in reasonable time.
    assert_eq!(
        count_revs(
            &[left_commits[num_generations - 1].id().clone()],
            &[],
            0..(num_generations + 1) as u64,
        ),
        2 * num_generations
    );
    assert_eq!(
        count_revs(
            &[right_commits[num_generations - 1].id().clone()],
            &[],
            0..(num_generations + 1) as u64,
        ),
        2 * num_generations
    );
    assert_eq!(
        count_revs(
            &[left_commits[num_generations - 1].id().clone()],
            &[left_commits[num_generations - 2].id().clone()],
            0..(num_generations + 1) as u64,
        ),
        2
    );
    assert_eq!(
        count_revs(
            &[right_commits[num_generations - 1].id().clone()],
            &[right_commits[num_generations - 2].id().clone()],
            0..(num_generations + 1) as u64,
        ),
        2
    );
}

#[test]
fn test_index_commits_previous_operations() {
    // Test that commits visible only in previous operations are indexed.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Remove commit B and C in one operation and make sure they're still
    // visible in the index after that operation.
    // o C
    // o B
    // o A
    // | o working copy
    // |/
    // o root

    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().remove_head(commit_c.id());
    let repo = tx.commit("test");

    // Delete index from disk
    let default_index_store: &DefaultIndexStore =
        repo.index_store().as_any().downcast_ref().unwrap();
    default_index_store.reinit().unwrap();

    let repo = load_repo_at_head(&settings, repo.repo_path());
    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 3 more
    assert_eq!(index.num_commits(), 1 + 3);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 1 + 3);
    assert_eq!(stats.num_merges, 0);
    assert_eq!(stats.max_generation_number, 3);

    assert_eq!(generation_number(index, commit_a.id()), 1);
    assert_eq!(generation_number(index, commit_b.id()), 2);
    assert_eq!(generation_number(index, commit_c.id()), 3);
}

#[test]
fn test_index_commits_hidden_but_referenced() {
    // Test that hidden-but-referenced commits are indexed.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Remote branches are usually visible at a certain point in operation
    // history, but that's not guaranteed if old operations have been discarded.
    // This can also happen if imported remote branches get immediately
    // abandoned because the other branch has moved.
    let mut tx = repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let commit_b = write_random_commit(tx.mut_repo(), &settings);
    let commit_c = write_random_commit(tx.mut_repo(), &settings);
    tx.mut_repo().remove_head(commit_a.id());
    tx.mut_repo().remove_head(commit_b.id());
    tx.mut_repo().remove_head(commit_c.id());
    tx.mut_repo().set_remote_branch(
        "branch",
        "origin",
        RemoteRef {
            target: RefTarget::from_legacy_form(
                [commit_a.id().clone()],
                [commit_b.id().clone(), commit_c.id().clone()],
            ),
            state: jj_lib::op_store::RemoteRefState::New,
        },
    );
    let repo = tx.commit("test");

    // All commits should be indexed
    assert!(repo.index().has_id(commit_a.id()));
    assert!(repo.index().has_id(commit_b.id()));
    assert!(repo.index().has_id(commit_c.id()));

    // Delete index from disk
    let default_index_store: &DefaultIndexStore =
        repo.index_store().as_any().downcast_ref().unwrap();
    default_index_store.reinit().unwrap();

    let repo = load_repo_at_head(&settings, repo.repo_path());
    // All commits should be reindexed
    assert!(repo.index().has_id(commit_a.id()));
    assert!(repo.index().has_id(commit_b.id()));
    assert!(repo.index().has_id(commit_c.id()));
}

#[test]
fn test_index_commits_incremental() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Create A in one operation, then B and C in another. Check that the index is
    // valid after.
    // o C
    // o B
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings);
    let commit_a = child_commit(tx.mut_repo(), &settings, &root_commit)
        .write()
        .unwrap();
    let repo = tx.commit("test");

    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 1 more
    assert_eq!(index.num_commits(), 1 + 1);

    let mut tx = repo.start_transaction(&settings);
    let commit_b = child_commit(tx.mut_repo(), &settings, &commit_a)
        .write()
        .unwrap();
    let commit_c = child_commit(tx.mut_repo(), &settings, &commit_b)
        .write()
        .unwrap();
    tx.commit("test");

    let repo = load_repo_at_head(&settings, repo.repo_path());
    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 3 more
    assert_eq!(index.num_commits(), 1 + 3);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 1 + 3);
    assert_eq!(stats.num_merges, 0);
    assert_eq!(stats.max_generation_number, 3);
    assert_eq!(stats.levels.len(), 1);
    assert_eq!(stats.levels[0].num_commits, 4);

    assert_eq!(generation_number(index, root_commit.id()), 0);
    assert_eq!(generation_number(index, commit_a.id()), 1);
    assert_eq!(generation_number(index, commit_b.id()), 2);
    assert_eq!(generation_number(index, commit_c.id()), 3);
}

#[test]
fn test_index_commits_incremental_empty_transaction() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Create A in one operation, then just an empty transaction. Check that the
    // index is valid after.
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings);
    let commit_a = child_commit(tx.mut_repo(), &settings, &root_commit)
        .write()
        .unwrap();
    let repo = tx.commit("test");

    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 1 more
    assert_eq!(index.num_commits(), 1 + 1);

    repo.start_transaction(&settings).commit("test");

    let repo = load_repo_at_head(&settings, repo.repo_path());
    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 1 more
    assert_eq!(index.num_commits(), 1 + 1);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 1 + 1);
    assert_eq!(stats.num_merges, 0);
    assert_eq!(stats.max_generation_number, 1);
    assert_eq!(stats.levels.len(), 1);
    assert_eq!(stats.levels[0].num_commits, 2);

    assert_eq!(generation_number(index, root_commit.id()), 0);
    assert_eq!(generation_number(index, commit_a.id()), 1);
}

#[test]
fn test_index_commits_incremental_already_indexed() {
    // Tests that trying to add a commit that's already been added is a no-op.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Create A in one operation, then try to add it again an new transaction.
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings);
    let commit_a = child_commit(tx.mut_repo(), &settings, &root_commit)
        .write()
        .unwrap();
    let repo = tx.commit("test");

    assert!(repo.index().has_id(commit_a.id()));
    assert_eq!(as_readonly_composite(&repo).num_commits(), 1 + 1);
    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.mut_repo();
    mut_repo.add_head(&commit_a).unwrap();
    assert_eq!(as_mutable_composite(mut_repo).num_commits(), 1 + 1);
}

#[must_use]
fn create_n_commits(
    settings: &UserSettings,
    repo: &Arc<ReadonlyRepo>,
    num_commits: i32,
) -> Arc<ReadonlyRepo> {
    let mut tx = repo.start_transaction(settings);
    for _ in 0..num_commits {
        write_random_commit(tx.mut_repo(), settings);
    }
    tx.commit("test")
}

fn as_readonly_composite(repo: &Arc<ReadonlyRepo>) -> &CompositeIndex {
    repo.readonly_index()
        .as_any()
        .downcast_ref::<DefaultReadonlyIndex>()
        .unwrap()
        .as_composite()
}

fn as_mutable_composite(repo: &MutableRepo) -> &CompositeIndex {
    repo.mutable_index()
        .as_any()
        .downcast_ref::<DefaultMutableIndex>()
        .unwrap()
        .as_composite()
}

fn commits_by_level(repo: &Arc<ReadonlyRepo>) -> Vec<u32> {
    as_readonly_composite(repo)
        .stats()
        .levels
        .iter()
        .map(|level| level.num_commits)
        .collect()
}

#[test]
fn test_index_commits_incremental_squashed() {
    let settings = testutils::user_settings();

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 1);
    assert_eq!(commits_by_level(&repo), vec![2]);
    let repo = create_n_commits(&settings, &repo, 1);
    assert_eq!(commits_by_level(&repo), vec![3]);

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 2);
    assert_eq!(commits_by_level(&repo), vec![3]);

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 100);
    assert_eq!(commits_by_level(&repo), vec![101]);

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 1);
    let repo = create_n_commits(&settings, &repo, 2);
    let repo = create_n_commits(&settings, &repo, 4);
    let repo = create_n_commits(&settings, &repo, 8);
    let repo = create_n_commits(&settings, &repo, 16);
    let repo = create_n_commits(&settings, &repo, 32);
    assert_eq!(commits_by_level(&repo), vec![64]);

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 32);
    let repo = create_n_commits(&settings, &repo, 16);
    let repo = create_n_commits(&settings, &repo, 8);
    let repo = create_n_commits(&settings, &repo, 4);
    let repo = create_n_commits(&settings, &repo, 2);
    assert_eq!(commits_by_level(&repo), vec![57, 6]);

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 30);
    let repo = create_n_commits(&settings, &repo, 15);
    let repo = create_n_commits(&settings, &repo, 7);
    let repo = create_n_commits(&settings, &repo, 3);
    let repo = create_n_commits(&settings, &repo, 1);
    assert_eq!(commits_by_level(&repo), vec![31, 15, 7, 3, 1]);

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    let repo = create_n_commits(&settings, &repo, 10);
    assert_eq!(commits_by_level(&repo), vec![71, 20]);
}

#[test]
fn test_reindex_no_segments_dir() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("test");
    assert!(repo.index().has_id(commit_a.id()));

    // jj <= 0.14 doesn't have "segments" directory
    let segments_dir = repo.repo_path().join("index").join("segments");
    assert!(segments_dir.is_dir());
    fs::remove_dir_all(&segments_dir).unwrap();

    let repo = load_repo_at_head(&settings, repo.repo_path());
    assert!(repo.index().has_id(commit_a.id()));
}

#[test]
fn test_reindex_corrupt_segment_files() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("test");
    assert!(repo.index().has_id(commit_a.id()));

    // Corrupt the index files
    let segments_dir = repo.repo_path().join("index").join("segments");
    for entry in segments_dir.read_dir().unwrap() {
        let entry = entry.unwrap();
        // u32: file format version
        // u32: parent segment file name length (0 means root)
        // u32: number of local commit entries
        // u32: number of local change ids
        // u32: number of overflow parent entries
        // u32: number of overflow change id positions
        fs::write(entry.path(), b"\0".repeat(24)).unwrap()
    }

    let repo = load_repo_at_head(&settings, repo.repo_path());
    assert!(repo.index().has_id(commit_a.id()));
}

#[test]
fn test_reindex_from_merged_operation() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // The following operation log:
    // x (add head, index will be missing)
    // x (add head, index will be missing)
    // |\
    // o o (remove head)
    // o o (add head)
    // |/
    // o
    let mut txs = Vec::new();
    for _ in 0..2 {
        let mut tx = repo.start_transaction(&settings);
        let commit = write_random_commit(tx.mut_repo(), &settings);
        let repo = tx.commit("test");
        let mut tx = repo.start_transaction(&settings);
        tx.mut_repo().remove_head(commit.id());
        txs.push(tx);
    }
    let repo = commit_transactions(&settings, txs);
    let mut op_ids_to_delete = Vec::new();
    op_ids_to_delete.push(repo.op_id());
    let mut tx = repo.start_transaction(&settings);
    write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("test");
    op_ids_to_delete.push(repo.op_id());
    let operation_to_reload = repo.operation();

    // Sanity check before corrupting the index store
    let index = as_readonly_composite(&repo);
    assert_eq!(index.num_commits(), 4);

    let index_operations_dir = repo.repo_path().join("index").join("operations");
    for &op_id in &op_ids_to_delete {
        fs::remove_file(index_operations_dir.join(op_id.hex())).unwrap();
    }

    // When re-indexing, one of the merge parent operations will be selected as
    // the parent index segment. The commits in the other side should still be
    // reachable.
    let repo = repo.reload_at(operation_to_reload).unwrap();
    let index = as_readonly_composite(&repo);
    assert_eq!(index.num_commits(), 4);
}

#[test]
fn test_reindex_missing_commit() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let missing_commit = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("test");
    let bad_op_id = repo.op_id();

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().remove_head(missing_commit.id());
    let repo = tx.commit("test");

    // Remove historical head commit to simulate bad GC.
    let test_backend: &TestBackend = repo.store().backend_impl().downcast_ref().unwrap();
    test_backend.remove_commit_unchecked(missing_commit.id());
    let repo = load_repo_at_head(&settings, repo.repo_path()); // discard cache
    assert!(repo.store().get_commit(missing_commit.id()).is_err());

    // Reindexing error should include the operation id where the commit
    // couldn't be found.
    let default_index_store: &DefaultIndexStore =
        repo.index_store().as_any().downcast_ref().unwrap();
    default_index_store.reinit().unwrap();
    let err = default_index_store
        .build_index_at_operation(repo.operation(), repo.store())
        .unwrap_err();
    assert_matches!(err, DefaultIndexStoreError::IndexCommits { op_id, .. } if op_id == *bad_op_id);
}

/// Test that .jj/repo/index/type is created when the repo is created, and that
/// it is created when an old repo is loaded.
#[test]
fn test_index_store_type() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    assert_eq!(as_readonly_composite(repo).num_commits(), 1);
    let index_store_type_path = repo.repo_path().join("index").join("type");
    assert_eq!(
        std::fs::read_to_string(&index_store_type_path).unwrap(),
        "default"
    );
    // Remove the file to simulate an old repo. Loading the repo should result in
    // the file being created.
    std::fs::remove_file(&index_store_type_path).unwrap();
    let repo = load_repo_at_head(&settings, repo.repo_path());
    assert_eq!(
        std::fs::read_to_string(&index_store_type_path).unwrap(),
        "default"
    );
    assert_eq!(as_readonly_composite(&repo).num_commits(), 1);
}

#[test]
fn test_change_id_index() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);

    let root_commit = repo.store().root_commit();
    let mut commit_number = 0;
    let mut commit_with_change_id = |change_id| {
        commit_number += 1;
        tx.mut_repo()
            .new_commit(
                &settings,
                vec![root_commit.id().clone()],
                root_commit.tree_id().clone(),
            )
            .set_change_id(ChangeId::from_hex(change_id))
            .set_description(format!("commit {commit_number}"))
            .write()
            .unwrap()
    };
    let commit_1 = commit_with_change_id("abbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let commit_2 = commit_with_change_id("aaaaabbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let commit_3 = commit_with_change_id("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let commit_4 = commit_with_change_id("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let commit_5 = commit_with_change_id("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

    let index_for_heads = |commits: &[&Commit]| {
        tx.repo()
            .mutable_index()
            .change_id_index(&mut commits.iter().map(|commit| commit.id()))
    };
    let change_id_index = index_for_heads(&[&commit_1, &commit_2, &commit_3, &commit_4, &commit_5]);
    let prefix_len =
        |commit: &Commit| change_id_index.shortest_unique_prefix_len(commit.change_id());
    assert_eq!(prefix_len(&root_commit), 1);
    assert_eq!(prefix_len(&commit_1), 2);
    assert_eq!(prefix_len(&commit_2), 6);
    assert_eq!(prefix_len(&commit_3), 6);
    assert_eq!(prefix_len(&commit_4), 1);
    assert_eq!(prefix_len(&commit_5), 1);
    let resolve_prefix = |prefix: &str| {
        change_id_index
            .resolve_prefix(&HexPrefix::new(prefix).unwrap())
            .map(HashSet::from_iter)
    };
    // Ambiguous matches
    assert_eq!(resolve_prefix("a"), PrefixResolution::AmbiguousMatch);
    assert_eq!(resolve_prefix("aaaaa"), PrefixResolution::AmbiguousMatch);
    // Exactly the necessary length
    assert_eq!(
        resolve_prefix("0"),
        PrefixResolution::SingleMatch(hashset! {root_commit.id().clone()})
    );
    assert_eq!(
        resolve_prefix("aaaaaa"),
        PrefixResolution::SingleMatch(hashset! {commit_3.id().clone()})
    );
    assert_eq!(
        resolve_prefix("aaaaab"),
        PrefixResolution::SingleMatch(hashset! {commit_2.id().clone()})
    );
    assert_eq!(
        resolve_prefix("ab"),
        PrefixResolution::SingleMatch(hashset! {commit_1.id().clone()})
    );
    assert_eq!(
        resolve_prefix("b"),
        PrefixResolution::SingleMatch(hashset! {commit_4.id().clone(), commit_5.id().clone()})
    );
    // Longer than necessary
    assert_eq!(
        resolve_prefix("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        PrefixResolution::SingleMatch(hashset! {commit_3.id().clone()})
    );
    // No match
    assert_eq!(resolve_prefix("ba"), PrefixResolution::NoMatch);

    // Test with an index containing only some of the commits. The shortest
    // length doesn't have to be minimized further, but unreachable commits
    // should never be included in the resolved set.
    let change_id_index = index_for_heads(&[&commit_1, &commit_2]);
    let resolve_prefix = |prefix: &str| {
        change_id_index
            .resolve_prefix(&HexPrefix::new(prefix).unwrap())
            .map(HashSet::from_iter)
    };
    assert_eq!(
        resolve_prefix("0"),
        PrefixResolution::SingleMatch(hashset! {root_commit.id().clone()})
    );
    assert_eq!(
        resolve_prefix("aaaaab"),
        PrefixResolution::SingleMatch(hashset! {commit_2.id().clone()})
    );
    assert_eq!(resolve_prefix("aaaaaa"), PrefixResolution::NoMatch);
    assert_eq!(resolve_prefix("a"), PrefixResolution::AmbiguousMatch);
    assert_eq!(resolve_prefix("b"), PrefixResolution::NoMatch);
}
