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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::repo::{ReadonlyRepo, RepoInitError, RepoLoader};
use crate::settings::UserSettings;
use crate::working_copy::WorkingCopy;

#[derive(Error, Debug, PartialEq)]
pub enum WorkspaceLoadError {
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

impl Workspace {
    pub fn init_local(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
    ) -> Result<(Self, Arc<ReadonlyRepo>), RepoInitError> {
        let repo = ReadonlyRepo::init_local(user_settings, workspace_root.clone())?;
        let repo_loader = repo.loader();
        let workspace = Self::from_repo_loader(workspace_root, repo_loader);
        Ok((workspace, repo))
    }

    pub fn init_internal_git(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
    ) -> Result<(Self, Arc<ReadonlyRepo>), RepoInitError> {
        let repo = ReadonlyRepo::init_internal_git(user_settings, workspace_root.clone())?;
        let repo_loader = repo.loader();
        let workspace = Self::from_repo_loader(workspace_root, repo_loader);
        Ok((workspace, repo))
    }

    pub fn init_external_git(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
        git_repo_path: PathBuf,
    ) -> Result<(Self, Arc<ReadonlyRepo>), RepoInitError> {
        let repo =
            ReadonlyRepo::init_external_git(user_settings, workspace_root.clone(), git_repo_path)?;
        let repo_loader = repo.loader();
        let workspace = Self::from_repo_loader(workspace_root, repo_loader);
        Ok((workspace, repo))
    }

    pub fn load(
        user_settings: &UserSettings,
        workspace_path: PathBuf,
    ) -> Result<Self, WorkspaceLoadError> {
        let repo_path = find_repo_dir(&workspace_path)
            .ok_or(WorkspaceLoadError::NoWorkspaceHere(workspace_path))?;
        let workspace_root = repo_path.parent().unwrap().to_owned();
        let repo_loader = RepoLoader::init(user_settings, repo_path);
        Ok(Self::from_repo_loader(workspace_root, repo_loader))
    }

    fn from_repo_loader(workspace_root: PathBuf, repo_loader: RepoLoader) -> Self {
        let working_copy_state_path = repo_loader.repo_path().join("working_copy");
        let working_copy = WorkingCopy::load(
            repo_loader.store().clone(),
            workspace_root.clone(),
            working_copy_state_path,
        );
        Self {
            workspace_root,
            repo_loader,
            working_copy,
        }
    }

    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
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

fn find_repo_dir(mut workspace_root: &Path) -> Option<PathBuf> {
    loop {
        let repo_path = workspace_root.join(".jj");
        if repo_path.is_dir() {
            return Some(repo_path);
        }
        if let Some(wc_dir_parent) = workspace_root.parent() {
            workspace_root = wc_dir_parent;
        } else {
            return None;
        }
    }
}
