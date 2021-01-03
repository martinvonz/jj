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

use jujube_lib::repo::Repo;
use jujube_lib::store::CommitId;
use jujube_lib::testutils;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_heads_empty(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let heads = repo.view();
    let wc = repo.working_copy_locked();
    let all_heads: Vec<CommitId> = heads.heads().cloned().collect();
    assert_eq!(all_heads, vec![wc.current_commit_id()]);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_heads_fork(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let mut tx = repo.start_transaction("test");

    let initial = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write_to_transaction(&mut tx);
    let child1 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_transaction(&mut tx);
    let child2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_transaction(&mut tx);

    let heads = tx.as_repo().view();
    let wc = repo.working_copy_locked();
    let mut actual_all_heads: Vec<CommitId> = heads.heads().cloned().collect();
    actual_all_heads.sort();
    let mut expected_all_heads = vec![
        wc.current_commit_id(),
        child1.id().clone(),
        child2.id().clone(),
    ];
    expected_all_heads.sort();
    assert_eq!(actual_all_heads, expected_all_heads);
    drop(heads);
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_heads_merge(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let mut tx = repo.start_transaction("test");

    let initial = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write_to_transaction(&mut tx);
    let child1 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_transaction(&mut tx);
    let child2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_transaction(&mut tx);
    let merge = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![child1.id().clone(), child2.id().clone()])
        .write_to_transaction(&mut tx);

    let heads = tx.as_repo().view();
    let wc = repo.working_copy_locked();
    let mut actual_all_heads: Vec<CommitId> = heads.heads().cloned().collect();
    actual_all_heads.sort();
    let mut expected_all_heads = vec![wc.current_commit_id(), merge.id().clone()];
    expected_all_heads.sort();
    assert_eq!(actual_all_heads, expected_all_heads);
    drop(heads);
    tx.discard();
}
