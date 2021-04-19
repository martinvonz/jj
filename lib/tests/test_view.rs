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

use jujube_lib::testutils;
use maplit::hashset;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_heads_empty(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let wc = repo.working_copy_locked();
    assert_eq!(*repo.view().heads(), hashset! {wc.current_commit_id()});
    assert_eq!(
        *repo.view().public_heads(),
        hashset! {repo.store().root_commit_id().clone()}
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_heads_fork(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let mut tx = repo.start_transaction("test");

    let initial = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![repo.store().root_commit_id().clone()])
        .write_to_repo(tx.mut_repo());
    let child1 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_repo(tx.mut_repo());
    let child2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_repo(tx.mut_repo());

    let wc = repo.working_copy_locked();
    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            wc.current_commit_id(),
            child1.id().clone(),
            child2.id().clone(),
        }
    );
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
        .write_to_repo(tx.mut_repo());
    let child1 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_repo(tx.mut_repo());
    let child2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![initial.id().clone()])
        .write_to_repo(tx.mut_repo());
    let merge = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![child1.id().clone(), child2.id().clone()])
        .write_to_repo(tx.mut_repo());

    let wc = repo.working_copy_locked();
    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {wc.current_commit_id(), merge.id().clone()}
    );
    tx.discard();
}
