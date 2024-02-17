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

use std::thread;

use assert_matches::assert_matches;
use jj_lib::op_store::WorkspaceId;
use jj_lib::repo::Repo;
use jj_lib::workspace::{
    default_working_copy_factories, default_working_copy_factory, Workspace, WorkspaceLoadError,
};
use testutils::{TestRepo, TestWorkspace};

#[test]
fn test_load_bad_path() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let workspace_root = temp_dir.path().to_owned();
    // We haven't created a repo in the workspace_root, so it should fail to load.
    let result = Workspace::load(
        &settings,
        &workspace_root,
        &TestRepo::default_store_factories(),
        &default_working_copy_factories(),
    );
    assert_matches!(
        result.err(),
        Some(WorkspaceLoadError::NoWorkspaceHere(root)) if root == workspace_root
    );
}

#[test]
fn test_init_additional_workspace() {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings);
    let workspace = &test_workspace.workspace;

    let ws2_id = WorkspaceId::new("ws2".to_string());
    let ws2_root = test_workspace.root_dir().join("ws2_root");
    std::fs::create_dir(&ws2_root).unwrap();
    let (ws2, repo) = Workspace::init_workspace_with_existing_repo(
        &settings,
        &ws2_root,
        &test_workspace.repo,
        &*default_working_copy_factory(),
        ws2_id.clone(),
    )
    .unwrap();
    let wc_commit_id = repo.view().get_wc_commit_id(&ws2_id);
    assert_ne!(wc_commit_id, None);
    let wc_commit_id = wc_commit_id.unwrap();
    let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();
    assert_eq!(
        wc_commit.parent_ids(),
        vec![repo.store().root_commit_id().clone()]
    );
    assert_eq!(ws2.workspace_id(), &ws2_id);
    assert_eq!(
        *ws2.repo_path(),
        workspace.repo_path().canonicalize().unwrap()
    );
    assert_eq!(*ws2.workspace_root(), ws2_root.canonicalize().unwrap());
    let same_workspace = Workspace::load(
        &settings,
        &ws2_root,
        &TestRepo::default_store_factories(),
        &default_working_copy_factories(),
    );
    assert!(same_workspace.is_ok());
    let same_workspace = same_workspace.unwrap();
    assert_eq!(same_workspace.workspace_id(), &ws2_id);
    assert_eq!(
        *same_workspace.repo_path(),
        workspace.repo_path().canonicalize().unwrap()
    );
    assert_eq!(same_workspace.workspace_root(), ws2.workspace_root());
}

/// Test cross-thread access to a workspace, which requires it to be Send
#[test]
fn test_sendable() {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings);
    let root = test_workspace.workspace.workspace_root().clone();

    thread::spawn(move || {
        let shared_workspace = test_workspace.workspace;
        assert_eq!(shared_workspace.workspace_root(), &root);
    })
    .join()
    .unwrap();
}
