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

use std::sync::Arc;

use crate::backend::{self, BackendResult, ChangeId, CommitId, MergedTreeId, Signature, TreeId};
use crate::commit::Commit;
use crate::repo::{MutableRepo, Repo};
use crate::settings::{JJRng, UserSettings};

#[must_use]
pub struct CommitBuilder<'repo> {
    mut_repo: &'repo mut MutableRepo,
    rng: Arc<JJRng>,
    commit: backend::Commit,
    rewrite_source: Option<Commit>,
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
        let rng = settings.get_rng();
        let change_id = rng.new_change_id(mut_repo.store().change_id_length());
        let commit = backend::Commit {
            parents,
            predecessors: vec![],
            // TODO(#1624): use the Merge variant when appropriate
            root_tree: MergedTreeId::Legacy(tree_id),
            change_id,
            description: String::new(),
            author: signature.clone(),
            committer: signature,
        };
        CommitBuilder {
            mut_repo,
            rng,
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
        if commit.author.name.is_empty()
            || commit.author.name == UserSettings::USER_NAME_PLACEHOLDER
        {
            commit.author.name = commit.committer.name.clone();
        }
        if commit.author.email.is_empty()
            || commit.author.email == UserSettings::USER_EMAIL_PLACEHOLDER
        {
            commit.author.email = commit.committer.email.clone();
        }
        CommitBuilder {
            mut_repo,
            commit,
            rng: settings.get_rng(),
            rewrite_source: Some(predecessor.clone()),
        }
    }

    pub fn parents(&self) -> &[CommitId] {
        &self.commit.parents
    }

    pub fn set_parents(mut self, parents: Vec<CommitId>) -> Self {
        assert!(!parents.is_empty());
        self.commit.parents = parents;
        self
    }

    pub fn predecessors(&self) -> &[CommitId] {
        &self.commit.predecessors
    }

    pub fn set_predecessors(mut self, predecessors: Vec<CommitId>) -> Self {
        self.commit.predecessors = predecessors;
        self
    }

    pub fn tree(&self) -> &TreeId {
        self.commit.root_tree.as_legacy_tree_id()
    }

    pub fn set_tree(mut self, tree_id: TreeId) -> Self {
        self.commit.root_tree = MergedTreeId::Legacy(tree_id);
        self
    }

    pub fn change_id(&self) -> &ChangeId {
        &self.commit.change_id
    }

    pub fn set_change_id(mut self, change_id: ChangeId) -> Self {
        self.commit.change_id = change_id;
        self
    }

    pub fn generate_new_change_id(mut self) -> Self {
        self.commit.change_id = self
            .rng
            .new_change_id(self.mut_repo.store().change_id_length());
        self
    }

    pub fn description(&self) -> &str {
        &self.commit.description
    }

    pub fn set_description(mut self, description: impl Into<String>) -> Self {
        self.commit.description = description.into();
        self
    }

    pub fn author(&self) -> &Signature {
        &self.commit.author
    }

    pub fn set_author(mut self, author: Signature) -> Self {
        self.commit.author = author;
        self
    }

    pub fn committer(&self) -> &Signature {
        &self.commit.committer
    }

    pub fn set_committer(mut self, committer: Signature) -> Self {
        self.commit.committer = committer;
        self
    }

    pub fn write(self) -> BackendResult<Commit> {
        let mut rewrite_source_id = None;
        if let Some(rewrite_source) = self.rewrite_source {
            if *rewrite_source.change_id() == self.commit.change_id {
                rewrite_source_id.replace(rewrite_source.id().clone());
            }
        }
        let commit = self.mut_repo.write_commit(self.commit)?;
        if let Some(rewrite_source_id) = rewrite_source_id {
            self.mut_repo
                .record_rewritten_commit(rewrite_source_id, commit.id().clone())
        }
        Ok(commit)
    }
}
