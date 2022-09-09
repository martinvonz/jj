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

use crate::backend;
use crate::backend::{ChangeId, CommitId, Signature, TreeId};
use crate::commit::Commit;
use crate::repo::MutableRepo;
use crate::settings::UserSettings;

#[derive(Debug)]
pub struct CommitBuilder {
    commit: backend::Commit,
    rewrite_source: Option<Commit>,
}

pub fn new_change_id() -> ChangeId {
    ChangeId::from_bytes(Uuid::new_v4().as_bytes())
}

impl CommitBuilder {
    pub fn for_new_commit(settings: &UserSettings, tree_id: TreeId) -> CommitBuilder {
        let signature = settings.signature();
        let commit = backend::Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: tree_id,
            change_id: new_change_id(),
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            is_open: false,
        };
        CommitBuilder {
            commit,
            rewrite_source: None,
        }
    }

    pub fn for_rewrite_from(settings: &UserSettings, predecessor: &Commit) -> CommitBuilder {
        let mut commit = predecessor.store_commit().clone();
        commit.predecessors = vec![predecessor.id().clone()];
        commit.committer = settings.signature();
        // If the user had not configured a name and email before but now they have,
        // update the author fields with the new information.
        if commit.author.name == UserSettings::user_name_placeholder() {
            commit.author.name = commit.committer.name.clone();
        }
        if commit.author.email == UserSettings::user_email_placeholder() {
            commit.author.email = commit.committer.email.clone();
        }
        CommitBuilder {
            commit,
            rewrite_source: Some(predecessor.clone()),
        }
    }

    pub fn for_open_commit(
        settings: &UserSettings,
        parent_id: CommitId,
        tree_id: TreeId,
    ) -> CommitBuilder {
        let signature = settings.signature();
        let commit = backend::Commit {
            parents: vec![parent_id],
            predecessors: vec![],
            root_tree: tree_id,
            change_id: new_change_id(),
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            is_open: true,
        };
        CommitBuilder {
            commit,
            rewrite_source: None,
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

    pub fn set_author(mut self, author: Signature) -> Self {
        self.commit.author = author;
        self
    }

    pub fn set_committer(mut self, committer: Signature) -> Self {
        self.commit.committer = committer;
        self
    }

    pub fn write_to_repo(mut self, repo: &mut MutableRepo) -> Commit {
        let parents = &mut self.commit.parents;
        if parents.contains(repo.store().root_commit_id()) {
            assert_eq!(parents.len(), 1);
            parents.clear();
        }
        let mut rewrite_source_id = None;
        if let Some(rewrite_source) = self.rewrite_source {
            if *rewrite_source.change_id() == self.commit.change_id {
                rewrite_source_id.replace(rewrite_source.id().clone());
            }
        }
        let commit = repo.write_commit(self.commit);
        if let Some(rewrite_source_id) = rewrite_source_id {
            repo.record_rewritten_commit(rewrite_source_id, commit.id().clone())
        }
        commit
    }
}
