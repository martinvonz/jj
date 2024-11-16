// Copyright 2023 The Jujutsu Authors
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

//! Defines the interface for the working copy. See `LocalWorkingCopy` for the
//! default local-disk implementation.

use std::any::Any;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use itertools::Itertools;
use thiserror::Error;
use tracing::instrument;

use crate::backend::BackendError;
use crate::backend::MergedTreeId;
use crate::commit::Commit;
use crate::conflicts::ConflictMarkerStyle;
use crate::dag_walk;
use crate::fsmonitor::FsmonitorSettings;
use crate::gitignore::GitIgnoreError;
use crate::gitignore::GitIgnoreFile;
use crate::matchers::EverythingMatcher;
use crate::matchers::Matcher;
use crate::op_heads_store::OpHeadsStoreError;
use crate::op_store::OpStoreError;
use crate::op_store::OperationId;
use crate::op_store::WorkspaceId;
use crate::operation::Operation;
use crate::repo::ReadonlyRepo;
use crate::repo::Repo;
use crate::repo::RewriteRootCommit;
use crate::repo_path::InvalidRepoPathError;
use crate::repo_path::RepoPath;
use crate::repo_path::RepoPathBuf;
use crate::settings::HumanByteSize;
use crate::settings::UserSettings;
use crate::store::Store;

/// The trait all working-copy implementations must implement.
pub trait WorkingCopy: Send {
    /// Should return `self`. For down-casting purposes.
    fn as_any(&self) -> &dyn Any;

    /// The name/id of the implementation. Used for choosing the right
    /// implementation when loading a working copy.
    fn name(&self) -> &str;

    /// The working copy's workspace ID.
    fn workspace_id(&self) -> &WorkspaceId;

    /// The operation this working copy was most recently updated to.
    fn operation_id(&self) -> &OperationId;

    /// The ID of the tree this working copy was most recently updated to.
    fn tree_id(&self) -> Result<&MergedTreeId, WorkingCopyStateError>;

    /// Patterns that decide which paths from the current tree should be checked
    /// out in the working copy. An empty list means that no paths should be
    /// checked out in the working copy. A single `RepoPath::root()` entry means
    /// that all files should be checked out.
    fn sparse_patterns(&self) -> Result<&[RepoPathBuf], WorkingCopyStateError>;

    /// Locks the working copy and returns an instance with methods for updating
    /// the working copy files and state.
    fn start_mutation(&self) -> Result<Box<dyn LockedWorkingCopy>, WorkingCopyStateError>;
}

