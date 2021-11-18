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

use std::path::PathBuf;

use crate::repo::{RepoLoadError, RepoLoader};
use crate::settings::UserSettings;

/// Represents a workspace, i.e. what's typically the .jj/ directory and its
/// parent.
pub struct Workspace {
    // Path to the workspace root (typically the parent of a .jj/ directory), which is where
    // working copy files live.
    workspace_root: PathBuf,
    repo_loader: RepoLoader,
}

impl Workspace {
    pub fn load(
        user_settings: &UserSettings,
        workspace_root: PathBuf,
    ) -> Result<Self, RepoLoadError> {
        // TODO: Move the find_repo_dir() call from RepoLoader::init() to here
        let repo_loader = RepoLoader::init(user_settings, workspace_root)?;
        let workspace_root = repo_loader.working_copy_path().clone();
        Ok(Self {
            workspace_root,
            repo_loader,
        })
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
}
