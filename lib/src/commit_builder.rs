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

use uuid::Uuid;

use crate::commit::Commit;
use crate::repo::ReadonlyRepo;
use crate::settings::UserSettings;
use crate::store;
use crate::store::{ChangeId, CommitId, Signature, Timestamp, TreeId};
use crate::store_wrapper::StoreWrapper;
use crate::transaction::Transaction;
use std::sync::Arc;

#[derive(Debug)]
pub struct CommitBuilder {
    store: Arc<StoreWrapper>,
    commit: store::Commit,
}

pub fn new_change_id() -> ChangeId {
    ChangeId(Uuid::new_v4().as_bytes().to_vec())
}
pub fn signature(settings: &UserSettings) -> Signature {
    // TODO: check if it's slow to get the timezone etc for every signature
    let timestamp = Timestamp::now();
    Signature {
        name: settings.user_name(),
        email: settings.user_email(),
        timestamp,
    }
}

impl CommitBuilder {
    pub fn for_new_commit(
        settings: &UserSettings,
        store: &Arc<StoreWrapper>,
        tree_id: TreeId,
    ) -> CommitBuilder {
        let signature = signature(settings);
        let commit = store::Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: tree_id,
            change_id: new_change_id(),
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            is_open: false,
            is_pruned: false,
        };
        CommitBuilder {
            store: store.clone(),
            commit,
        }
    }

    pub fn for_rewrite_from(
        settings: &UserSettings,
        store: &Arc<StoreWrapper>,
        predecessor: &Commit,
    ) -> CommitBuilder {
        let mut commit = predecessor.store_commit().clone();
        commit.predecessors = vec![predecessor.id().clone()];
        commit.committer = signature(settings);
        CommitBuilder {
            store: store.clone(),
            commit,
        }
    }

    pub fn for_open_commit(
        settings: &UserSettings,
        store: &Arc<StoreWrapper>,
        parent_id: CommitId,
        tree_id: TreeId,
    ) -> CommitBuilder {
        let signature = signature(settings);
        let commit = store::Commit {
            parents: vec![parent_id],
            predecessors: vec![],
            root_tree: tree_id,
            change_id: new_change_id(),
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            is_open: true,
            is_pruned: false,
        };
        CommitBuilder {
            store: store.clone(),
            commit,
        }
    }

    pub fn set_parents(mut self, parents: Vec<CommitId>) -> Self {
        self.commit.parents = parents;
        self
    }

    pub fn set_predecessors(mut self, predecessors: Vec<CommitId>) -> Self {
        self.commit.predecessors = predecessors;
        self
    }

    pub fn set_tree(mut self, tree_id: TreeId) -> Self {
        self.commit.root_tree = tree_id;
        self
    }

    pub fn set_change_id(mut self, change_id: ChangeId) -> Self {
        self.commit.change_id = change_id;
        self
    }

    pub fn generate_new_change_id(mut self) -> Self {
        self.commit.change_id = new_change_id();
        self
    }

    pub fn set_description(mut self, description: String) -> Self {
        self.commit.description = description;
        self
    }

    pub fn set_open(mut self, is_open: bool) -> Self {
        self.commit.is_open = is_open;
        self
    }

    pub fn set_pruned(mut self, is_pruned: bool) -> Self {
        self.commit.is_pruned = is_pruned;
        self
    }

    pub fn set_author(mut self, author: Signature) -> Self {
        self.commit.author = author;
        self
    }

    pub fn set_committer(mut self, committer: Signature) -> Self {
        self.commit.committer = committer;
        self
    }

    pub fn write_to_new_transaction(self, repo: &ReadonlyRepo, description: &str) -> Commit {
        let mut tx = repo.start_transaction(description);
        let commit = self.write_to_transaction(&mut tx);
        tx.commit();
        commit
    }

    pub fn write_to_transaction(mut self, tx: &mut Transaction) -> Commit {
        let parents = &mut self.commit.parents;
        if parents.contains(self.store.root_commit_id()) {
            assert_eq!(parents.len(), 1);
            parents.clear();
        }
        tx.write_commit(self.commit)
    }
}
