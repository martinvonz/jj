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
use crate::evolution::MutableEvolution;
use crate::index::MutableIndex;
use crate::op_store;
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo, RepoRef};
use crate::settings::UserSettings;
use crate::store;
use crate::store::{CommitId, Timestamp};
use crate::store_wrapper::StoreWrapper;
use crate::view::MutableView;
use std::sync::Arc;

pub struct Transaction<'r> {
    repo: Option<Arc<MutableRepo<'r>>>,
    description: String,
    start_time: Timestamp,
    closed: bool,
}

impl<'r> Transaction<'r> {
    pub fn new(mut_repo: Arc<MutableRepo<'r>>, description: &str) -> Transaction<'r> {
        Transaction {
            repo: Some(mut_repo),
            description: description.to_owned(),
            start_time: Timestamp::now(),
            closed: false,
        }
    }

    pub fn base_repo(&self) -> &'r ReadonlyRepo {
        self.repo.as_ref().unwrap().base_repo()
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        self.repo.as_ref().unwrap().store()
    }

    pub fn as_repo_ref(&self) -> RepoRef {
        self.repo.as_ref().unwrap().as_repo_ref()
    }

    pub fn as_repo_mut(&mut self) -> &mut MutableRepo<'r> {
        Arc::get_mut(self.repo.as_mut().unwrap()).unwrap()
    }

    pub fn index(&self) -> &MutableIndex {
        self.repo.as_ref().unwrap().index()
    }

    pub fn view(&self) -> &MutableView {
        self.repo.as_ref().unwrap().view()
    }

    pub fn evolution(&self) -> &MutableEvolution {
        self.repo.as_ref().unwrap().evolution()
    }

    pub fn write_commit(&mut self, commit: store::Commit) -> Commit {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.write_commit(commit)
    }

    pub fn check_out(&mut self, settings: &UserSettings, commit: &Commit) -> Commit {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.check_out(settings, commit)
    }

    pub fn set_checkout(&mut self, id: CommitId) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.set_checkout(id);
    }

    pub fn add_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.add_head(head)
    }

    pub fn remove_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.remove_head(head)
    }

    pub fn add_public_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.add_public_head(head)
    }

    pub fn remove_public_head(&mut self, head: &Commit) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.remove_public_head(head);
    }

    pub fn insert_git_ref(&mut self, name: String, commit_id: CommitId) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.insert_git_ref(name, commit_id);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.remove_git_ref(name);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        let mut_repo = Arc::get_mut(self.repo.as_mut().unwrap()).unwrap();
        mut_repo.set_view(data);
    }

    pub fn commit(mut self) -> Operation {
        let mut_repo = Arc::try_unwrap(self.repo.take().unwrap()).ok().unwrap();
        let base_repo = mut_repo.base_repo();
        let index_store = base_repo.index_store();
        let (mut_index, mut_view) = mut_repo.consume();
        let index = index_store.write_index(mut_index).unwrap();
        let operation = mut_view.save(self.description.clone(), self.start_time.clone());
        index_store
            .associate_file_with_operation(&index, operation.id())
            .unwrap();
        base_repo.op_heads_store().update_op_heads(&operation);
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
            debug_assert!(self.closed, "Transaction was dropped without being closed.");
        }
    }
}
