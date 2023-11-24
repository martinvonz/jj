// Copyright 2020 The Jujutsu Authors
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

use std::cmp::Ordering;
use std::fmt::{Debug, Error, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::backend;
use crate::backend::{BackendError, ChangeId, CommitId, MergedTreeId, Signature};
use crate::merged_tree::MergedTree;
use crate::signing::{SignResult, Verification};
use crate::store::Store;

#[derive(Clone)]
pub struct Commit {
    store: Arc<Store>,
    id: CommitId,
    data: Arc<backend::Commit>,
}

impl Debug for Commit {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("Commit").field("id", &self.id).finish()
    }
}

impl PartialEq for Commit {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Commit {}

impl Ord for Commit {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for Commit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for Commit {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl Commit {
    pub fn new(store: Arc<Store>, id: CommitId, data: Arc<backend::Commit>) -> Self {
        Commit { store, id, data }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn id(&self) -> &CommitId {
        &self.id
    }

    pub fn parent_ids(&self) -> &[CommitId] {
        &self.data.parents
    }

    pub fn parents(&self) -> Vec<Commit> {
        self.data
            .parents
            .iter()
            .map(|id| self.store.get_commit(id).unwrap())
            .collect()
    }

    pub fn predecessor_ids(&self) -> &[CommitId] {
        &self.data.predecessors
    }

    pub fn predecessors(&self) -> Vec<Commit> {
        self.data
            .predecessors
            .iter()
            .map(|id| self.store.get_commit(id).unwrap())
            .collect()
    }

    pub fn tree(&self) -> Result<MergedTree, BackendError> {
        self.store.get_root_tree(&self.data.root_tree)
    }

    pub fn tree_id(&self) -> &MergedTreeId {
        &self.data.root_tree
    }

    pub fn has_conflict(&self) -> Result<bool, BackendError> {
        if let MergedTreeId::Merge(tree_ids) = self.tree_id() {
            Ok(!tree_ids.is_resolved())
        } else {
            Ok(self.tree()?.has_conflict())
        }
    }

    pub fn change_id(&self) -> &ChangeId {
        &self.data.change_id
    }

    pub fn store_commit(&self) -> &backend::Commit {
        &self.data
    }

    pub fn description(&self) -> &str {
        &self.data.description
    }

    pub fn author(&self) -> &Signature {
        &self.data.author
    }

    pub fn committer(&self) -> &Signature {
        &self.data.committer
    }

    /// A commit is discardable if it has one parent, no change from its
    /// parent, and an empty description.
    pub fn is_discardable(&self) -> bool {
        if self.description().is_empty() {
            if let [parent_commit] = &*self.parents() {
                return self.tree_id() == parent_commit.tree_id();
            }
        }
        false
    }

    /// A quick way to just check if a signature is present.
    pub fn is_signed(&self) -> bool {
        self.data.secure_sig.is_some()
    }

    /// A slow (but cached) way to get the full verification.
    pub fn verification(&self) -> SignResult<Option<Verification>> {
        self.data
            .secure_sig
            .as_ref()
            .map(|sig| self.store.signer().verify(&self.id, &sig.data, &sig.sig))
            .transpose()
    }
}

/// Wrapper to sort `Commit` by committer timestamp.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CommitByCommitterTimestamp(pub Commit);

impl Ord for CommitByCommitterTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_timestamp = &self.0.committer().timestamp.timestamp;
        let other_timestamp = &other.0.committer().timestamp.timestamp;
        self_timestamp
            .cmp(other_timestamp)
            .then_with(|| self.0.cmp(&other.0)) // to comply with Eq
    }
}

impl PartialOrd for CommitByCommitterTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
