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

//! This file contains the default implementation of the `WorkingCopyStore` for both the Git and
//! native Backend. It stores the working copies in the `.jj/run/default` path as directories.
use std::any::Any;
use std::borrow::Borrow;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use itertools::Itertools;

use crate::backend::MergedTreeId;
use crate::commit::{self, Commit};
use crate::local_working_copy::TreeState;
use crate::object_id::ObjectId;
use crate::repo::Repo;
use crate::revset::{RevsetExpression, RevsetIteratorExt};
use crate::store::Store;
use crate::working_copy_store::{CachedWorkingCopy, WorkingCopyStore, WorkingCopyStoreError};

/// A thin wrapper over a `TreeState` for now.
// TODO: Move this to a LocalWorkingCopy instead of using just the TreeState.
#[derive(Clone)]
struct StoredWorkingCopy {
    /// The actual commit which owns the associated [`TreeState`].
    commit: Commit,
    /// Current state of the associated [`WorkingCopy`].
    state: Arc<TreeState>,
    /// The output path for tools, which do not specify a location. Like C(++) Compilers, scripts and more.
    /// It also contains the respective output stream, so stderr and stdout which was redirected for this commit.
    output_path: PathBuf,
    /// Path to the associated working copy.
    working_copy_path: PathBuf,
    /// Path to the associated tree state.
    state_path: PathBuf,
    /// Is this working-copy in use?
    pub(crate) is_used: bool,
}

impl StoredWorkingCopy {
    /// Set up a `StoredWorkingCopy`. It's assumed that all paths exist on disk.
    fn create(
        store: Arc<Store>,
        commit: Commit,
        output_path: PathBuf,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> Self {
        // Load the tree for our commit.
        let state = Arc::new(
            TreeState::load(store, working_copy_path.clone(), state_path.clone()).unwrap(),
        );
        Self {
            commit,
            state,
            output_path,
            working_copy_path,
            state_path,
            is_used: false,
        }
    }

    /// Replace the currently cached working-copy and it's tree with the tree from `commit`.
    /// Automatically marks it as used.
    fn replace_with(&mut self, commit: &Commit) -> Result<Self, WorkingCopyStoreError> {
        let Self {
            commit: _,
            ref mut state,
            output_path,
            working_copy_path,
            state_path,
            is_used: _,
        } = self;
        state.check_out(&commit.tree()?).map_err(|e| {
            WorkingCopyStoreError::TreeUpdate(format!(
                "failed to update the local working-copy with {e:?}"
            ))
        })?;

        Ok(Self {
            commit: commit.clone(),
            state: state.clone(),
            output_path: output_path.to_path_buf(),
            working_copy_path: working_copy_path.to_path_buf(),
            state_path: state_path.to_path_buf(),
            is_used: true,
        })
    }
}

impl CachedWorkingCopy for StoredWorkingCopy {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn working_copy_path(&self) -> &Path {
        self.working_copy_path.as_path()
    }

    fn state_path(&self) -> &Path {
        self.state_path.as_path()
    }

    fn output_path(&self) -> &Path {
        self.output_path.as_path()
    }

    fn exists(&self) -> bool {
        self.state_path.exists() && self.working_copy_path.exists()
    }
}

/// The default [`WorkingCopyStore`] for both the Git and native backend.
// TODO: Offload the creation of working copy directories onto a threadpool.
#[derive(Default)]
pub struct DefaultWorkingCopyStore {
    /// Where the working copies are stored, in this case `.jj/run/default/`
    stored_paths: PathBuf,
    /// All managed working copies.
    stored_working_copies: Vec<StoredWorkingCopy>,
    /// The store which owns this and all other backend related stuff. It gets set during the first
    /// creation of the managed working copies.
    store: OnceLock<Arc<Store>>,
}

/// Creates the required directories for a StoredWorkingCopy.
/// Returns a tuple of (`output_dir`, `working_copy` and `state`).
fn create_working_copy_paths(
    path: &PathBuf,
) -> Result<(PathBuf, PathBuf, PathBuf), std::io::Error> {
    let output = path.join("output");
    let working_copy = path.join("working_copy");
    let state = path.join("state");
    std::fs::create_dir(&output)?;
    std::fs::create_dir(&working_copy)?;
    std::fs::create_dir(&state)?;
    Ok((output, working_copy, state))
}

/// Represent a `MergeTreeId` in a way that it may be used as a working-copy
/// name. This makes no stability guarantee, as the format may change at
/// any time.
fn to_wc_name(id: &MergedTreeId) -> String {
    match id {
        MergedTreeId::Legacy(tree_id) => tree_id.hex(),
        MergedTreeId::Merge(tree_ids) => {
            let ids = tree_ids
                .map(|id| id.hex())
                .iter_mut()
                .enumerate()
                .map(|(i, s)| {
                    // Incredibly "smart" way to say, append "-" if the number is odd "+"
                    // otherwise.
                    if i & 1 != 0 {
                        s.push('-');
                    } else {
                        s.push('+');
                    }
                    s.to_owned()
                })
                .collect_vec();
            let mut obfuscated: String = ids.concat();
            // `PATH_MAX` could be a problem for different operating systems, so truncate it.
            if obfuscated.len() >= 255 {
                obfuscated.truncate(200);
            }
            obfuscated
        }
    }
}

impl DefaultWorkingCopyStore {
    pub fn name() -> &'static str {
        "default"
    }

