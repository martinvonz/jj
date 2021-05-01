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
use jujube_lib::testutils::CommitGraphBuilder;
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

    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let initial = graph_builder.initial_commit();
    let child1 = graph_builder.commit_with_parents(&[&initial]);
    let child2 = graph_builder.commit_with_parents(&[&initial]);

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

    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let initial = graph_builder.initial_commit();
    let child1 = graph_builder.commit_with_parents(&[&initial]);
    let child2 = graph_builder.commit_with_parents(&[&initial]);
    let merge = graph_builder.commit_with_parents(&[&child1, &child2]);

    let wc = repo.working_copy_locked();
    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {wc.current_commit_id(), merge.id().clone()}
    );
    tx.discard();
}
