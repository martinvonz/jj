// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use jj_lib::repo::RepoLoader;
use testutils::{write_random_commit, TestRepo};

#[test]
fn test_load_at_operation() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let commit = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("add commit");

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().remove_head(commit.id());
    tx.commit("remove commit");

    // If we load the repo at head, we should not see the commit since it was
    // removed
    let loader = RepoLoader::init(
        &settings,
        repo.repo_path(),
        &TestRepo::default_store_factories(),
    )
    .unwrap();
    let head_repo = loader.load_at_head(&settings).unwrap();
    assert!(!head_repo.view().heads().contains(commit.id()));

    // If we load the repo at the previous operation, we should see the commit since
    // it has not been removed yet
    let loader = RepoLoader::init(
        &settings,
        repo.repo_path(),
        &TestRepo::default_store_factories(),
    )
    .unwrap();
    let old_repo = loader.load_at(repo.operation()).unwrap();
    assert!(old_repo.view().heads().contains(commit.id()));
}
