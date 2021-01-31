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

use jujube_lib::commit::Commit;
use jujube_lib::commit_builder::CommitBuilder;
use jujube_lib::index::IndexRef;
use jujube_lib::repo::ReadonlyRepo;
use jujube_lib::settings::UserSettings;
use jujube_lib::store::CommitId;
use jujube_lib::testutils;
use std::sync::Arc;
use test_case::test_case;

#[must_use]
fn child_commit(settings: &UserSettings, repo: &ReadonlyRepo, commit: &Commit) -> CommitBuilder {
    testutils::create_random_commit(&settings, repo).set_parents(vec![commit.id().clone()])
}

// Helper just to reduce line wrapping
fn generation_number<'a>(index: impl Into<IndexRef<'a>>, commit_id: &CommitId) -> u32 {
    index
        .into()
        .entry_by_id(commit_id)
        .unwrap()
        .generation_number()
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_index_commits_empty_repo(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let index = repo.index();
    // There should be the root commit and the working copy commit
    assert_eq!(index.num_commits(), 2);

    // Check the generation numbers of the root and the working copy
    assert_eq!(
        generation_number(index.clone(), repo.store().root_commit_id()),
        0
    );
    assert_eq!(
        generation_number(
            index.clone(),
            &repo.working_copy_locked().current_commit_id()
        ),
        1
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_index_commits_standard_cases(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

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

    let root_commit = repo.store().root_commit();
    let wc_commit = repo.working_copy_locked().current_commit();
    let mut tx = repo.start_transaction("test");
    let commit_a = child_commit(&settings, &repo, &root_commit).write_to_transaction(&mut tx);
    let commit_b = child_commit(&settings, &repo, &commit_a).write_to_transaction(&mut tx);
    let commit_c = child_commit(&settings, &repo, &commit_a).write_to_transaction(&mut tx);
    let commit_d = child_commit(&settings, &repo, &commit_c).write_to_transaction(&mut tx);
    let commit_e = child_commit(&settings, &repo, &commit_d).write_to_transaction(&mut tx);
    let commit_f = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit_b.id().clone(), commit_e.id().clone()])
        .write_to_transaction(&mut tx);
    let commit_g = child_commit(&settings, &repo, &commit_f).write_to_transaction(&mut tx);
    let commit_h = child_commit(&settings, &repo, &commit_e).write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let index = repo.index();
    // There should be the root commit and the working copy commit, plus
    // 8 more
    assert_eq!(index.num_commits(), 2 + 8);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 2 + 8);
    assert_eq!(stats.num_merges, 1);
    assert_eq!(stats.max_generation_number, 6);

    assert_eq!(generation_number(index.clone(), root_commit.id()), 0);
    assert_eq!(generation_number(index.clone(), wc_commit.id()), 1);
    assert_eq!(generation_number(index.clone(), commit_a.id()), 1);
    assert_eq!(generation_number(index.clone(), commit_b.id()), 2);
    assert_eq!(generation_number(index.clone(), commit_c.id()), 2);
    assert_eq!(generation_number(index.clone(), commit_d.id()), 3);
    assert_eq!(generation_number(index.clone(), commit_e.id()), 4);
    assert_eq!(generation_number(index.clone(), commit_f.id()), 5);
    assert_eq!(generation_number(index.clone(), commit_g.id()), 6);
    assert_eq!(generation_number(index.clone(), commit_h.id()), 5);

    assert!(index.is_ancestor(root_commit.id(), commit_a.id()));
    assert!(!index.is_ancestor(commit_a.id(), root_commit.id()));

    assert!(index.is_ancestor(root_commit.id(), commit_b.id()));
    assert!(!index.is_ancestor(commit_b.id(), root_commit.id()));

    assert!(!index.is_ancestor(commit_b.id(), commit_c.id()));

    assert!(index.is_ancestor(commit_a.id(), commit_b.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_e.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_f.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_g.id()));
    assert!(index.is_ancestor(commit_a.id(), commit_h.id()));
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_index_commits_criss_cross(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let num_generations = 50;
    let root_commit = repo.store().root_commit();

    // Create a long chain of criss-crossed merges. If they were traversed without
    // keeping track of visited nodes, it would be 2^50 visits, so if this test
    // finishes in reasonable time, we know that we don't do a naive traversal.
    let mut tx = repo.start_transaction("test");
    let mut left_commits =
        vec![child_commit(&settings, &repo, &root_commit).write_to_transaction(&mut tx)];
    let mut right_commits =
        vec![child_commit(&settings, &repo, &root_commit).write_to_transaction(&mut tx)];
    for gen in 1..num_generations {
        let new_left = testutils::create_random_commit(&settings, &repo)
            .set_parents(vec![
                left_commits[gen - 1].id().clone(),
                right_commits[gen - 1].id().clone(),
            ])
            .write_to_transaction(&mut tx);
        let new_right = testutils::create_random_commit(&settings, &repo)
            .set_parents(vec![
                left_commits[gen - 1].id().clone(),
                right_commits[gen - 1].id().clone(),
            ])
            .write_to_transaction(&mut tx);
        left_commits.push(new_left);
        right_commits.push(new_right);
    }
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let index = repo.index();
    // There should the root commit and the working copy commit, plus 2 for each
    // generation
    assert_eq!(index.num_commits(), 2 + 2 * (num_generations as u32));

    let stats = index.stats();
    assert_eq!(stats.num_commits, 2 + 2 * (num_generations as u32));
    // The first generations are not merges
    assert_eq!(stats.num_merges, 2 * (num_generations as u32 - 1));
    assert_eq!(stats.max_generation_number, num_generations as u32);

    // Check generation numbers
    for gen in 0..num_generations {
        assert_eq!(
            generation_number(index.clone(), left_commits[gen].id()),
            (gen as u32) + 1
        );
        assert_eq!(
            generation_number(index.clone(), right_commits[gen].id()),
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
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_index_commits_previous_operations(use_git: bool) {
    // Test that commits visible only in previous operations are indexed.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    // Remove commit B and C in one operation and make sure they're still
    // visible in the index after that operation.
    // o C
    // o B
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let commit_a = child_commit(&settings, &repo, &root_commit).write_to_transaction(&mut tx);
    let commit_b = child_commit(&settings, &repo, &commit_a).write_to_transaction(&mut tx);
    let commit_c = child_commit(&settings, &repo, &commit_b).write_to_transaction(&mut tx);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    let mut tx = repo.start_transaction("test");
    tx.remove_head(&commit_c);
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    // Delete index from disk
    let index_operations_dir = repo
        .working_copy_path()
        .join(".jj")
        .join("index")
        .join("operations");
    assert!(index_operations_dir.is_dir());
    std::fs::remove_dir_all(&index_operations_dir).unwrap();
    std::fs::create_dir(&index_operations_dir).unwrap();

    let repo = ReadonlyRepo::load(&settings, repo.working_copy_path().clone()).unwrap();
    let index = repo.index();
    // There should be the root commit and the working copy commit, plus
    // 3 more
    assert_eq!(index.num_commits(), 2 + 3);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 2 + 3);
    assert_eq!(stats.num_merges, 0);
    assert_eq!(stats.max_generation_number, 3);

    assert_eq!(generation_number(index.clone(), commit_a.id()), 1);
    assert_eq!(generation_number(index.clone(), commit_b.id()), 2);
    assert_eq!(generation_number(index.clone(), commit_c.id()), 3);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_index_commits_incremental(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    // Create A in one operation, then B and C in another. Check that the index is
    // valid after.
    // o C
    // o B
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let commit_a =
        child_commit(&settings, &repo, &root_commit).write_to_new_transaction(&repo, "test");
    Arc::get_mut(&mut repo).unwrap().reload();

    let index = repo.index();
    // There should be the root commit and the working copy commit, plus
    // 1 more
    assert_eq!(index.num_commits(), 2 + 1);

    let mut tx = repo.start_transaction("test");
    let commit_b = child_commit(&settings, &repo, &commit_a).write_to_transaction(&mut tx);
    let commit_c = child_commit(&settings, &repo, &commit_b).write_to_transaction(&mut tx);
    tx.commit();

    let repo = ReadonlyRepo::load(&settings, repo.working_copy_path().clone()).unwrap();
    let index = repo.index();
    // There should be the root commit and the working copy commit, plus
    // 3 more
    assert_eq!(index.num_commits(), 2 + 3);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 2 + 3);
    assert_eq!(stats.num_merges, 0);
    assert_eq!(stats.max_generation_number, 3);
    assert_eq!(stats.levels.len(), 3);
    assert_eq!(stats.levels[0].num_commits, 2);
    assert_eq!(stats.levels[1].num_commits, 1);
    assert_ne!(stats.levels[1].name, stats.levels[0].name);
    assert_eq!(stats.levels[2].num_commits, 2);
    assert_ne!(stats.levels[2].name, stats.levels[0].name);
    assert_ne!(stats.levels[2].name, stats.levels[1].name);

    assert_eq!(generation_number(index.clone(), root_commit.id()), 0);
    assert_eq!(generation_number(index.clone(), commit_a.id()), 1);
    assert_eq!(generation_number(index.clone(), commit_b.id()), 2);
    assert_eq!(generation_number(index.clone(), commit_c.id()), 3);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_index_commits_incremental_empty_transaction(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    // Create A in one operation, then just an empty transaction. Check that the
    // index is valid after.
    // o A
    // | o working copy
    // |/
    // o root

    let root_commit = repo.store().root_commit();
    let commit_a =
        child_commit(&settings, &repo, &root_commit).write_to_new_transaction(&repo, "test");
    Arc::get_mut(&mut repo).unwrap().reload();

    let index = repo.index();
    // There should be the root commit and the working copy commit, plus
    // 1 more
    assert_eq!(index.num_commits(), 2 + 1);

    repo.start_transaction("test").commit();

    let repo = ReadonlyRepo::load(&settings, repo.working_copy_path().clone()).unwrap();
    let index = repo.index();
    // There should be the root commit and the working copy commit, plus
    // 1 more
    assert_eq!(index.num_commits(), 2 + 1);

    let stats = index.stats();
    assert_eq!(stats.num_commits, 2 + 1);
    assert_eq!(stats.num_merges, 0);
    assert_eq!(stats.max_generation_number, 1);
    assert_eq!(stats.levels.len(), 3);
    assert_eq!(stats.levels[0].num_commits, 0);
    assert_eq!(stats.levels[1].num_commits, 1);
    assert_eq!(stats.levels[2].num_commits, 2);

    assert_eq!(generation_number(index.clone(), root_commit.id()), 0);
    assert_eq!(generation_number(index.clone(), commit_a.id()), 1);
}
