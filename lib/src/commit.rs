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

use std::cmp::Ordering;
use std::fmt::{Debug, Error, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::backend;
use crate::backend::{ChangeId, CommitId, Signature, TreeId};
use crate::repo_path::RepoPath;
use crate::store::Store;
use crate::tree::Tree;

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
        Some(self.id.cmp(&other.id))
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

    pub fn parent_ids(&self) -> Vec<CommitId> {
        if self.data.parents.is_empty() && &self.id != self.store.root_commit_id() {
            vec![self.store.root_commit_id().clone()]
        } else {
            self.data.parents.clone()
        }
    }

    pub fn parents(&self) -> Vec<Commit> {
        let mut parents = Vec::new();
        for parent in &self.data.parents {
            parents.push(self.store.get_commit(parent).unwrap());
        }
        if parents.is_empty() && &self.id != self.store.root_commit_id() {
            parents.push(self.store.root_commit())
        }
        parents
    }

    pub fn predecessor_ids(&self) -> Vec<CommitId> {
        self.data.predecessors.clone()
    }

    pub fn predecessors(&self) -> Vec<Commit> {
        let mut predecessors = Vec::new();
        for predecessor in &self.data.predecessors {
            predecessors.push(self.store.get_commit(predecessor).unwrap());
        }
        predecessors
    }

    pub fn tree(&self) -> Tree {
        self.store
            .get_tree(&RepoPath::root(), &self.data.root_tree)
            .unwrap()
    }

    pub fn tree_id(&self) -> &TreeId {
        &self.data.root_tree
    }

    pub fn change_id(&self) -> &ChangeId {
        &self.data.change_id
    }

    pub fn store_commit(&self) -> &backend::Commit {
        &self.data
    }

    pub fn is_open(&self) -> bool {
        self.data.is_open
    }

    pub fn is_empty(&self) -> bool {
        let parents = self.parents();
        // TODO: Perhaps the root commit should also be considered empty.
        parents.len() == 1 && parents[0].tree_id() == self.tree_id()
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
}
