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
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::testutils;
use jujutsu_lib::tree::DiffSummary;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_initial(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let root_file_path = RepoPath::from("file");
    let dir_file_path = RepoPath::from("dir/file");
    let tree = testutils::create_tree(
        &repo,
        &[
            (&root_file_path, "file contents"),
            (&dir_file_path, "dir/file contents"),
        ],
    );

    let commit = CommitBuilder::for_new_commit(&settings, store, tree.id().clone())
        .set_parents(vec![store.root_commit_id().clone()])
        .write_to_new_transaction(&repo, "test");

    assert_eq!(commit.parents(), vec![store.root_commit()]);
    assert_eq!(commit.predecessors(), vec![]);
    assert!(!commit.is_open());
    assert_eq!(commit.description(), "");
    assert_eq!(commit.author().name, settings.user_name());
    assert_eq!(commit.author().email, settings.user_email());
    assert_eq!(commit.committer().name, settings.user_name());
    assert_eq!(commit.committer().email, settings.user_email());
    assert_eq!(
        store.root_commit().tree().diff_summary(&commit.tree()),
        DiffSummary {
            modified: vec![],
            added: vec![root_file_path, dir_file_path],
            removed: vec![]
        }
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_rewrite(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store().clone();

    let root_file_path = RepoPath::from("file");
    let dir_file_path = RepoPath::from("dir/file");
    let initial_tree = testutils::create_tree(
        &repo,
        &[
            (&root_file_path, "file contents"),
            (&dir_file_path, "dir/file contents"),
        ],
    );

    let initial_commit =
        CommitBuilder::for_new_commit(&settings, &store, initial_tree.id().clone())
            .set_parents(vec![store.root_commit_id().clone()])
            .write_to_new_transaction(&repo, "test");
    let repo = repo.reload();

    let rewritten_tree = testutils::create_tree(
        &repo,
        &[
            (&root_file_path, "file contents"),
            (&dir_file_path, "updated dir/file contents"),
        ],
    );

    let mut config = config::Config::new();
    config.set("user.name", "Rewrite User").unwrap();
    config
        .set("user.email", "rewrite.user@example.com")
        .unwrap();
    let rewrite_settings = UserSettings::from_config(config);
    let rewritten_commit =
        CommitBuilder::for_rewrite_from(&rewrite_settings, &store, &initial_commit)
            .set_tree(rewritten_tree.id().clone())
            .write_to_new_transaction(&repo, "test");
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
            .diff_summary(&rewritten_commit.tree()),
        DiffSummary {
            modified: vec![],
            added: vec![root_file_path, dir_file_path.clone()],
            removed: vec![]
        }
    );
    assert_eq!(
        initial_commit.tree().diff_summary(&rewritten_commit.tree()),
        DiffSummary {
            modified: vec![dir_file_path],
            added: vec![],
            removed: vec![]
        }
    );
}
