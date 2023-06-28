// Copyright 2021 The Jujutsu Authors
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

use jj_lib::repo::{RepoLoader, StoreFactories};
use test_case::test_case;
use testutils::{write_random_commit, TestRepo};

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_load_at_operation(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "add commit");
    let commit = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit();

    let mut tx = repo.start_transaction(&settings, "remove commit");
    tx.mut_repo().remove_head(commit.id());
    tx.commit();

    // If we load the repo at head, we should not see the commit since it was
    // removed
    let loader = RepoLoader::init(&settings, repo.repo_path(), &StoreFactories::default()).unwrap();
    let head_repo = loader.load_at_head(&settings).unwrap();
    assert!(!head_repo.view().heads().contains(commit.id()));

    // If we load the repo at the previous operation, we should see the commit since
    // it has not been removed yet
    let loader = RepoLoader::init(&settings, repo.repo_path(), &StoreFactories::default()).unwrap();
    let old_repo = loader.load_at(repo.operation());
    assert!(old_repo.view().heads().contains(commit.id()));
}
