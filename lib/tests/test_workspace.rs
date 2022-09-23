// Copyright 2021 Google LLC
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

use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::testutils;
use jujutsu_lib::testutils::TestWorkspace;
use jujutsu_lib::workspace::{Workspace, WorkspaceLoadError};
use test_case::test_case;

#[test]
fn test_load_bad_path() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let workspace_root = temp_dir.path().to_owned();
    // We haven't created a repo in the workspace_root, so it should fail to load.
    let result = Workspace::load(&settings, &workspace_root);
    assert_eq!(
        result.err(),
        Some(WorkspaceLoadError::NoWorkspaceHere(workspace_root))
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_load_from_subdir(use_git: bool) {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings, use_git);
    let workspace = &test_workspace.workspace;

    let subdir = workspace.workspace_root().join("dir").join("subdir");
    std::fs::create_dir_all(subdir.clone()).unwrap();
    let same_workspace = Workspace::load(&settings, &subdir);
    assert!(same_workspace.is_ok());
    let same_workspace = same_workspace.unwrap();
    assert_eq!(same_workspace.repo_path(), workspace.repo_path());
    assert_eq!(same_workspace.workspace_root(), workspace.workspace_root());
}

#[test_case(false ; "local backend")]
// #[test_case(true ; "git backend")]
fn test_init_additional_workspace(use_git: bool) {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings, use_git);
    let workspace = &test_workspace.workspace;

    let ws2_id = WorkspaceId::new("ws2".to_string());
    let ws2_root = test_workspace.root_dir().join("ws2_root");
    std::fs::create_dir(&ws2_root).unwrap();
    let (ws2, repo) = Workspace::init_workspace_with_existing_repo(
        &settings,
        &ws2_root,
        &test_workspace.repo,
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
    assert_eq!(ws2.workspace_id(), ws2_id);
    assert_eq!(
        *ws2.repo_path(),
        workspace.repo_path().canonicalize().unwrap()
    );
    assert_eq!(*ws2.workspace_root(), ws2_root.canonicalize().unwrap());
    let same_workspace = Workspace::load(&settings, &ws2_root);
    assert!(same_workspace.is_ok());
    let same_workspace = same_workspace.unwrap();
    assert_eq!(same_workspace.workspace_id(), ws2_id);
    assert_eq!(
        *same_workspace.repo_path(),
        workspace.repo_path().canonicalize().unwrap()
    );
    assert_eq!(same_workspace.workspace_root(), ws2.workspace_root());
}
