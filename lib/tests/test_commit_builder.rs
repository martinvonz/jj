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

use futures::StreamExt as _;
use itertools::Itertools;
use jj_lib::backend::ChangeId;
use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Signature;
use jj_lib::backend::Timestamp;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::settings::UserSettings;
use pollster::FutureExt as _;
use test_case::test_case;
use testutils::assert_rebased_onto;
use testutils::create_tree;
use testutils::CommitGraphBuilder;
use testutils::TestRepo;
use testutils::TestRepoBackend;

fn diff_paths(from_tree: &MergedTree, to_tree: &MergedTree) -> Vec<RepoPathBuf> {
    from_tree
        .diff_stream(to_tree, &EverythingMatcher)
        .map(|diff| {
            let _ = diff.values.unwrap();
            diff.path
        })
        .collect()
        .block_on()
}

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
        .repo_mut()
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

    let parents: Vec<_> = commit.parents().try_collect().unwrap();
    assert_eq!(parents, vec![store.root_commit()]);
    assert!(commit.predecessors().next().is_none());
    assert_eq!(commit.description(), "description");
    assert_eq!(commit.author(), &author_signature);
    assert_eq!(commit.committer(), &committer_signature);
    assert_eq!(
        diff_paths(
            &store.root_commit().tree().unwrap(),
            &commit.tree().unwrap(),
        ),
        to_owned_path_vec(&[dir_file_path, root_file_path]),
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
        .repo_mut()
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
        .repo_mut()
        .rewrite_commit(&rewrite_settings, &initial_commit)
        .set_tree_id(rewritten_tree.id().clone())
        .write()
        .unwrap();
    tx.repo_mut().rebase_descendants(&settings).unwrap();
    tx.commit("test");
    let parents: Vec<_> = rewritten_commit.parents().try_collect().unwrap();
    assert_eq!(parents, vec![store.root_commit()]);
    let predecessors: Vec<_> = rewritten_commit.predecessors().try_collect().unwrap();
    assert_eq!(predecessors, vec![initial_commit.clone()]);
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
        diff_paths(
            &store.root_commit().tree().unwrap(),
            &rewritten_commit.tree().unwrap(),
        ),
        to_owned_path_vec(&[dir_file_path, root_file_path]),
    );
    assert_eq!(
        diff_paths(
            &initial_commit.tree().unwrap(),
            &rewritten_commit.tree().unwrap(),
        ),
        to_owned_path_vec(&[dir_file_path]),
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
        .repo_mut()
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
        .repo_mut()
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
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_rewrite_resets_author_timestamp(backend: TestRepoBackend) {
    let test_repo = TestRepo::init_with_backend(backend);
    let repo = &test_repo.repo;

    // Create discardable commit
    let initial_timestamp = "2001-02-03T04:05:06+07:00";
    let config = testutils::base_config()
        .set_override("debug.commit-timestamp", initial_timestamp)
        .unwrap()
        .build()
        .unwrap();
    let settings = UserSettings::from_config(config);
    let mut tx = repo.start_transaction(&settings);
    let initial_commit = tx
        .repo_mut()
        .new_commit(
            &settings,
            vec![repo.store().root_commit_id().clone()],
            repo.store().empty_merged_tree_id(),
        )
        .write()
        .unwrap();

    let initial_timestamp =
        Timestamp::from_datetime(chrono::DateTime::parse_from_rfc3339(initial_timestamp).unwrap());
    assert_eq!(initial_commit.author().timestamp, initial_timestamp);
    assert_eq!(initial_commit.committer().timestamp, initial_timestamp);

    // Rewrite discardable commit to no longer be discardable
    let new_timestamp_1 = "2002-03-04T05:06:07+08:00";
    let config = testutils::base_config()
        .set_override("debug.commit-timestamp", new_timestamp_1)
        .unwrap()
        .build()
        .unwrap();
    let settings = UserSettings::from_config(config);
    let rewritten_commit_1 = tx
        .repo_mut()
        .rewrite_commit(&settings, &initial_commit)
        .set_description("No longer discardable")
        .write()
        .unwrap();

    let new_timestamp_1 =
        Timestamp::from_datetime(chrono::DateTime::parse_from_rfc3339(new_timestamp_1).unwrap());
    assert_ne!(new_timestamp_1, initial_timestamp);

    assert_eq!(rewritten_commit_1.author().timestamp, new_timestamp_1);
    assert_eq!(rewritten_commit_1.committer().timestamp, new_timestamp_1);
    assert_eq!(rewritten_commit_1.author(), rewritten_commit_1.committer());

    // Rewrite non-discardable commit
    let new_timestamp_2 = "2003-04-05T06:07:08+09:00";
    let config = testutils::base_config()
        .set_override("debug.commit-timestamp", new_timestamp_2)
        .unwrap()
        .build()
        .unwrap();
    let settings = UserSettings::from_config(config);
    let rewritten_commit_2 = tx
        .repo_mut()
        .rewrite_commit(&settings, &rewritten_commit_1)
        .set_description("New description")
        .write()
        .unwrap();

    let new_timestamp_2 =
        Timestamp::from_datetime(chrono::DateTime::parse_from_rfc3339(new_timestamp_2).unwrap());
    assert_ne!(new_timestamp_2, new_timestamp_1);

    assert_eq!(rewritten_commit_2.author().timestamp, new_timestamp_1);
    assert_eq!(rewritten_commit_2.committer().timestamp, new_timestamp_2);
}

#[test_case(TestRepoBackend::Local ; "local backend")]
// #[test_case(TestRepoBackend::Git ; "git backend")]
fn test_commit_builder_descendants(backend: TestRepoBackend) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(backend);
    let repo = &test_repo.repo;
    let store = repo.store().clone();

    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.repo_mut());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let repo = tx.commit("test");

    // Test with for_new_commit()
    let mut tx = repo.start_transaction(&settings);
    tx.repo_mut()
        .new_commit(
            &settings,
            vec![store.root_commit_id().clone()],
            store.empty_merged_tree_id(),
        )
        .write()
        .unwrap();
    let rebase_map = tx
        .repo_mut()
        .rebase_descendants_return_map(&settings)
        .unwrap();
    assert_eq!(rebase_map.len(), 0);

    // Test with for_rewrite_from()
    let mut tx = repo.start_transaction(&settings);
    let commit4 = tx
        .repo_mut()
        .rewrite_commit(&settings, &commit2)
        .write()
        .unwrap();
    let rebase_map = tx
        .repo_mut()
        .rebase_descendants_return_map(&settings)
        .unwrap();
    assert_rebased_onto(tx.repo_mut(), &rebase_map, &commit3, &[commit4.id()]);
    assert_eq!(rebase_map.len(), 1);

    // Test with for_rewrite_from() but new change id
    let mut tx = repo.start_transaction(&settings);
    tx.repo_mut()
        .rewrite_commit(&settings, &commit2)
        .generate_new_change_id()
        .write()
        .unwrap();
    let rebase_map = tx
        .repo_mut()
        .rebase_descendants_return_map(&settings)
        .unwrap();
    assert!(rebase_map.is_empty());
}