/// The factory which creates and loads a specific type of working copy.
pub trait WorkingCopyFactory {
    /// Create a new working copy from scratch.
    fn init_working_copy(
        &self,
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        operation_id: OperationId,
        workspace_id: WorkspaceId,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError>;

    /// Load an existing working copy.
    fn load_working_copy(
        &self,
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError>;
}

/// A working copy that's being modified.
pub trait LockedWorkingCopy {
    /// Should return `self`. For down-casting purposes.
    fn as_any(&self) -> &dyn Any;

    /// Should return `self`. For down-casting purposes.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// The operation at the time the lock was taken
    fn old_operation_id(&self) -> &OperationId;

    /// The tree at the time the lock was taken
    fn old_tree_id(&self) -> &MergedTreeId;

    /// Snapshot the working copy and return the tree id.
    fn snapshot(&mut self, options: &SnapshotOptions) -> Result<MergedTreeId, SnapshotError>;

    /// Check out the specified commit in the working copy.
    fn check_out(
        &mut self,
        commit: &Commit,
        options: &CheckoutOptions,
    ) -> Result<CheckoutStats, CheckoutError>;

    /// Update the workspace name.
    fn rename_workspace(&mut self, new_workspace_name: WorkspaceId);

    /// Update to another commit without touching the files in the working copy.
    fn reset(&mut self, commit: &Commit) -> Result<(), ResetError>;

    /// Update to another commit without touching the files in the working copy,
    /// without assuming that the previous tree exists.
    fn recover(&mut self, commit: &Commit) -> Result<(), ResetError>;

    /// See `WorkingCopy::sparse_patterns()`
    fn sparse_patterns(&self) -> Result<&[RepoPathBuf], WorkingCopyStateError>;

    /// Updates the patterns that decide which paths from the current tree
    /// should be checked out in the working copy.
    // TODO: Use a different error type here so we can include a
    // `SparseNotSupported` variants for working copies that don't support sparse
    // checkouts (e.g. because they use a virtual file system so there's no reason
    // to use sparse).
    fn set_sparse_patterns(
        &mut self,
        new_sparse_patterns: Vec<RepoPathBuf>,
        options: &CheckoutOptions,
    ) -> Result<CheckoutStats, CheckoutError>;

    /// Finish the modifications to the working copy by writing the updated
    /// states to disk. Returns the new (unlocked) working copy.
    fn finish(
        self: Box<Self>,
        operation_id: OperationId,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError>;
}

/// An error while snapshotting the working copy.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// A tracked path contained invalid component such as `..`.
    #[error(transparent)]
    InvalidRepoPath(#[from] InvalidRepoPathError),
    /// A path in the working copy was not valid UTF-8.
    #[error("Working copy path {} is not valid UTF-8", path.to_string_lossy())]
    InvalidUtf8Path {
        /// The path with invalid UTF-8.
        path: OsString,
    },
    /// A symlink target in the working copy was not valid UTF-8.
    #[error("Symlink {path} target is not valid UTF-8")]
    InvalidUtf8SymlinkTarget {
        /// The path of the symlink that has a target that's not valid UTF-8.
        /// This path itself is valid UTF-8.
        path: PathBuf,
    },
    /// Reading or writing from the commit backend failed.
    #[error(transparent)]
    BackendError(#[from] BackendError),
    /// A file was larger than the specified maximum file size for new
    /// (previously untracked) files.
    #[error("New file {path} of size ~{size} exceeds snapshot.max-new-file-size ({max_size})")]
    NewFileTooLarge {
        /// The path of the large file.
        path: PathBuf,
        /// The size of the large file.
        size: HumanByteSize,
        /// The maximum allowed size.
        max_size: HumanByteSize,
    },
    /// Checking path with ignore patterns failed.
    #[error(transparent)]
    GitIgnoreError(#[from] GitIgnoreError),
    /// Some other error happened while snapshotting the working copy.
    #[error("{message}")]
    Other {
        /// Error message.
        message: String,
        /// The underlying error.
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Options used when snapshotting the working copy. Some of them may be ignored
/// by some `WorkingCopy` implementations.
#[derive(Clone)]
pub struct SnapshotOptions<'a> {
    /// The `.gitignore`s to use while snapshotting. The typically come from the
    /// user's configured patterns combined with per-repo patterns.
    // The base_ignores are passed in here rather than being set on the TreeState
    // because the TreeState may be long-lived if the library is used in a
    // long-lived process.
    pub base_ignores: Arc<GitIgnoreFile>,
    /// The fsmonitor (e.g. Watchman) to use, if any.
    // TODO: Should we make this a field on `LocalWorkingCopy` instead since it's quite specific to
    // that implementation?
    pub fsmonitor_settings: FsmonitorSettings,
    /// A callback for the UI to display progress.
    pub progress: Option<&'a SnapshotProgress<'a>>,
    /// For new files that are not already tracked, start tracking them if they
    /// match this.
    pub start_tracking_matcher: &'a dyn Matcher,
    /// The size of the largest file that should be allowed to become tracked
    /// (already tracked files are always snapshotted). If there are larger
    /// files in the working copy, then `LockedWorkingCopy::snapshot()` may
    /// (depending on implementation)
    /// return `SnapshotError::NewFileTooLarge`.
    pub max_new_file_size: u64,
    /// Expected conflict marker style for checking for changed files.
    pub conflict_marker_style: ConflictMarkerStyle,
}

impl SnapshotOptions<'_> {
    /// Create an instance for use in tests.
    pub fn empty_for_test() -> Self {
        SnapshotOptions {
            base_ignores: GitIgnoreFile::empty(),
            fsmonitor_settings: FsmonitorSettings::None,
            progress: None,
            start_tracking_matcher: &EverythingMatcher,
            max_new_file_size: u64::MAX,
            conflict_marker_style: ConflictMarkerStyle::default(),
        }
    }
}

/// A callback for getting progress updates.
pub type SnapshotProgress<'a> = dyn Fn(&RepoPath) + 'a + Sync;

/// Options used when checking out a tree in the working copy.
#[derive(Clone)]
pub struct CheckoutOptions {
    /// Conflict marker style to use when materializing files
    pub conflict_marker_style: ConflictMarkerStyle,
}

impl CheckoutOptions {
    /// Create an instance for use in tests.
    pub fn empty_for_test() -> Self {
        CheckoutOptions {
            conflict_marker_style: ConflictMarkerStyle::default(),
        }
    }
}

/// Stats about a checkout operation on a working copy. All "files" mentioned
/// below may also be symlinks or materialized conflicts.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CheckoutStats {
    /// The number of files that were updated in the working copy.
    /// These files existed before and after the checkout.
    pub updated_files: u32,
    /// The number of files added in the working copy.
    pub added_files: u32,
    /// The number of files removed in the working copy.
    pub removed_files: u32,
    /// The number of files that were supposed to be updated or added in the
    /// working copy but were skipped because there was an untracked (probably
    /// ignored) file in its place.
    pub skipped_files: u32,
}

/// The working-copy checkout failed.
#[derive(Debug, Error)]
pub enum CheckoutError {
    /// The current working-copy commit was deleted, maybe by an overly
    /// aggressive GC that happened while the current process was running.
    #[error("Current working-copy commit not found")]
    SourceNotFound {
        /// The underlying error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Another process checked out a commit while the current process was
    /// running (after the working copy was read by the current process).
    #[error("Concurrent checkout")]
    ConcurrentCheckout,
    /// Path in the commit contained invalid component such as `..`.
    #[error(transparent)]
    InvalidRepoPath(#[from] InvalidRepoPathError),
    /// Path contained reserved name which cannot be checked out to disk.
    #[error("Reserved path component {name} in {path}")]
    ReservedPathComponent {
        /// The file or directory path.
        path: PathBuf,
        /// The reserved path component.
        name: &'static str,
    },
    /// Reading or writing from the commit backend failed.
    #[error("Internal backend error")]
    InternalBackendError(#[from] BackendError),
    /// Some other error happened while checking out the working copy.
    #[error("{message}")]
    Other {
        /// Error message.
        message: String,
        /// The underlying error.
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// An error while resetting the working copy.
#[derive(Debug, Error)]
pub enum ResetError {
    /// The current working-copy commit was deleted, maybe by an overly
    /// aggressive GC that happened while the current process was running.
    #[error("Current working-copy commit not found")]
    SourceNotFound {
        /// The underlying error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Reading or writing from the commit backend failed.
    #[error("Internal error")]
    InternalBackendError(#[from] BackendError),
    /// Some other error happened while checking out the working copy.
    #[error("{message}")]
    Other {
        /// Error message.
        message: String,
        /// The underlying error.
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Whether the working copy is stale or not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkingCopyFreshness {
    /// The working copy isn't stale, and no need to reload the repo.
    Fresh,
    /// The working copy was updated since we loaded the repo. The repo must be
    /// reloaded at the working copy's operation.
    Updated(Box<Operation>),
    /// The working copy is behind the latest operation.
    WorkingCopyStale,
    /// The working copy is a sibling of the latest operation.
    SiblingOperation,
}

impl WorkingCopyFreshness {
    /// Determine the freshness of the provided working copy relative to the
    /// target commit.
    #[instrument(skip_all)]
    pub fn check_stale(
        locked_wc: &dyn LockedWorkingCopy,
        wc_commit: &Commit,
        repo: &ReadonlyRepo,
    ) -> Result<Self, OpStoreError> {
        // Check if the working copy's tree matches the repo's view
        let wc_tree_id = locked_wc.old_tree_id();
        if wc_commit.tree_id() == wc_tree_id {
            // The working copy isn't stale, and no need to reload the repo.
            Ok(Self::Fresh)
        } else {
            let wc_operation = repo.loader().load_operation(locked_wc.old_operation_id())?;
            let repo_operation = repo.operation();
            let ancestor_op = dag_walk::closest_common_node_ok(
                [Ok(wc_operation.clone())],
                [Ok(repo_operation.clone())],
                |op: &Operation| op.id().clone(),
                |op: &Operation| op.parents().collect_vec(),
            )?
            .expect("unrelated operations");
            if ancestor_op.id() == repo_operation.id() {
                // The working copy was updated since we loaded the repo. The repo must be
                // reloaded at the working copy's operation.
                Ok(Self::Updated(Box::new(wc_operation)))
            } else if ancestor_op.id() == wc_operation.id() {
                // The working copy was not updated when some repo operation committed,
                // meaning that it's stale compared to the repo view.
                Ok(Self::WorkingCopyStale)
            } else {
                Ok(Self::SiblingOperation)
            }
        }
    }
}

/// An error while recovering a stale working copy.
#[derive(Debug, Error)]
pub enum RecoverWorkspaceError {
    /// Backend error.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// Error during transaction.
    #[error(transparent)]
    OpHeadsStore(#[from] OpHeadsStoreError),
    /// Error during checkout.
    #[error(transparent)]
    Reset(#[from] ResetError),
    /// Checkout attempted to modify the root commit.
    #[error(transparent)]
    RewriteRootCommit(#[from] RewriteRootCommit),
    /// Working copy commit is missing.
    #[error("\"{0:?}\" doesn't have a working-copy commit")]
    WorkspaceMissingWorkingCopy(WorkspaceId),
}

/// Recover this workspace to its last known checkout.
pub fn create_and_check_out_recovery_commit(
    locked_wc: &mut dyn LockedWorkingCopy,
    repo: &Arc<ReadonlyRepo>,
    workspace_id: WorkspaceId,
    user_settings: &UserSettings,
    description: &str,
) -> Result<(Arc<ReadonlyRepo>, Commit), RecoverWorkspaceError> {
    let mut tx = repo.start_transaction(user_settings);
    let repo_mut = tx.repo_mut();

    let commit_id = repo
        .view()
        .get_wc_commit_id(&workspace_id)
        .ok_or_else(|| RecoverWorkspaceError::WorkspaceMissingWorkingCopy(workspace_id.clone()))?;
    let commit = repo.store().get_commit(commit_id)?;
    let new_commit = repo_mut
        .new_commit(
            user_settings,
            vec![commit_id.clone()],
            commit.tree_id().clone(),
        )
        .set_description(description)
        .write()?;
    repo_mut.set_wc_commit(workspace_id, new_commit.id().clone())?;

    let repo = tx.commit("recovery commit")?;
    locked_wc.recover(&new_commit)?;

    Ok((repo, new_commit))
}

/// An error while reading the working copy state.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct WorkingCopyStateError {
    /// Error message.
    pub message: String,
    /// The underlying error.
    #[source]
    pub err: Box<dyn std::error::Error + Send + Sync>,
}
