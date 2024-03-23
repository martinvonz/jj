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

use crate::backend::{self, BackendResult, ChangeId, CommitId, MergedTreeId, Signature, SigningFn};
use crate::commit::Commit;
use crate::repo::{MutableRepo, Repo};
use crate::settings::{JJRng, SignSettings, UserSettings};
use crate::signing::SignBehavior;

#[must_use]
pub struct CommitBuilder<'repo> {
    mut_repo: &'repo mut MutableRepo,
    rng: Arc<JJRng>,
    commit: backend::Commit,
    rewrite_source: Option<Commit>,
    sign_settings: SignSettings,
}

impl CommitBuilder<'_> {
    /// Only called from [`MutRepo::new_commit`]. Use that function instead.
    pub(crate) fn for_new_commit<'repo>(
        mut_repo: &'repo mut MutableRepo,
        settings: &UserSettings,
        parents: Vec<CommitId>,
        tree_id: MergedTreeId,
    ) -> CommitBuilder<'repo> {
        let signature = settings.signature();
        assert!(!parents.is_empty());
        let rng = settings.get_rng();
        let change_id = rng.new_change_id(mut_repo.store().change_id_length());
        let commit = backend::Commit {
            parents,
            predecessors: vec![],
            root_tree: tree_id,
            change_id,
            description: String::new(),
            author: signature.clone(),
            committer: signature,
            secure_sig: None,
        };
        CommitBuilder {
            mut_repo,
            rng,
            commit,
            rewrite_source: None,
            sign_settings: settings.sign_settings(),
        }
    }

    /// Only called from [`MutRepo::rewrite_commit`]. Use that function instead.
    pub(crate) fn for_rewrite_from<'repo>(
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
            sign_settings: settings.sign_settings(),
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

    pub fn tree_id(&self) -> &MergedTreeId {
        &self.commit.root_tree
    }

    pub fn set_tree_id(mut self, tree_id: MergedTreeId) -> Self {
        self.commit.root_tree = tree_id;
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

    pub fn sign_settings(&self) -> &SignSettings {
        &self.sign_settings
    }

    pub fn set_sign_behavior(mut self, sign_behavior: SignBehavior) -> Self {
        self.sign_settings.behavior = sign_behavior;
        self
    }

    pub fn set_sign_key(mut self, sign_key: Option<String>) -> Self {
        self.sign_settings.key = sign_key;
        self
    }

    pub fn write(mut self) -> BackendResult<Commit> {
        let sign_settings = &self.sign_settings;
        let store = self.mut_repo.store();

        let mut signing_fn = (store.signer().can_sign() && sign_settings.should_sign(&self.commit))
            .then(|| -> Box<SigningFn> {
                let store = store.clone();
                Box::new(move |data: &_| store.signer().sign(data, sign_settings.key.as_deref()))
            });

        // Commit backend doesn't use secure_sig for writing and enforces it with an
        // assert, but sign_settings.should_sign check above will want to know
        // if we're rewriting a signed commit
        self.commit.secure_sig = None;

        let commit = self
            .mut_repo
            .write_commit(self.commit, signing_fn.as_deref_mut())?;
        if let Some(rewrite_source) = self.rewrite_source {
            if rewrite_source.change_id() == commit.change_id() {
                self.mut_repo
                    .set_rewritten_commit(rewrite_source.id().clone(), commit.id().clone());
            }
        }
        Ok(commit)
    }
}
