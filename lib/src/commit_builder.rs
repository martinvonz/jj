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

use uuid::Uuid;

use crate::backend;
use crate::backend::{ChangeId, CommitId, Signature, TreeId};
use crate::commit::Commit;
use crate::repo::MutableRepo;
use crate::settings::UserSettings;

pub struct CommitBuilder<'repo> {
    mut_repo: &'repo mut MutableRepo,
    commit: backend::Commit,
    rewrite_source: Option<Commit>,
}

pub fn new_change_id() -> ChangeId {
    ChangeId::from_bytes(Uuid::new_v4().as_bytes())
}

impl CommitBuilder<'_> {
    pub fn for_new_commit<'repo>(
        mut_repo: &'repo mut MutableRepo,
        settings: &UserSettings,
        parents: Vec<CommitId>,
        tree_id: TreeId,
    ) -> CommitBuilder<'repo> {
        let signature = settings.signature();
        assert!(!parents.is_empty());
        let commit = backend::Commit {
            parents,
            predecessors: vec![],
            root_tree: tree_id,
            change_id: new_change_id(),
            description: String::new(),
            author: signature.clone(),
            committer: signature,
        };
        CommitBuilder {
            mut_repo,
            commit,
            rewrite_source: None,
        }
    }

    pub fn for_rewrite_from<'repo>(
        mut_repo: &'repo mut MutableRepo,
        settings: &UserSettings,
        predecessor: &Commit,
    ) -> CommitBuilder<'repo> {
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
            mut_repo,
            commit,
            rewrite_source: Some(predecessor.clone()),
        }
    }

    pub fn set_parents(mut self, parents: Vec<CommitId>) -> Self {
        assert!(!parents.is_empty());
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

    pub fn set_description(mut self, description: impl Into<String>) -> Self {
        self.commit.description = description.into();
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

    pub fn write(self) -> Commit {
        let mut rewrite_source_id = None;
        if let Some(rewrite_source) = self.rewrite_source {
            if *rewrite_source.change_id() == self.commit.change_id {
                rewrite_source_id.replace(rewrite_source.id().clone());
            }
        }
        let commit = self.mut_repo.write_commit(self.commit);
        if let Some(rewrite_source_id) = rewrite_source_id {
            self.mut_repo
                .record_rewritten_commit(rewrite_source_id, commit.id().clone())
        }
        commit
    }
}
