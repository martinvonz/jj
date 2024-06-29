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

//! This file contains the [`WorkingCopyStore`] interface which is used to cached working copies.
//!
//! These must be implemented for Virtual Filesystems such as [EdenFS]
//! to allow cheaper working copy materializations, they are used for the `jj run`
//! implementation.
//!
//!
//! [EdenFS]: www.github.com/facebook/sapling/main/blob/eden/fs

use std::{any::Any, convert::Infallible, io, path::Path};

use crate::{backend::BackendError, commit::Commit, repo::Repo, revset::RevsetEvaluationError};
use thiserror::Error;

/// Represents a [`WorkingCopy`] which is cached somewhere, it could be on disk or on another
/// machine.
pub trait CachedWorkingCopy {
    /// Allow trait upcasting
    fn as_any(&self) -> &dyn Any;

    /// The path where the `CachedWorkingCopy`'s wc is stored.
    fn working_copy_path(&self) -> &Path;

    /// The path where the `TreeState` of the `CachedWorkingCopy` is stored.
    fn state_path(&self) -> &Path;

    /// The output path for artifacts from a build system or script. This is for the `jj run` use
    /// case of an CMake/Make or Ninja invocation.
    // TODO: Is it possible to inject this path into the command after generating it?
    fn output_path(&self) -> &Path;

    /// Does this `CachedWorkingCopy` exist somewhere.
    fn exists(&self) -> bool;
}

/// A general Error which this backend returns.
#[derive(Debug, Error)]
pub enum WorkingCopyStoreError {
    /// We failed to initialize something, the store or any underlying working-copies.
    #[error("failed to initialize")]
    Initialization(#[from] io::Error),
    /// An error occured during a `CachedWorkingCopy` update.
    #[error("could not update the working copy {0}")]
    TreeUpdate(String),
    /// If the backend failed internally.
    #[error("backend failed internally")]
    Backend(#[from] BackendError),
    /// Any internal error, which shouldn't be propagated to the user.
    // TODO: This ideally also should contain the `RevsetError`, as it purely is an implementation
    // detail.
    #[error("internal error")]
    Internal(#[from] Infallible),
    // The variant below shouldn't exist.
    #[error("revset evaluation failed")]
    Revset(#[from] RevsetEvaluationError),
}

/// A `WorkingCopyStore` manages the working copies on disk for `jj run`.
/// It's an ideal extension point for an virtual filesystem, as they ease the creation of
/// working copies.
///
/// The trait's design is similar to a database. Clients request a single or multiple working-copies
/// and the backend can coalesce the requests if needed. This allows an implementation to build
/// a global view of all actively used working-copies and where they are stored.
pub trait WorkingCopyStore: Send + Sync {
    /// Return `self` as `Any` to allow trait upcasting.
    fn as_any(&self) -> &dyn Any;

    /// The name of the backend, determines how it actually interacts with working copies.
    fn name(&self) -> &str;

    /// Get existing or create `Stores` for `revisions`.
    fn get_or_create_working_copies(
        &mut self,
        repo: &dyn Repo,
        revisions: Vec<Commit>,
    ) -> Result<Vec<Box<dyn CachedWorkingCopy>>, WorkingCopyStoreError>;

    /// Are any `Stores` available.
    fn has_stores(&self) -> bool;

    /// How many unused stores are available. This should allow clients to determine when to batch
    /// requests.
    fn unused_stores(&self) -> usize;

    /// Update multiple stored working copies at once, akin to a sql update.
    fn update_working_copies(
        &mut self,
        repo: &dyn Repo,
        replacements: Vec<Commit>,
    ) -> Result<(), WorkingCopyStoreError>;

    /// Update a single working-copy, determined by the backend.
    fn update_single(&mut self, new_commit: Commit) -> Result<(), WorkingCopyStoreError>;
}
