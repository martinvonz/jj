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

use std::sync::Arc;

use jujutsu_lib::backend::CommitId;
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::default_index_store::{CompositeIndex, MutableIndexImpl, ReadonlyIndexImpl};
use jujutsu_lib::repo::{MutableRepo, ReadonlyRepo, Repo};
use jujutsu_lib::settings::UserSettings;
use test_case::test_case;
use testutils::{
    create_random_commit, load_repo_at_head, write_random_commit, CommitGraphBuilder, TestRepo,
};

fn child_commit<'repo>(
    mut_repo: &'repo mut MutableRepo,
    settings: &UserSettings,
    commit: &Commit,
) -> CommitBuilder<'repo> {
    create_random_commit(mut_repo, settings).set_parents(vec![commit.id().clone()])
}

// Helper just to reduce line wrapping
fn generation_number(index: CompositeIndex, commit_id: &CommitId) -> u32 {
    index.entry_by_id(commit_id).unwrap().generation_number()
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_empty_repo(use_git: bool) {
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let index = as_readonly_composite(repo);
    // There should be just the root commit
    assert_eq!(index.num_commits(), 1);

    // Check the generation numbers of the root and the working copy
    assert_eq!(generation_number(index, repo.store().root_commit_id()), 0);
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_standard_cases(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
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
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_b, &commit_e]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_f]);
    let commit_h = graph_builder.commit_with_parents(&[&commit_e]);
    let repo = tx.commit();

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

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_criss_cross(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let num_generations = 50;

    // Create a long chain of criss-crossed merges. If they were traversed without
    // keeping track of visited nodes, it would be 2^50 visits, so if this test
    // finishes in reasonable time, we know that we don't do a naive traversal.
    let mut tx = repo.start_transaction(&settings, "test");
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
    let repo = tx.commit();

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

    // RevWalk deduplicates chains by entry.
    assert_eq!(
        index
            .walk_revs(&[left_commits[num_generations - 1].id().clone()], &[])
            .count(),
        2 * num_generations
    );
    assert_eq!(
        index
            .walk_revs(&[right_commits[num_generations - 1].id().clone()], &[])
            .count(),
        2 * num_generations
    );
    assert_eq!(
        index
            .walk_revs(
                &[left_commits[num_generations - 1].id().clone()],
                &[left_commits[num_generations - 2].id().clone()]
            )
            .count(),
        2
    );
    assert_eq!(
        index
            .walk_revs(
                &[right_commits[num_generations - 1].id().clone()],
                &[right_commits[num_generations - 2].id().clone()]
            )
            .count(),
        2
    );

    // RevWalkGenerationRange deduplicates chains by (entry, generation), which may
    // be more expensive than RevWalk, but should still finish in reasonable time.
    assert_eq!(
        index
            .walk_revs(&[left_commits[num_generations - 1].id().clone()], &[])
            .filter_by_generation(0..(num_generations + 1) as u32)
            .count(),
        2 * num_generations
    );
    assert_eq!(
        index
            .walk_revs(&[right_commits[num_generations - 1].id().clone()], &[])
            .filter_by_generation(0..(num_generations + 1) as u32)
            .count(),
        2 * num_generations
    );
    assert_eq!(
        index
            .walk_revs(
                &[left_commits[num_generations - 1].id().clone()],
                &[left_commits[num_generations - 2].id().clone()]
            )
            .filter_by_generation(0..(num_generations + 1) as u32)
            .count(),
        2
    );
    assert_eq!(
        index
            .walk_revs(
                &[right_commits[num_generations - 1].id().clone()],
                &[right_commits[num_generations - 2].id().clone()]
            )
            .filter_by_generation(0..(num_generations + 1) as u32)
            .count(),
        2
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_previous_operations(use_git: bool) {
    // Test that commits visible only in previous operations are indexed.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    // Remove commit B and C in one operation and make sure they're still
    // visible in the index after that operation.
    // o C
    // o B
    // o A
    // | o working copy
    // |/
    // o root

    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let repo = tx.commit();

    let mut tx = repo.start_transaction(&settings, "test");
    tx.mut_repo().remove_head(commit_c.id());
    let repo = tx.commit();

    // Delete index from disk
    let index_operations_dir = repo.repo_path().join("index").join("operations");
    assert!(index_operations_dir.is_dir());
    std::fs::remove_dir_all(&index_operations_dir).unwrap();
    std::fs::create_dir(&index_operations_dir).unwrap();

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

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_incremental(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
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
    let mut tx = repo.start_transaction(&settings, "test");
    let commit_a = child_commit(tx.mut_repo(), &settings, &root_commit)
        .write()
        .unwrap();
    let repo = tx.commit();

    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 1 more
    assert_eq!(index.num_commits(), 1 + 1);

    let mut tx = repo.start_transaction(&settings, "test");
    let commit_b = child_commit(tx.mut_repo(), &settings, &commit_a)
        .write()
        .unwrap();
    let commit_c = child_commit(tx.mut_repo(), &settings, &commit_b)
        .write()
        .unwrap();
    tx.commit();

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

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_incremental_empty_transaction(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    // Create A in one operation, then just an empty transaction. Check that the
    // index is valid after.
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let commit_a = child_commit(tx.mut_repo(), &settings, &root_commit)
        .write()
        .unwrap();
    let repo = tx.commit();

    let index = as_readonly_composite(&repo);
    // There should be the root commit, plus 1 more
    assert_eq!(index.num_commits(), 1 + 1);

    repo.start_transaction(&settings, "test").commit();

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

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_incremental_already_indexed(use_git: bool) {
    // Tests that trying to add a commit that's already been added is a no-op.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    // Create A in one operation, then try to add it again an new transaction.
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let commit_a = child_commit(tx.mut_repo(), &settings, &root_commit)
        .write()
        .unwrap();
    let repo = tx.commit();

    assert!(repo.index().has_id(commit_a.id()));
    assert_eq!(as_readonly_composite(&repo).num_commits(), 1 + 1);
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    mut_repo.add_head(&commit_a);
    assert_eq!(as_mutable_composite(mut_repo).num_commits(), 1 + 1);
}

#[must_use]
fn create_n_commits(
    settings: &UserSettings,
    repo: &Arc<ReadonlyRepo>,
    num_commits: i32,
) -> Arc<ReadonlyRepo> {
    let mut tx = repo.start_transaction(settings, "test");
    for _ in 0..num_commits {
        write_random_commit(tx.mut_repo(), settings);
    }
    tx.commit()
}

fn as_readonly_impl(repo: &Arc<ReadonlyRepo>) -> &ReadonlyIndexImpl {
    repo.readonly_index()
        .as_index()
        .as_any()
        .downcast_ref::<ReadonlyIndexImpl>()
        .unwrap()
}

fn as_readonly_composite(repo: &Arc<ReadonlyRepo>) -> CompositeIndex<'_> {
    as_readonly_impl(repo).as_composite()
}

fn as_mutable_impl(repo: &MutableRepo) -> &MutableIndexImpl {
    repo.index()
        .as_any()
        .downcast_ref::<MutableIndexImpl>()
        .unwrap()
}

fn as_mutable_composite(repo: &MutableRepo) -> CompositeIndex<'_> {
    as_mutable_impl(repo).as_composite()
}

fn commits_by_level(repo: &Arc<ReadonlyRepo>) -> Vec<u32> {
    as_readonly_composite(repo)
        .stats()
        .levels
        .iter()
        .map(|level| level.num_commits)
        .collect()
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_commits_incremental_squashed(use_git: bool) {
    let settings = testutils::user_settings();

    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 1);
    assert_eq!(commits_by_level(&repo), vec![2]);
    let repo = create_n_commits(&settings, &repo, 1);
    assert_eq!(commits_by_level(&repo), vec![3]);

    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 2);
    assert_eq!(commits_by_level(&repo), vec![3]);

    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 100);
    assert_eq!(commits_by_level(&repo), vec![101]);

    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 1);
    let repo = create_n_commits(&settings, &repo, 2);
    let repo = create_n_commits(&settings, &repo, 4);
    let repo = create_n_commits(&settings, &repo, 8);
    let repo = create_n_commits(&settings, &repo, 16);
    let repo = create_n_commits(&settings, &repo, 32);
    assert_eq!(commits_by_level(&repo), vec![64]);

    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 32);
    let repo = create_n_commits(&settings, &repo, 16);
    let repo = create_n_commits(&settings, &repo, 8);
    let repo = create_n_commits(&settings, &repo, 4);
    let repo = create_n_commits(&settings, &repo, 2);
    assert_eq!(commits_by_level(&repo), vec![57, 6]);

    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;
    let repo = create_n_commits(&settings, repo, 30);
    let repo = create_n_commits(&settings, &repo, 15);
    let repo = create_n_commits(&settings, &repo, 7);
    let repo = create_n_commits(&settings, &repo, 3);
    let repo = create_n_commits(&settings, &repo, 1);
    assert_eq!(commits_by_level(&repo), vec![31, 15, 7, 3, 1]);

    let test_repo = TestRepo::init(use_git);
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

/// Test that .jj/repo/index/type is created when the repo is created, and that
/// it is created when an old repo is loaded.
#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_index_store_type(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
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
