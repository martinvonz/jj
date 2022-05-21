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

use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::matchers::EverythingMatcher;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::testutils;
use jujutsu_lib::testutils::{assert_rebased, CommitGraphBuilder, TestRepo};
use jujutsu_lib::tree::DiffSummary;
use test_case::test_case;

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_initial(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(&settings, use_git);
    let repo = &test_repo.repo;
    let store = repo.store();

    let root_file_path = RepoPath::from_internal_string("file");
    let dir_file_path = RepoPath::from_internal_string("dir/file");
    let tree = testutils::create_tree(
        repo,
        &[
            (&root_file_path, "file contents"),
            (&dir_file_path, "dir/file contents"),
        ],
    );

    let mut tx = repo.start_transaction("test");
    let commit = CommitBuilder::for_new_commit(&settings, store, tree.id().clone())
        .set_parents(vec![store.root_commit_id().clone()])
        .write_to_repo(tx.mut_repo());
    tx.commit();

    assert_eq!(commit.parents(), vec![store.root_commit()]);
    assert_eq!(commit.predecessors(), vec![]);
    assert!(!commit.is_open());
    assert_eq!(commit.description(), "");
    assert_eq!(commit.author().name, settings.user_name());
    assert_eq!(commit.author().email, settings.user_email());
    assert_eq!(commit.committer().name, settings.user_name());
    assert_eq!(commit.committer().email, settings.user_email());
    assert_eq!(
        store
            .root_commit()
            .tree()
            .diff_summary(&commit.tree(), &EverythingMatcher),
        DiffSummary {
            modified: vec![],
            added: vec![dir_file_path, root_file_path],
            removed: vec![]
        }
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_rewrite(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(&settings, use_git);
    let repo = &test_repo.repo;
    let store = repo.store().clone();

    let root_file_path = RepoPath::from_internal_string("file");
    let dir_file_path = RepoPath::from_internal_string("dir/file");
    let initial_tree = testutils::create_tree(
        repo,
        &[
            (&root_file_path, "file contents"),
            (&dir_file_path, "dir/file contents"),
        ],
    );

    let mut tx = repo.start_transaction("test");
    let initial_commit =
        CommitBuilder::for_new_commit(&settings, &store, initial_tree.id().clone())
            .set_parents(vec![store.root_commit_id().clone()])
            .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let rewritten_tree = testutils::create_tree(
        &repo,
        &[
            (&root_file_path, "file contents"),
            (&dir_file_path, "updated dir/file contents"),
        ],
    );

    let config = config::Config::builder()
        .set_override("user.name", "Rewrite User")
        .unwrap()
        .set_override("user.email", "rewrite.user@example.com")
        .unwrap()
        .build()
        .unwrap();
    let rewrite_settings = UserSettings::from_config(config);
    let mut tx = repo.start_transaction("test");
    let rewritten_commit =
        CommitBuilder::for_rewrite_from(&rewrite_settings, &store, &initial_commit)
            .set_tree(rewritten_tree.id().clone())
            .write_to_repo(tx.mut_repo());
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    tx.commit();
    assert_eq!(rewritten_commit.parents(), vec![store.root_commit()]);
    assert_eq!(
        rewritten_commit.predecessors(),
        vec![initial_commit.clone()]
    );
    assert!(!rewritten_commit.is_open());
    assert_eq!(rewritten_commit.author().name, settings.user_name());
    assert_eq!(rewritten_commit.author().email, settings.user_email());
    assert_eq!(
        rewritten_commit.committer().name,
        rewrite_settings.user_name()
    );
    assert_eq!(
        rewritten_commit.committer().email,
        rewrite_settings.user_email()
    );
    assert_eq!(
        store
            .root_commit()
            .tree()
            .diff_summary(&rewritten_commit.tree(), &EverythingMatcher),
        DiffSummary {
            modified: vec![],
            added: vec![dir_file_path.clone(), root_file_path],
            removed: vec![]
        }
    );
    assert_eq!(
        initial_commit
            .tree()
            .diff_summary(&rewritten_commit.tree(), &EverythingMatcher),
        DiffSummary {
            modified: vec![dir_file_path],
            added: vec![],
            removed: vec![]
        }
    );
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_commit_builder_descendants(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(&settings, use_git);
    let repo = &test_repo.repo;
    let store = repo.store().clone();

    let mut tx = repo.start_transaction("test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let repo = tx.commit();

    // Test with for_new_commit()
    let mut tx = repo.start_transaction("test");
    CommitBuilder::for_new_commit(&settings, &store, store.empty_tree_id().clone())
        .write_to_repo(tx.mut_repo());
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    assert!(rebaser.rebase_next().unwrap().is_none());

    // Test with for_open_commit()
    let mut tx = repo.start_transaction("test");
    CommitBuilder::for_open_commit(
        &settings,
        &store,
        commit2.id().clone(),
        store.empty_tree_id().clone(),
    )
    .write_to_repo(tx.mut_repo());
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    assert!(rebaser.rebase_next().unwrap().is_none());

    // Test with for_rewrite_from()
    let mut tx = repo.start_transaction("test");
    let commit4 =
        CommitBuilder::for_rewrite_from(&settings, &store, &commit2).write_to_repo(tx.mut_repo());
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    assert_rebased(rebaser.rebase_next().unwrap(), &commit3, &[&commit4]);
    assert!(rebaser.rebase_next().unwrap().is_none());

    // Test with for_rewrite_from() but new change id
    let mut tx = repo.start_transaction("test");
    CommitBuilder::for_rewrite_from(&settings, &store, &commit2)
        .generate_new_change_id()
        .write_to_repo(tx.mut_repo());
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    assert!(rebaser.rebase_next().unwrap().is_none());
}
