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

#![allow(missing_docs)]

use std::fs;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::backend::{Backend, BackendInitError, MergedTreeId};
use crate::file_util::{self, IoResultExt as _, PathError};
use crate::git_backend::GitBackend;
use crate::index::IndexStore;
use crate::local_backend::LocalBackend;
use crate::local_working_copy::{
    CheckoutError, CheckoutStats, LocalWorkingCopy, LockedLocalWorkingCopy, WorkingCopyStateError,
};
use crate::merged_tree::MergedTree;
use crate::op_heads_store::OpHeadsStore;
use crate::op_store::{OpStore, OperationId, WorkspaceId};
use crate::repo::{
    CheckOutCommitError, ReadonlyRepo, Repo, RepoInitError, RepoLoader, StoreFactories,
    StoreLoadError,
};
use crate::settings::UserSettings;
use crate::submodule_store::SubmoduleStore;
use crate::working_copy::{LockedWorkingCopy, WorkingCopy};

#[derive(Error, Debug)]
pub enum WorkspaceInitError {
    #[error("The destination repo ({0}) already exists")]
    DestinationExists(PathBuf),
    #[error("Repo path could not be interpreted as Unicode text")]
    NonUnicodePath,
    #[error(transparent)]
    CheckOutCommit(#[from] CheckOutCommitError),
    #[error(transparent)]
    WorkingCopyState(#[from] WorkingCopyStateError),
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    Backend(#[from] BackendInitError),
}

#[derive(Error, Debug)]
pub enum WorkspaceLoadError {
    #[error("The repo appears to no longer be at {0}")]
    RepoDoesNotExist(PathBuf),
    #[error("There is no Jujutsu repo in {0}")]
    NoWorkspaceHere(PathBuf),
    #[error("Cannot read the repo: {0}")]
    StoreLoadError(#[from] StoreLoadError),
    #[error("Repo path could not be interpreted as Unicode text")]
    NonUnicodePath,
    #[error(transparent)]
    Path(#[from] PathError),
}

/// Represents a workspace, i.e. what's typically the .jj/ directory and its
/// parent.
pub struct Workspace {
    // Path to the workspace root (typically the parent of a .jj/ directory), which is where
    // working copy files live.
    workspace_root: PathBuf,
    repo_loader: RepoLoader,
    working_copy: LocalWorkingCopy,
}

fn create_jj_dir(workspace_root: &Path) -> Result<PathBuf, WorkspaceInitError> {
    let jj_dir = workspace_root.join(".jj");
    match std::fs::create_dir(&jj_dir).context(&jj_dir) {
        Ok(()) => Ok(jj_dir),
        Err(ref e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
            Err(WorkspaceInitError::DestinationExists(jj_dir))
        }
        Err(e) => Err(e.into()),
    }
}

fn init_working_copy(
    user_settings: &UserSettings,
    repo: &Arc<ReadonlyRepo>,
    workspace_root: &Path,
    jj_dir: &Path,
    workspace_id: WorkspaceId,
) -> Result<(LocalWorkingCopy, Arc<ReadonlyRepo>), WorkspaceInitError> {
    let working_copy_state_path = jj_dir.join("working_copy");
    std::fs::create_dir(&working_copy_state_path).context(&working_copy_state_path)?;

    let mut tx = repo.start_transaction(
        user_settings,
        &format!("add workspace '{}'", workspace_id.as_str()),
    );
    tx.mut_repo().check_out(
        workspace_id.clone(),
        user_settings,
        &repo.store().root_commit(),
    )?;
    let repo = tx.commit();

    let working_copy = LocalWorkingCopy::init(
        repo.store().clone(),
        workspace_root.to_path_buf(),
        working_copy_state_path,
        repo.op_id().clone(),
        workspace_id,
    )?;
    Ok((working_copy, repo))
}

impl Workspace {
    fn new(
        workspace_root: &Path,
        working_copy: LocalWorkingCopy,
        repo_loader: RepoLoader,
    ) -> Result<Workspace, PathError> {
        let workspace_root = workspace_root.canonicalize().context(workspace_root)?;
        Ok(Workspace {
            workspace_root,
            repo_loader,
            working_copy,
        })
    }

    pub fn init_local(
        user_settings: &UserSettings,
        workspace_root: &Path,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        Self::init_with_backend(user_settings, workspace_root, |store_path| {
            Ok(Box::new(LocalBackend::init(store_path)))
        })
    }

    /// Initializes a workspace with a new Git backend in .jj/git/ (bare Git
    /// repo)
    pub fn init_internal_git(
        user_settings: &UserSettings,
        workspace_root: &Path,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        Self::init_with_backend(user_settings, workspace_root, |store_path| {
            Ok(Box::new(GitBackend::init_internal(store_path)?))
        })
    }

    /// Initializes a workspace with an existing Git backend at the specified
    /// path
    pub fn init_external_git(
        user_settings: &UserSettings,
        workspace_root: &Path,
        git_repo_path: &Path,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        Self::init_with_backend(user_settings, workspace_root, |store_path| {
            // If the git repo is inside the workspace, use a relative path to it so the
            // whole workspace can be moved without breaking.
            // TODO: Clean up path normalization. store_path is canonicalized by
            // ReadonlyRepo::init(). workspace_root will be canonicalized by
            // Workspace::new(), but it's not yet here.
            let store_relative_git_repo_path =
                match (workspace_root.canonicalize(), git_repo_path.canonicalize()) {
                    (Ok(workspace_root), Ok(git_repo_path))
                        if git_repo_path.starts_with(&workspace_root) =>
                    {
                        file_util::relative_path(store_path, &git_repo_path)
                    }
                    _ => git_repo_path.to_owned(),
                };
            Ok(Box::new(GitBackend::init_external(
                store_path,
                &store_relative_git_repo_path,
            )?))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn init_with_factories(
        user_settings: &UserSettings,
        workspace_root: &Path,
        backend_factory: impl FnOnce(&Path) -> Result<Box<dyn Backend>, BackendInitError>,
        op_store_factory: impl FnOnce(&Path) -> Box<dyn OpStore>,
        op_heads_store_factory: impl FnOnce(&Path) -> Box<dyn OpHeadsStore>,
        index_store_factory: impl FnOnce(&Path) -> Box<dyn IndexStore>,
        submodule_store_factory: impl FnOnce(&Path) -> Box<dyn SubmoduleStore>,
        workspace_id: WorkspaceId,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        let jj_dir = create_jj_dir(workspace_root)?;
        (|| {
            let repo_dir = jj_dir.join("repo");
            std::fs::create_dir(&repo_dir).context(&repo_dir)?;
            let repo = ReadonlyRepo::init(
                user_settings,
                &repo_dir,
                backend_factory,
                op_store_factory,
                op_heads_store_factory,
                index_store_factory,
                submodule_store_factory,
            )
            .map_err(|repo_init_err| match repo_init_err {
                RepoInitError::Backend(err) => WorkspaceInitError::Backend(err),
                RepoInitError::Path(err) => WorkspaceInitError::Path(err),
            })?;
            let (working_copy, repo) =
                init_working_copy(user_settings, &repo, workspace_root, &jj_dir, workspace_id)?;
            let repo_loader = repo.loader();
            let workspace = Workspace::new(workspace_root, working_copy, repo_loader)?;
            Ok((workspace, repo))
        })()
        .map_err(|err| {
            let _ = std::fs::remove_dir_all(jj_dir);
            err
        })
    }

    pub fn init_with_backend(
        user_settings: &UserSettings,
        workspace_root: &Path,
        backend_factory: impl FnOnce(&Path) -> Result<Box<dyn Backend>, BackendInitError>,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        Self::init_with_factories(
            user_settings,
            workspace_root,
            backend_factory,
            ReadonlyRepo::default_op_store_factory(),
            ReadonlyRepo::default_op_heads_store_factory(),
            ReadonlyRepo::default_index_store_factory(),
            ReadonlyRepo::default_submodule_store_factory(),
            WorkspaceId::default(),
        )
    }

    pub fn init_workspace_with_existing_repo(
        user_settings: &UserSettings,
        workspace_root: &Path,
        repo: &Arc<ReadonlyRepo>,
        workspace_id: WorkspaceId,
    ) -> Result<(Self, Arc<ReadonlyRepo>), WorkspaceInitError> {
        let jj_dir = create_jj_dir(workspace_root)?;

        let repo_dir = repo.repo_path().canonicalize().context(repo.repo_path())?;
        let repo_file_path = jj_dir.join("repo");
        let mut repo_file = File::create(&repo_file_path).context(&repo_file_path)?;
        repo_file
            .write_all(
                repo_dir
                    .to_str()
                    .ok_or(WorkspaceInitError::NonUnicodePath)?
                    .as_bytes(),
            )
            .context(&repo_file_path)?;

        let (working_copy, repo) =
            init_working_copy(user_settings, repo, workspace_root, &jj_dir, workspace_id)?;
        let workspace = Workspace::new(workspace_root, working_copy, repo.loader())?;
        Ok((workspace, repo))
    }

    pub fn load(
        user_settings: &UserSettings,
        workspace_path: &Path,
        store_factories: &StoreFactories,
    ) -> Result<Self, WorkspaceLoadError> {
        let loader = WorkspaceLoader::init(workspace_path)?;
        let workspace = loader.load(user_settings, store_factories)?;
        Ok(workspace)
    }

    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        self.working_copy.workspace_id()
    }

    pub fn repo_path(&self) -> &PathBuf {
        self.repo_loader.repo_path()
    }

    pub fn repo_loader(&self) -> &RepoLoader {
        &self.repo_loader
    }

    pub fn working_copy(&self) -> &LocalWorkingCopy {
        &self.working_copy
    }

    pub fn start_working_copy_mutation(
        &mut self,
    ) -> Result<LockedWorkspace, WorkingCopyStateError> {
        let locked_wc = self.working_copy.start_mutation()?;
        Ok(LockedWorkspace {
            base: self,
            locked_wc,
        })
    }

    pub fn check_out(
        &mut self,
        operation_id: OperationId,
        old_tree_id: Option<&MergedTreeId>,
        new_tree: &MergedTree,
    ) -> Result<CheckoutStats, CheckoutError> {
        let mut locked_ws =
            self.start_working_copy_mutation()
                .map_err(|err| CheckoutError::Other {
                    message: "Failed to start editing the working copy state".to_string(),
                    err: err.into(),
                })?;
        // Check if the current working-copy commit has changed on disk compared to what
        // the caller expected. It's safe to check out another commit
        // regardless, but it's probably not what  the caller wanted, so we let
        // them know.
        if let Some(old_tree_id) = old_tree_id {
            if old_tree_id != locked_ws.locked_wc().old_tree_id() {
                return Err(CheckoutError::ConcurrentCheckout);
            }
        }
        let stats = locked_ws.locked_wc().check_out(new_tree)?;
        locked_ws
            .finish(operation_id)
            .map_err(|err| CheckoutError::Other {
                message: "Failed to save the working copy state".to_string(),
                err: err.into(),
            })?;
        Ok(stats)
    }
}

pub struct LockedWorkspace<'a> {
    base: &'a mut Workspace,
    locked_wc: LockedLocalWorkingCopy,
}

impl<'a> LockedWorkspace<'a> {
    pub fn locked_wc(&mut self) -> &mut LockedLocalWorkingCopy {
        &mut self.locked_wc
    }

    pub fn finish(self, operation_id: OperationId) -> Result<(), WorkingCopyStateError> {
        let new_wc = self.locked_wc.finish(operation_id)?;
        self.base.working_copy = new_wc;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceLoader {
    workspace_root: PathBuf,
    repo_dir: PathBuf,
    working_copy_state_path: PathBuf,
}

impl WorkspaceLoader {
    pub fn init(workspace_root: &Path) -> Result<Self, WorkspaceLoadError> {
        let jj_dir = workspace_root.join(".jj");
        if !jj_dir.is_dir() {
            return Err(WorkspaceLoadError::NoWorkspaceHere(
                workspace_root.to_owned(),
            ));
        }
        let mut repo_dir = jj_dir.join("repo");
        // If .jj/repo is a file, then we interpret its contents as a relative path to
        // the actual repo directory (typically in another workspace).
        if repo_dir.is_file() {
            let buf = fs::read(&repo_dir).context(&repo_dir)?;
            let repo_path_str =
                String::from_utf8(buf).map_err(|_| WorkspaceLoadError::NonUnicodePath)?;
            repo_dir = jj_dir
                .join(&repo_path_str)
                .canonicalize()
                .context(&repo_path_str)?;
            if !repo_dir.is_dir() {
                return Err(WorkspaceLoadError::RepoDoesNotExist(repo_dir));
            }
        }
        let working_copy_state_path = jj_dir.join("working_copy");
        Ok(WorkspaceLoader {
            workspace_root: workspace_root.to_owned(),
            repo_dir,
            working_copy_state_path,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo_dir
    }

    pub fn load(
        &self,
        user_settings: &UserSettings,
        store_factories: &StoreFactories,
    ) -> Result<Workspace, WorkspaceLoadError> {
        let repo_loader = RepoLoader::init(user_settings, &self.repo_dir, store_factories)?;
        let working_copy = LocalWorkingCopy::load(
            repo_loader.store().clone(),
            self.workspace_root.clone(),
            self.working_copy_state_path.clone(),
        );
        let workspace = Workspace::new(&self.workspace_root, working_copy, repo_loader)?;
        Ok(workspace)
    }
}
