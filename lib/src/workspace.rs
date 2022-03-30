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

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::op_store::WorkspaceId;
use crate::repo::{ReadonlyRepo, RepoLoader};
use crate::settings::UserSettings;
use crate::working_copy::WorkingCopy;

#[derive(Error, Debug, PartialEq)]
pub enum WorkspaceInitError {
    #[error("The destination repo ({0}) already exists")]
    DestinationExists(PathBuf),
}

#[derive(Error, Debug, PartialEq)]
pub enum WorkspaceLoadError {
    #[error("The repo appears to no longer be at {0}")]
    RepoDoesNotExist(PathBuf),
    #[error("There is no Jujutsu repo in {0}")]
    NoWorkspaceHere(PathBuf),
}

/// Represents a workspace, i.e. what's typically the .jj/ directory and its
/// parent.
pub struct Workspace {
    // Path to the workspace root (typically the parent of a .jj/ directory), which is where
    // working copy files live.
    workspace_root: PathBuf,
    repo_loader: RepoLoader,
    working_copy: WorkingCopy,
}

fn create_jj_dir(workspace_root: &Path) -> Result<PathBuf, WorkspaceInitError> {
    let jj_dir = workspace_root.join(".jj");
    if jj_dir.exists() {
        Err(WorkspaceInitError::DestinationExists(jj_dir))
    } else {
        std::fs::create_dir(&jj_dir).unwrap();
        Ok(jj_dir)
    }
}

fn init_working_copy(
    user_settings: &UserSettings,
    repo: &Arc<ReadonlyRepo>,
    workspace_root: &Path,
    jj_dir: &Path,
    workspace_id: WorkspaceId,
) -> (WorkingCopy, Arc<ReadonlyRepo>) {
    let working_copy_state_path = jj_dir.join("working_copy");
    std::fs::create_dir(&working_copy_state_path).unwrap();

    let mut tx = repo.start_transaction(&format!("add workspace '{}'", workspace_id.as_str()));
    tx.mut_repo().check_out(
        workspace_id.clone(),
        user_settings,
        &repo.store().root_commit(),
    );
    let repo = tx.commit();

    let working_copy = WorkingCopy::init(
        repo.store().clone(),
        workspace_root.to_path_buf(),
        working_copy_state_path,
        repo.op_id().clone(),
        workspace_id,
    );
    (working_copy, repo)
}

impl Workspace {
    pub fn init_local(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        let jj_dir = create_jj_dir(&workspace_root)?;
        let repo_dir = jj_dir.join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        let repo = ReadonlyRepo::init_local(user_settings, repo_dir);
        let (working_copy, repo) = init_working_copy(
            user_settings,
            &repo,
            &workspace_root,
            &jj_dir,
            WorkspaceId::default(),
        );
        let repo_loader = repo.loader();
        let workspace = Workspace {
            workspace_root,
            repo_loader,
            working_copy,
        };
        Ok((workspace, repo))
    }

    pub fn init_internal_git(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        let jj_dir = create_jj_dir(&workspace_root)?;
        let repo_dir = jj_dir.join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        let repo = ReadonlyRepo::init_internal_git(user_settings, repo_dir);
        let (working_copy, repo) = init_working_copy(
            user_settings,
            &repo,
            &workspace_root,
            &jj_dir,
            WorkspaceId::default(),
        );
        let repo_loader = repo.loader();
        let workspace = Workspace {
            workspace_root,
            repo_loader,
            working_copy,
        };
        Ok((workspace, repo))
    }

    pub fn init_external_git(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
        git_repo_path: PathBuf,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        let jj_dir = create_jj_dir(&workspace_root)?;
        let repo_dir = jj_dir.join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        let repo = ReadonlyRepo::init_external_git(user_settings, repo_dir, git_repo_path);
        let (working_copy, repo) = init_working_copy(
            user_settings,
            &repo,
            &workspace_root,
            &jj_dir,
            WorkspaceId::default(),
        );
        let repo_loader = repo.loader();
        let workspace = Workspace {
            workspace_root,
            repo_loader,
            working_copy,
        };
        Ok((workspace, repo))
    }

    pub fn init_workspace_with_existing_repo(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
        repo: &Arc<ReadonlyRepo>,
        workspace_id: WorkspaceId,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        let jj_dir = create_jj_dir(&workspace_root)?;

        let repo_dir = repo.repo_path().canonicalize().unwrap();
        let mut repo_file = File::create(jj_dir.join("repo")).unwrap();
        repo_file
            .write_all(repo_dir.to_str().unwrap().as_bytes())
            .unwrap();

        let repo_loader = RepoLoader::init(user_settings, repo_dir);
        let (working_copy, repo) =
            init_working_copy(user_settings, repo, &workspace_root, &jj_dir, workspace_id);
        let workspace = Workspace {
            workspace_root,
            repo_loader,
            working_copy,
        };
        Ok((workspace, repo))
    }

    pub fn load(
        user_settings: &UserSettings,
        workspace_path: PathBuf,
    ) -> Result<Self, WorkspaceLoadError> {
        let jj_dir = find_jj_dir(&workspace_path)
            .ok_or(WorkspaceLoadError::NoWorkspaceHere(workspace_path))?;
        let workspace_root = jj_dir.parent().unwrap().to_owned();
        let mut repo_dir = jj_dir.join("repo");
        if !repo_dir.exists() {
            // TODO: Delete this in mid 2022 or so
            println!("The repo format has changed. Moving repo into .jj/repo/");
            std::fs::create_dir(&repo_dir).unwrap();
            for dir in ["store", "op_store", "op_heads", "index"] {
                std::fs::rename(jj_dir.join(dir), repo_dir.join(dir)).unwrap();
            }
        }
        // If .jj/repo is a file, then we interpret its contents as a relative path to
        // the actual repo directory (typically in another workspace).
        if repo_dir.is_file() {
            let mut repo_file = File::open(repo_dir).unwrap();
            let mut buf = Vec::new();
            repo_file.read_to_end(&mut buf).unwrap();
            let repo_path_str = String::from_utf8(buf).unwrap();
            repo_dir = jj_dir.join(repo_path_str).canonicalize().unwrap();
            if !repo_dir.is_dir() {
                return Err(WorkspaceLoadError::RepoDoesNotExist(repo_dir));
            }
        }
        let repo_loader = RepoLoader::init(user_settings, repo_dir);
        let working_copy_state_path = jj_dir.join("working_copy");
        let working_copy = WorkingCopy::load(
            repo_loader.store().clone(),
            workspace_root.clone(),
            working_copy_state_path,
        );
        Ok(Self {
            workspace_root,
            repo_loader,
            working_copy,
        })
    }

    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    pub fn workspace_id(&self) -> WorkspaceId {
        self.working_copy.workspace_id()
    }

    pub fn repo_path(&self) -> &PathBuf {
        self.repo_loader.repo_path()
    }

    pub fn repo_loader(&self) -> &RepoLoader {
        &self.repo_loader
    }

    pub fn working_copy(&self) -> &WorkingCopy {
        &self.working_copy
    }

    pub fn working_copy_mut(&mut self) -> &mut WorkingCopy {
        &mut self.working_copy
    }
}

fn find_jj_dir(mut workspace_root: &Path) -> Option<PathBuf> {
    loop {
        let jj_path = workspace_root.join(".jj");
        if jj_path.is_dir() {
            return Some(jj_path);
        }
        if let Some(wc_dir_parent) = workspace_root.parent() {
            workspace_root = wc_dir_parent;
        } else {
            return None;
        }
    }
}