    pub fn init(dot_dir: &Path) -> Self {
        let stored_paths = dot_dir.join(Self::name());
        // If the toplevel dir doesn't exist, create it.
        if !stored_paths.exists() {
            // TODO: correct error handling
            std::fs::create_dir(stored_paths.clone()).expect("shouldn't fail");
        }

        Self {
            stored_paths,
            ..Default::default()
        }
    }

    pub fn load(dot_dir: &Path) -> Self {
        Self::init(dot_dir)
    }

    fn create_working_copies(
        &mut self,
        revisions: &[Commit],
    ) -> Result<Vec<Box<dyn CachedWorkingCopy>>, std::io::Error> {
        let store = revisions
            .first()
            .expect("revisions shouldn't be empty")
            .store();
        // only set the store if we're a fresh call or a reload.
        self.store.get_or_init(|| store.clone());
        let mut results: Vec<Box<dyn CachedWorkingCopy>> = Vec::new();
        // Use the tree id for a unique directory.
        for rev in revisions {
            let tree_id = to_wc_name(&rev.tree_id());
            let path: PathBuf = self.stored_paths.join(tree_id);
            // Create a dir under `.jj/run/`.
            std::fs::create_dir(&path)?;
            // And the additional directories.
            let (output, working_copy_path, state) = create_working_copy_paths(&path)?;
            let cached_wc = StoredWorkingCopy::create(
                store.clone(),
                rev.clone(),
                output,
                working_copy_path,
                state,
            );
            let cached_clone = cached_wc.clone();
            self.stored_working_copies.push(cached_wc);
            results.push(Box::new(cached_clone) as Box<dyn CachedWorkingCopy>);
        }
        Ok(results)
    }
}

impl WorkingCopyStore for DefaultWorkingCopyStore {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &'static str {
        Self::name()
    }

    fn get_or_create_working_copies(
        &mut self,
        repo: &dyn Repo,
        revisions: Vec<Commit>,
    ) -> Result<Vec<Box<dyn CachedWorkingCopy>>, WorkingCopyStoreError> {
        // This is the initial call for a Workspace, so just create working-copies.
        if self.stored_working_copies.is_empty() {
            return Ok(self.create_working_copies(&revisions)?);
        }
        assert!(
            !self.stored_working_copies.is_empty(),
            "we must have working copies after the first call"
        );
        // If we already have some existing working copies, try to minimize pending work.
        // This is done by finding the intersection of the existing and new commits and only
        // creating the non-overlapping revisions.
        let new_revision_ids = revisions.iter().map(|rev| rev.id().clone()).collect_vec();
        let contained_revisions = self
            .stored_working_copies
            .iter()
            .map(|sc| sc.commit.id().clone())
            .collect_vec();
        let contained_revset = RevsetExpression::commits(contained_revisions);
        // intersect the existing revisions with the newly requested revisions to see which need to
        // be replaced.
        let overlapping_commits_revset =
            &contained_revset.intersection(&RevsetExpression::commits(new_revision_ids));
        let overlappping_commits: Vec<commit::Commit> = overlapping_commits_revset
            .clone()
            .evaluate_programmatic(repo)?
            .iter()
            .commits(self.store.get().unwrap())
            .try_collect()?;
        // the new revisions which we need to create.
        let new_revisions: Vec<commit::Commit> = overlapping_commits_revset
            .minus(&contained_revset)
            .evaluate_programmatic(repo)?
            .iter()
            .commits(self.store.get().unwrap())
            .try_collect()?;

        self.stored_working_copies
            .iter_mut()
            .filter(|sc| !overlappping_commits.contains(&sc.commit))
            // I don't know if this works.
            .map(|sc| sc.replace_with(new_revisions.iter().next().unwrap()));

        // the caller is going to use the working-copies so mark them as that.
        self.stored_working_copies
            .iter_mut()
            .map(|sc| sc.is_used = true);

        Ok(self
            .stored_working_copies
            .iter()
            .map(|sc| Box::new(sc.clone()) as Box<dyn CachedWorkingCopy>)
            .collect_vec())
    }

    fn has_stores(&self) -> bool {
        !self.stored_working_copies.is_empty()
    }

    fn unused_stores(&self) -> usize {
        self.stored_working_copies
            .iter()
            .map(|sc| !sc.is_used)
            .count()
    }

    fn update_working_copies(
        &mut self,
        _repo: &dyn Repo,
        replacements: Vec<Commit>,
    ) -> Result<(), WorkingCopyStoreError> {
        // Find multiple unused working copies and replace them.
        let mut old_wcs = self
            .stored_working_copies
            .iter()
            .filter(|sc| !sc.is_used)
            .collect_vec();
        // TODO: is this correct?
        old_wcs
            .iter_mut()
            .map(|wc| wc.replace_with(replacements.iter().next().unwrap()));

        Ok(())
    }

    fn update_single(&mut self, new_commit: Commit) -> Result<(), WorkingCopyStoreError> {
        let old_wc: &mut StoredWorkingCopy = self
            .stored_working_copies
            .iter_mut()
            .find(|sc| !sc.is_used)
            .unwrap();
        old_wc.replace_with(&new_commit)?;
        Ok(())
    }
}
