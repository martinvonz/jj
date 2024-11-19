// Copyright 2024 The Jujutsu Authors
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

use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::rewrite::duplicate_commits;
use jj_lib::transaction::Transaction;
use testutils::create_tree;
use testutils::TestRepo;

#[test]
fn test_duplicate_linear_contents() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let path_1 = RepoPath::from_internal_string("file1");
    let path_2 = RepoPath::from_internal_string("file2");
    let empty_tree_id = repo.store().empty_merged_tree_id();
    let tree_1 = create_tree(repo, &[(path_1, "content1")]);
    let tree_2 = create_tree(repo, &[(path_2, "content2")]);
    let tree_1_2 = create_tree(repo, &[(path_1, "content1"), (path_2, "content2")]);

    // E [=file2]
    // D [-file1, =file2]
    // C [=file1, +file2]
    // B [+file1]
    // A []
    let mut tx = repo.start_transaction(&settings);
    let commit_a = tx
        .repo_mut()
        .new_commit(
            &settings,
            vec![repo.store().root_commit_id().clone()],
            empty_tree_id.clone(),
        )
        .write()
        .unwrap();
    let commit_b = tx
        .repo_mut()
        .new_commit(&settings, vec![commit_a.id().clone()], tree_1.id())
        .write()
        .unwrap();
    let commit_c = tx
        .repo_mut()
        .new_commit(&settings, vec![commit_b.id().clone()], tree_1_2.id())
        .write()
        .unwrap();
    let commit_d = tx
        .repo_mut()
        .new_commit(&settings, vec![commit_c.id().clone()], tree_2.id())
        .write()
        .unwrap();
    let commit_e = tx
        .repo_mut()
        .new_commit(&settings, vec![commit_d.id().clone()], tree_2.id())
        .write()
        .unwrap();
    let repo = tx.commit("test").unwrap();

    let duplicate_in_between = |tx: &mut Transaction,
                                target_commits: &[&CommitId],
                                parent_commit_ids: &[&CommitId],
                                children_commit_ids: &[&CommitId]| {
        duplicate_commits(
            &settings,
            tx.repo_mut(),
            &target_commits.iter().copied().cloned().collect_vec(),
            &parent_commit_ids.iter().copied().cloned().collect_vec(),
            &children_commit_ids.iter().copied().cloned().collect_vec(),
        )
        .unwrap()
    };
    let duplicate_onto =
        |tx: &mut Transaction, target_commits: &[&CommitId], parent_commit_ids: &[&CommitId]| {
            duplicate_in_between(tx, target_commits, parent_commit_ids, &[])
        };

    // Duplicate empty commit onto empty ancestor tree
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_e.id()], &[commit_a.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_e.id()].tree_id(),
        &empty_tree_id
    );

    // Duplicate empty commit onto non-empty ancestor tree
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_e.id()], &[commit_b.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_e.id()].tree_id(),
        &tree_1.id()
    );

    // Duplicate non-empty commit onto empty ancestor tree
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_c.id()], &[commit_a.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_c.id()].tree_id(),
        &tree_2.id()
    );

    // Duplicate non-empty commit onto non-empty ancestor tree
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_d.id()], &[commit_b.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_d.id()].tree_id(),
        &empty_tree_id
    );

    // Duplicate non-empty commit onto non-empty descendant tree
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_b.id()], &[commit_d.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_b.id()].tree_id(),
        &tree_1_2.id()
    );

    // Duplicate multiple contiguous commits
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_e.id(), commit_d.id()], &[commit_b.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_d.id()].tree_id(),
        &empty_tree_id
    );
    assert_eq!(
        stats.duplicated_commits[commit_e.id()].tree_id(),
        &empty_tree_id
    );

    // Duplicate multiple non-contiguous commits
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_e.id(), commit_c.id()], &[commit_a.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_c.id()].tree_id(),
        &tree_2.id()
    );
    assert_eq!(
        stats.duplicated_commits[commit_e.id()].tree_id(),
        &tree_2.id()
    );

    // Duplicate onto multiple parents
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_onto(&mut tx, &[commit_d.id()], &[commit_c.id(), commit_b.id()]);
    assert_eq!(
        stats.duplicated_commits[commit_d.id()].tree_id(),
        &tree_2.id()
    );

    // Insert duplicated commit
    let mut tx = repo.start_transaction(&settings);
    let stats = duplicate_in_between(
        &mut tx,
        &[commit_b.id()],
        &[commit_d.id()],
        &[commit_e.id()],
    );
    assert_eq!(
        stats.duplicated_commits[commit_b.id()].tree_id(),
        &tree_1_2.id()
    );
    let (head_id,) = tx.repo().view().heads().iter().collect_tuple().unwrap();
    assert_ne!(head_id, commit_e.id());
    assert_eq!(
        tx.repo().store().get_commit(head_id).unwrap().tree_id(),
        &tree_1_2.id()
    );
}
