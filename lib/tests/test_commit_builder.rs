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

use jj_lib::backend::{ChangeId, MillisSinceEpoch, Signature, Timestamp};
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merged_tree::DiffSummary;
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::settings::UserSettings;
use test_case::test_case;
use testutils::{assert_rebased_onto, create_tree, CommitGraphBuilder, TestRepo, TestRepoBackend};

fn to_owned_path_vec(paths: &[&RepoPath]) -> Vec<RepoPathBuf> {
    paths.iter().map(|&path| path.to_owned()).collect()
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_initial(backend: TestRepoBackend) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(backend);
    let repo = &test_repo.repo;
    let store = repo.store();

    let root_file_path = RepoPath::from_internal_string("file");
    let dir_file_path = RepoPath::from_internal_string("dir/file");
    let tree = create_tree(
        repo,
        &[
            (root_file_path, "file contents"),
            (dir_file_path, "dir/file contents"),
        ],
    );

    let mut tx = repo.start_transaction(&settings);
    let author_signature = Signature {
        name: "author name".to_string(),
        email: "author email".to_string(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(1000),
            tz_offset: 60,
        },
    };
    let committer_signature = Signature {
        name: "committer name".to_string(),
        email: "committer email".to_string(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(2000),
            tz_offset: -60,
        },
    };
    let change_id = ChangeId::new(vec![100u8; 16]);
    let builder = tx
        .mut_repo()
        .new_commit(&settings, vec![store.root_commit_id().clone()], tree.id())
        .set_change_id(change_id.clone())
        .set_description("description")
        .set_author(author_signature.clone())
        .set_committer(committer_signature.clone());
    assert_eq!(builder.parents(), &[store.root_commit_id().clone()]);
    assert_eq!(builder.predecessors(), &[]);
    assert_eq!(builder.tree_id(), &tree.id());
    assert_eq!(builder.change_id(), &change_id);
    assert_eq!(builder.author(), &author_signature);
    assert_eq!(builder.committer(), &committer_signature);
    let commit = builder.write().unwrap();
    tx.commit("test");

    assert_eq!(commit.parents(), vec![store.root_commit()]);
    assert_eq!(commit.predecessors(), vec![]);
    assert_eq!(commit.description(), "description");
    assert_eq!(commit.author(), &author_signature);
    assert_eq!(commit.committer(), &committer_signature);
    assert_eq!(
        store
            .root_commit()
            .tree()
            .unwrap()
            .diff_summary(&commit.tree().unwrap(), &EverythingMatcher)
            .unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[dir_file_path, root_file_path]),
            removed: vec![],
        }
    );
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_rewrite(backend: TestRepoBackend) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(backend);
    let repo = &test_repo.repo;
    let store = repo.store().clone();

    let root_file_path = RepoPath::from_internal_string("file");
    let dir_file_path = RepoPath::from_internal_string("dir/file");
    let initial_tree = create_tree(
        repo,
        &[
            (root_file_path, "file contents"),
            (dir_file_path, "dir/file contents"),
        ],
    );

    let mut tx = repo.start_transaction(&settings);
    let initial_commit = tx
        .mut_repo()
        .new_commit(
            &settings,
            vec![store.root_commit_id().clone()],
            initial_tree.id(),
        )
        .write()
        .unwrap();
    let repo = tx.commit("test");

    let rewritten_tree = create_tree(
        &repo,
        &[
            (root_file_path, "file contents"),
            (dir_file_path, "updated dir/file contents"),
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
    let mut tx = repo.start_transaction(&settings);
    let rewritten_commit = tx
        .mut_repo()
        .rewrite_commit(&rewrite_settings, &initial_commit)
        .set_tree_id(rewritten_tree.id().clone())
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    tx.commit("test");
    assert_eq!(rewritten_commit.parents(), vec![store.root_commit()]);
    assert_eq!(
        rewritten_commit.predecessors(),
        vec![initial_commit.clone()]
    );
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
            .unwrap()
            .diff_summary(&rewritten_commit.tree().unwrap(), &EverythingMatcher)
            .unwrap(),
        DiffSummary {
            modified: vec![],
            added: to_owned_path_vec(&[dir_file_path, root_file_path]),
            removed: vec![],
        }
    );
    assert_eq!(
        initial_commit
            .tree()
            .unwrap()
            .diff_summary(&rewritten_commit.tree().unwrap(), &EverythingMatcher)
            .unwrap(),
        DiffSummary {
            modified: to_owned_path_vec(&[dir_file_path]),
            added: vec![],
            removed: vec![],
        }
    );
}

// An author field with an empty name/email should get filled in on rewrite
#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_rewrite_update_missing_user(backend: TestRepoBackend) {
    let missing_user_settings =
        UserSettings::from_config(config::Config::builder().build().unwrap());
    let test_repo = TestRepo::init_with_backend(backend);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&missing_user_settings);
    let initial_commit = tx
        .mut_repo()
        .new_commit(
            &missing_user_settings,
            vec![repo.store().root_commit_id().clone()],
            repo.store().empty_merged_tree_id(),
        )
        .write()
        .unwrap();
    assert_eq!(initial_commit.author().name, "");
    assert_eq!(initial_commit.author().email, "");
    assert_eq!(initial_commit.committer().name, "");
    assert_eq!(initial_commit.committer().email, "");

    let config = config::Config::builder()
        .set_override("user.name", "Configured User")
        .unwrap()
        .set_override("user.email", "configured.user@example.com")
        .unwrap()
        .build()
        .unwrap();
    let settings = UserSettings::from_config(config);
    let rewritten_commit = tx
        .mut_repo()
        .rewrite_commit(&settings, &initial_commit)
        .write()
        .unwrap();

    assert_eq!(rewritten_commit.author().name, "Configured User");
    assert_eq!(
        rewritten_commit.author().email,
        "configured.user@example.com"
    );
    assert_eq!(rewritten_commit.committer().name, "Configured User");
    assert_eq!(
        rewritten_commit.committer().email,
        "configured.user@example.com"
    );
}

#[test_case(TestRepoBackend::Local ; "local backend")]
// #[test_case(TestRepoBackend::Git ; "git backend")]
fn test_commit_builder_descendants(backend: TestRepoBackend) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(backend);
    let repo = &test_repo.repo;
    let store = repo.store().clone();

    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let repo = tx.commit("test");

    // Test with for_new_commit()
    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo()
        .new_commit(
            &settings,
            vec![store.root_commit_id().clone()],
            store.empty_merged_tree_id(),
        )
        .write()
        .unwrap();
    let rebase_map = tx
        .mut_repo()
        .rebase_descendants_return_map(&settings)
        .unwrap();
    assert_eq!(rebase_map.len(), 0);

    // Test with for_rewrite_from()
    let mut tx = repo.start_transaction(&settings);
    let commit4 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit2)
        .write()
        .unwrap();
    let rebase_map = tx
        .mut_repo()
        .rebase_descendants_return_map(&settings)
        .unwrap();
    assert_rebased_onto(tx.mut_repo(), &rebase_map, &commit3, &[commit4.id()]);
    assert_eq!(rebase_map.len(), 1);

    // Test with for_rewrite_from() but new change id
    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo()
        .rewrite_commit(&settings, &commit2)
        .generate_new_change_id()
        .write()
        .unwrap();
    let rebase_map = tx
        .mut_repo()
        .rebase_descendants_return_map(&settings)
        .unwrap();
    assert!(rebase_map.is_empty());
}
