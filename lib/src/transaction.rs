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

use crate::commit::Commit;
use crate::commit_builder::CommitBuilder;
use crate::conflicts;
use crate::evolution::{Evolution, MutableEvolution, ReadonlyEvolution};
use crate::op_store;
use crate::operation::Operation;
use crate::repo::{ReadonlyRepo, Repo};
use crate::settings::UserSettings;
use crate::store;
use crate::store::{CommitId, Timestamp};
use crate::store_wrapper::StoreWrapper;
use crate::view::{MutableView, ReadonlyView, View};
use std::ops::Deref;
use std::sync::Arc;

pub struct Transaction<'r> {
    repo: Option<Arc<MutableRepo<'r>>>,
    description: String,
    start_time: Timestamp,
    closed: bool,
}

pub struct MutableRepo<'r> {
    repo: &'r ReadonlyRepo,
    view: Option<MutableView>,
    evolution: Option<MutableEvolution<'static, 'static>>,
}

impl<'r> Transaction<'r> {
    pub fn new(
        repo: &'r ReadonlyRepo,
        view: &ReadonlyView,
        evolution: &ReadonlyEvolution<'r>,
        description: &str,
    ) -> Transaction<'r> {
        let mut_view = view.start_modification();
        let internal = Arc::new(MutableRepo {
            repo,
            view: Some(mut_view),
            evolution: None,
        });
        let repo_ref: &MutableRepo = internal.as_ref();
        let static_lifetime_repo: &'static MutableRepo = unsafe { std::mem::transmute(repo_ref) };
        let mut tx = Transaction {
            repo: Some(internal),
            description: description.to_owned(),
            start_time: Timestamp::now(),
            closed: false,
        };
        let mut_evolution: MutableEvolution<'_, '_> =
            evolution.start_modification(static_lifetime_repo);
        let static_lifetime_mut_evolution: MutableEvolution<'static, 'static> =
            unsafe { std::mem::transmute(mut_evolution) };
        Arc::get_mut(tx.repo.as_mut().unwrap()).unwrap().evolution =
            Some(static_lifetime_mut_evolution);
        tx
    }

    pub fn base_repo(&self) -> &'r ReadonlyRepo {
        self.repo.as_ref().unwrap().base_repo()
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        self.repo.as_ref().unwrap().store()
    }

    pub fn as_repo<'a: 'r>(&'a self) -> &(impl Repo + 'a) {
        self.repo.as_ref().unwrap().deref()
    }

    pub fn as_repo_mut(&mut self) -> &mut MutableRepo<'r> {
        Arc::get_mut(self.repo.as_mut().unwrap()).unwrap()
    }

    pub fn write_commit(&mut self, commit: store::Commit) -> Commit {
        let commit = self.store().write_commit(commit);
        self.add_head(&commit);
        commit
    }

    pub fn check_out(&mut self, settings: &UserSettings, commit: &Commit) -> Commit {
        let current_checkout_id = self.as_repo().view().checkout().clone();
        let current_checkout = self.store().get_commit(&current_checkout_id).unwrap();
        assert!(current_checkout.is_open(), "current checkout is closed");
        if current_checkout.is_empty()
            && !(current_checkout.is_pruned()
                || self.as_repo().evolution().is_obsolete(&current_checkout_id))
        {
            // Prune the checkout we're leaving if it's empty.
            // TODO: Also prune it if the only changes are conflicts that got materialized.
            CommitBuilder::for_rewrite_from(settings, self.store(), &current_checkout)
                .set_pruned(true)
                .write_to_transaction(self);
        }
        let store = self.store();
        // Create a new tree with any conflicts resolved.
        let mut tree_builder = store.tree_builder(commit.tree().id().clone());
        for (path, conflict_id) in commit.tree().conflicts() {
            let conflict = store.read_conflict(&conflict_id).unwrap();
            let materialized_value =
                conflicts::conflict_to_materialized_value(store, &path, &conflict);
            tree_builder.set(path, materialized_value);
        }
        let tree_id = tree_builder.write_tree();
        let open_commit;
        if !commit.is_open() || &tree_id != commit.tree().id() {
            // If the commit is closed, or if it had conflicts, create a new open commit on
            // top
            open_commit = CommitBuilder::for_open_commit(
                settings,
                self.store(),
                commit.id().clone(),
                tree_id,
            )
            .write_to_transaction(self);
        } else {
            // Otherwise the commit was open and didn't have any conflicts, so just use
            // that commit as is.
            open_commit = commit.clone();
        }
        let id = open_commit.id().clone();
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().set_checkout(id);
        open_commit
    }

    pub fn set_checkout(&mut self, id: CommitId) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().set_checkout(id);
    }

    pub fn add_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        // TODO: Also add other ancestors that were not already known. For example, when
        // the user creates a new repo backed by a git repo and they then check
        // out some commit, we'll need add all ancestors of that commit. This
        // code probably needs to be restructured a bit. Perhaps it should be
        // higher-level code that makes sure unknown ancestors are also added.
        mut_repo.view_mut().add_head(head);
        mut_repo.evolution_mut().add_commit(head);
    }

    pub fn remove_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().remove_head(head);
        mut_repo.evolution_mut().invalidate();
    }

    pub fn add_public_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().add_public_head(head);
        mut_repo.evolution_mut().add_commit(head);
    }

    pub fn remove_public_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().remove_public_head(head);
        mut_repo.evolution_mut().invalidate();
    }

    pub fn insert_git_ref(&mut self, name: String, commit_id: CommitId) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().insert_git_ref(name, commit_id);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().remove_git_ref(name);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.view_mut().set_view(data);
        mut_repo.evolution_mut().invalidate();
    }

    pub fn commit(mut self) -> Operation {
        let mut_repo = Arc::try_unwrap(self.repo.take().unwrap()).ok().unwrap();
        let view = mut_repo.take_view();
        let operation = view.save(self.description.clone(), self.start_time.clone());
        self.closed = true;
        operation
    }

    pub fn discard(mut self) {
        self.closed = true;
    }
}

impl<'r> Drop for Transaction<'r> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(self.closed, "Transaction was dropped without being closed.");
        }
    }
}

impl<'r> Repo for MutableRepo<'r> {
    fn store(&self) -> &Arc<StoreWrapper> {
        self.repo.store()
    }

    fn view(&self) -> &dyn View {
        self.view.as_ref().unwrap()
    }

    fn evolution(&self) -> &dyn Evolution {
        self.evolution.as_ref().unwrap()
    }
}

impl<'r> MutableRepo<'r> {
    pub fn base_repo(&self) -> &'r ReadonlyRepo {
        self.repo
    }

    pub fn view_mut(&mut self) -> &mut MutableView {
        self.view.as_mut().unwrap()
    }

    pub fn take_view(mut self) -> MutableView {
        self.view.take().unwrap()
    }

    pub fn evolution_mut<'m>(&'m mut self) -> &'m mut MutableEvolution<'r, 'm> {
        let evolution: &mut MutableEvolution<'static, 'static> = self.evolution.as_mut().unwrap();
        let evolution: &mut MutableEvolution<'r, 'm> = unsafe { std::mem::transmute(evolution) };
        evolution
    }
}
