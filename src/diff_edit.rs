// Copyright 2020 Google LLC
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

use jj_lib::repo_path::{DirRepoPath, RepoPath};
use jj_lib::store::{StoreError, TreeId, TreeValue};
use jj_lib::store_wrapper::StoreWrapper;
use jj_lib::tree::Tree;
use jj_lib::tree_builder::TreeBuilder;
use jj_lib::trees::merge_trees;
use jj_lib::working_copy::{CheckoutError, TreeState};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tempfile::tempdir;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiffEditError {
    #[error("The diff tool exited with a non-zero code")]
    DifftoolAborted,
    #[error("Failed to write directories to diff: {0:?}")]
    CheckoutError(CheckoutError),
    #[error("Internal error: {0:?}")]
    InternalStoreError(StoreError),
}

impl From<CheckoutError> for DiffEditError {
    fn from(err: CheckoutError) -> Self {
        DiffEditError::CheckoutError(err)
    }
}

impl From<StoreError> for DiffEditError {
    fn from(err: StoreError) -> Self {
        DiffEditError::InternalStoreError(err)
    }
}

fn add_to_tree(
    store: &StoreWrapper,
    tree_builder: &mut TreeBuilder,
    repo_path: &RepoPath,
    value: &TreeValue,
) -> Result<(), StoreError> {
    match value {
        TreeValue::Conflict(conflict_id) => {
            let conflict = store.read_conflict(conflict_id)?;
            let materialized_value =
                jj_lib::conflicts::conflict_to_materialized_value(store, repo_path, &conflict);
            tree_builder.set(repo_path.clone(), materialized_value);
        }
        _ => {
            tree_builder.set(repo_path.clone(), (*value).clone());
        }
    }
    Ok(())
}

fn check_out(
    store: Arc<StoreWrapper>,
    wc_dir: PathBuf,
    state_dir: PathBuf,
    tree_id: TreeId,
) -> Result<TreeState, DiffEditError> {
    std::fs::create_dir(&wc_dir).unwrap();
    std::fs::create_dir(&state_dir).unwrap();
    let mut tree_state = TreeState::init(store, wc_dir, state_dir);
    tree_state.check_out(tree_id)?;
    Ok(tree_state)
}

pub fn edit_diff(left_tree: &Tree, right_tree: &Tree) -> Result<TreeId, DiffEditError> {
    // First create partial Trees of only the subset of the left and right trees
    // that affect files changed between them.
    let store = left_tree.store();
    let mut left_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut right_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    left_tree.diff(&right_tree, &mut |file_path, diff| {
        let (left_value, right_value) = diff.as_options();
        let repo_path = file_path.to_repo_path();
        if let Some(value) = left_value {
            add_to_tree(store, &mut left_tree_builder, &repo_path, value).unwrap();
        }
        if let Some(value) = right_value {
            add_to_tree(store, &mut right_tree_builder, &repo_path, value).unwrap();
        }
    });
    let left_partial_tree_id = left_tree_builder.write_tree();
    let right_partial_tree_id = right_tree_builder.write_tree();
    let right_partial_tree = store.get_tree(&DirRepoPath::root(), &right_partial_tree_id)?;

    // Check out the two partial trees in temporary directories.
    let temp_dir = tempdir().unwrap();
    let left_wc_dir = temp_dir.path().join("left");
    let left_state_dir = temp_dir.path().join("left_state");
    let right_wc_dir = temp_dir.path().join("right");
    let right_state_dir = temp_dir.path().join("right_state");
    check_out(
        store.clone(),
        left_wc_dir.clone(),
        left_state_dir,
        left_partial_tree_id,
    )?;
    // TODO: mark left dir readonly
    let mut right_tree_state = check_out(
        store.clone(),
        right_wc_dir.clone(),
        right_state_dir,
        right_partial_tree_id,
    )?;

    // Start a diff editor on the two directories.
    let exit_status = Command::new("meld")
        .arg(&left_wc_dir)
        .arg(&right_wc_dir)
        .status()
        .expect("failed to run diff editor");
    if !exit_status.success() {
        return Err(DiffEditError::DifftoolAborted);
    }

    // Create a Tree based on the initial right tree, applying the changes made to
    // that directory by the diff editor.
    let new_right_partial_tree_id = right_tree_state.write_tree();
    let new_right_partial_tree =
        store.get_tree(&DirRepoPath::root(), &new_right_partial_tree_id)?;
    let new_tree_id = merge_trees(right_tree, &right_partial_tree, &new_right_partial_tree)?;

    Ok(new_tree_id)
}
