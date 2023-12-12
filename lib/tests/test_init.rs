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

use std::path::{Path, PathBuf};

use jj_lib::git_backend::GitBackend;
use jj_lib::op_store::WorkspaceId;
use jj_lib::repo::Repo;
use jj_lib::settings::UserSettings;
use jj_lib::workspace::Workspace;
use test_case::test_case;
use testutils::{write_random_commit, TestRepoBackend, TestWorkspace};

fn canonicalize(input: &Path) -> (PathBuf, PathBuf) {
    let uncanonical = input.join("..").join(input.file_name().unwrap());
    let canonical = uncanonical.canonicalize().unwrap();
    (canonical, uncanonical)
}

#[test]
fn test_init_local() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let (canonical, uncanonical) = canonicalize(temp_dir.path());
    let (workspace, repo) = Workspace::init_local(&settings, &uncanonical).unwrap();
    assert!(repo
        .store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .is_none());
    assert_eq!(repo.repo_path(), &canonical.join(".jj").join("repo"));
    assert_eq!(workspace.workspace_root(), &canonical);

    // Just test that we can write a commit to the store
    let mut tx = repo.start_transaction(&settings);
    write_random_commit(tx.mut_repo(), &settings);
}

#[test]
fn test_init_internal_git() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let (canonical, uncanonical) = canonicalize(temp_dir.path());
    let (workspace, repo) = Workspace::init_internal_git(&settings, &uncanonical).unwrap();
    let git_backend = repo
        .store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .unwrap();
    assert_eq!(repo.repo_path(), &canonical.join(".jj").join("repo"));
    assert_eq!(workspace.workspace_root(), &canonical);
    assert_eq!(
        git_backend.git_repo_path(),
        canonical.join(PathBuf::from_iter([".jj", "repo", "store", "git"])),
    );
    assert!(git_backend.git_workdir().is_none());
    assert_eq!(
        std::fs::read_to_string(repo.repo_path().join("store").join("git_target")).unwrap(),
        "git"
    );

    // Just test that we can write a commit to the store
    let mut tx = repo.start_transaction(&settings);
    write_random_commit(tx.mut_repo(), &settings);
}

#[test]
fn test_init_colocated_git() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let (canonical, uncanonical) = canonicalize(temp_dir.path());
    let (workspace, repo) = Workspace::init_colocated_git(&settings, &uncanonical).unwrap();
    let git_backend = repo
        .store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .unwrap();
    assert_eq!(repo.repo_path(), &canonical.join(".jj").join("repo"));
    assert_eq!(workspace.workspace_root(), &canonical);
    assert_eq!(git_backend.git_repo_path(), canonical.join(".git"));
    assert_eq!(git_backend.git_workdir(), Some(canonical.as_ref()));
    assert_eq!(
        std::fs::read_to_string(repo.repo_path().join("store").join("git_target")).unwrap(),
        "../../../.git"
    );

    // Just test that we can write a commit to the store
    let mut tx = repo.start_transaction(&settings);
    write_random_commit(tx.mut_repo(), &settings);
}

#[test]
fn test_init_external_git() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let (canonical, uncanonical) = canonicalize(temp_dir.path());
    let git_repo_path = uncanonical.join("git");
    git2::Repository::init(&git_repo_path).unwrap();
    std::fs::create_dir(uncanonical.join("jj")).unwrap();
    let (workspace, repo) = Workspace::init_external_git(
        &settings,
        &uncanonical.join("jj"),
        &git_repo_path.join(".git"),
    )
    .unwrap();
    let git_backend = repo
        .store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .unwrap();
    assert_eq!(
        repo.repo_path(),
        &canonical.join("jj").join(".jj").join("repo")
    );
    assert_eq!(workspace.workspace_root(), &canonical.join("jj"));
    assert_eq!(
        git_backend.git_repo_path(),
        canonical.join("git").join(".git")
    );
    assert_eq!(
        git_backend.git_workdir(),
        Some(canonical.join("git").as_ref())
    );

    // Just test that we can write a commit to the store
    let mut tx = repo.start_transaction(&settings);
    write_random_commit(tx.mut_repo(), &settings);
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_init_no_config_set(backend: TestRepoBackend) {
    // Test that we can create a repo without setting any config
    let settings = UserSettings::from_config(config::Config::default());
    let test_workspace = TestWorkspace::init_with_backend(&settings, backend);
    let repo = &test_workspace.repo;
    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(&WorkspaceId::default())
        .unwrap();
    let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();
    assert_eq!(wc_commit.author().name, "".to_string());
    assert_eq!(wc_commit.author().email, "".to_string());
    assert_eq!(wc_commit.committer().name, "".to_string());
    assert_eq!(wc_commit.committer().email, "".to_string());
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_init_checkout(backend: TestRepoBackend) {
    // Test the contents of the working-copy commit after init
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init_with_backend(&settings, backend);
    let repo = &test_workspace.repo;
    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(&WorkspaceId::default())
        .unwrap();
    let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();
    assert_eq!(*wc_commit.tree_id(), repo.store().empty_merged_tree_id());
    assert_eq!(
        wc_commit.store_commit().parents,
        vec![repo.store().root_commit_id().clone()]
    );
    assert_eq!(wc_commit.predecessors(), vec![]);
    assert_eq!(wc_commit.description(), "");
    assert_eq!(wc_commit.author().name, settings.user_name());
    assert_eq!(wc_commit.author().email, settings.user_email());
    assert_eq!(wc_commit.committer().name, settings.user_name());
    assert_eq!(wc_commit.committer().email, settings.user_email());
}
