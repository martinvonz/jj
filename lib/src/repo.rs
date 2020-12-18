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

use std::fmt::{Debug, Formatter};
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use thiserror::Error;

use crate::commit_builder::{new_change_id, signature};
use crate::evolution::{Evolution, ReadonlyEvolution};
use crate::git_store::GitStore;
use crate::index::IndexFile;
use crate::local_store::LocalStore;
use crate::operation::Operation;
use crate::settings::{RepoSettings, UserSettings};
use crate::store;
use crate::store::{Store, StoreError};
use crate::store_wrapper::StoreWrapper;
use crate::transaction::Transaction;
use crate::view::{ReadonlyView, View};
use crate::working_copy::WorkingCopy;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RepoError {
    #[error("Object not found")]
    NotFound,
    #[error("Error: {0}")]
    Other(String),
}

impl From<StoreError> for RepoError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::NotFound => RepoError::NotFound,
            StoreError::Other(description) => RepoError::Other(description),
        }
    }
}

pub type RepoResult<T> = Result<T, RepoError>;

pub trait Repo: Sync {
    fn store(&self) -> &Arc<StoreWrapper>;
    fn view(&self) -> &dyn View;
    fn evolution(&self) -> &dyn Evolution;
}

pub struct ReadonlyRepo {
    repo_path: PathBuf,
    wc_path: PathBuf,
    store: Arc<StoreWrapper>,
    settings: RepoSettings,
    index: Mutex<Option<Arc<IndexFile>>>,
    working_copy: Arc<Mutex<WorkingCopy>>,
    view: ReadonlyView,
    evolution: Option<ReadonlyEvolution<'static>>,
}

impl Debug for ReadonlyRepo {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Repo")
            .field("repo_path", &self.repo_path)
            .field("wc_path", &self.wc_path)
            .field("store", &self.store)
            .finish()
    }
}

impl ReadonlyRepo {
    pub fn init_local(settings: &UserSettings, wc_path: PathBuf) -> Arc<ReadonlyRepo> {
        let repo_path = wc_path.join(".jj");
        fs::create_dir(repo_path.clone()).unwrap();
        let store_path = repo_path.join("store");
        fs::create_dir(&store_path).unwrap();
        let store = Box::new(LocalStore::init(store_path));
        ReadonlyRepo::init(settings, repo_path, wc_path, store)
    }

    pub fn init_git(
        settings: &UserSettings,
        wc_path: PathBuf,
        git_store_path: PathBuf,
    ) -> Arc<ReadonlyRepo> {
        let repo_path = wc_path.join(".jj");
        fs::create_dir(repo_path.clone()).unwrap();
        let store_path = repo_path.join("store");
        let git_store_path = fs::canonicalize(git_store_path).unwrap();
        let mut store_file = File::create(store_path).unwrap();
        store_file
            .write_all((String::from("git: ") + git_store_path.to_str().unwrap()).as_bytes())
            .unwrap();
        let store = Box::new(GitStore::load(git_store_path));
        ReadonlyRepo::init(settings, repo_path, wc_path, store)
    }

    fn init(
        user_settings: &UserSettings,
        repo_path: PathBuf,
        wc_path: PathBuf,
        store: Box<dyn Store>,
    ) -> Arc<ReadonlyRepo> {
        let repo_settings = user_settings.with_repo(&repo_path).unwrap();
        let store = StoreWrapper::new(store);

        fs::create_dir(repo_path.join("working_copy")).unwrap();
        let working_copy = WorkingCopy::init(store.clone(), repo_path.join("working_copy"));

        fs::create_dir(repo_path.join("view")).unwrap();
        let signature = signature(user_settings);
        let checkout_commit = store::Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: store.empty_tree_id().clone(),
            change_id: new_change_id(),
            description: "".to_string(),
            author: signature.clone(),
            committer: signature,
            is_open: true,
            is_pruned: false,
        };
        let checkout_commit = store.write_commit(checkout_commit);
        let view = ReadonlyView::init(
            store.clone(),
            repo_path.join("view"),
            checkout_commit.id().clone(),
        );

        let repo = ReadonlyRepo {
            repo_path: repo_path.clone(),
            wc_path,
            store,
            settings: repo_settings,
            index: Mutex::new(None),
            working_copy: Arc::new(Mutex::new(working_copy)),
            view,
            evolution: None,
        };
        let mut repo = Arc::new(repo);
        let repo_ref: &ReadonlyRepo = repo.as_ref();
        let static_lifetime_repo: &'static ReadonlyRepo = unsafe { std::mem::transmute(repo_ref) };

        fs::create_dir(repo_path.join("index")).unwrap();
        IndexFile::init(repo_path.join("index"));

        let evolution = ReadonlyEvolution::new(static_lifetime_repo);

        ReadonlyRepo::init_cycles(&mut repo, evolution);
        repo.working_copy_locked()
            .check_out(&repo, checkout_commit)
            .expect("failed to check out root commit");
        repo
    }

    pub fn load(user_settings: &UserSettings, wc_path: PathBuf) -> Arc<ReadonlyRepo> {
        let repo_path = wc_path.join(".jj");
        let store_path = repo_path.join("store");
        let store: Box<dyn Store>;
        if store_path.is_dir() {
            store = Box::new(LocalStore::load(store_path));
        } else {
            let mut store_file = File::open(store_path).unwrap();
            let mut buf = Vec::new();
            store_file.read_to_end(&mut buf).unwrap();
            let contents = String::from_utf8(buf).unwrap();
            assert!(contents.starts_with("git: "));
            let git_store_path_str = contents[5..].to_string();
            let git_store_path = PathBuf::from(git_store_path_str);
            store = Box::new(GitStore::load(git_store_path));
        }
        let store = StoreWrapper::new(store);
        let repo_settings = user_settings.with_repo(&repo_path).unwrap();
        let working_copy = WorkingCopy::load(store.clone(), repo_path.join("working_copy"));
        let view = ReadonlyView::load(store.clone(), repo_path.join("view"));
        let repo = ReadonlyRepo {
            repo_path,
            wc_path,
            store,
            settings: repo_settings,
            index: Mutex::new(None),
            working_copy: Arc::new(Mutex::new(working_copy)),
            view,
            evolution: None,
        };
        let mut repo = Arc::new(repo);
        let repo_ref: &ReadonlyRepo = repo.as_ref();
        let static_lifetime_repo: &'static ReadonlyRepo = unsafe { std::mem::transmute(repo_ref) };
        let evolution = ReadonlyEvolution::new(static_lifetime_repo);
        ReadonlyRepo::init_cycles(&mut repo, evolution);
        repo
    }

    fn init_cycles(mut repo: &mut Arc<ReadonlyRepo>, evolution: ReadonlyEvolution<'static>) {
        let mut repo_ref_mut = Arc::get_mut(&mut repo).unwrap();
        repo_ref_mut.evolution = Some(evolution);
    }

    pub fn repo_path(&self) -> &PathBuf {
        &self.repo_path
    }

    pub fn working_copy_path(&self) -> &PathBuf {
        &self.wc_path
    }

    pub fn index(&self) -> Arc<IndexFile> {
        let mut locked_index = self.index.lock().unwrap();
        if locked_index.is_none() {
            let op_id = self.view.base_op_head_id().clone();
            locked_index.replace(IndexFile::load(self, self.repo_path.join("index"), op_id));
        }
        locked_index.as_ref().unwrap().clone()
    }

    pub fn reindex(&mut self) -> Arc<IndexFile> {
        IndexFile::reinit(self.repo_path.join("index"));
        {
            let mut locked_index = self.index.lock().unwrap();
            locked_index.take();
        }
        self.index()
    }

    pub fn working_copy(&self) -> &Arc<Mutex<WorkingCopy>> {
        &self.working_copy
    }

    pub fn working_copy_locked(&self) -> MutexGuard<WorkingCopy> {
        self.working_copy.as_ref().lock().unwrap()
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        &self.store
    }

    pub fn settings(&self) -> &RepoSettings {
        &self.settings
    }

    pub fn start_transaction(&self, description: &str) -> Transaction {
        Transaction::new(
            &self,
            &self.view,
            &self.evolution.as_ref().unwrap(),
            description,
        )
    }

    pub fn reload(&mut self) {
        self.view.reload();
        let repo_ref: &ReadonlyRepo = self;
        let static_lifetime_repo: &'static ReadonlyRepo = unsafe { std::mem::transmute(repo_ref) };
        {
            let mut locked_index = self.index.lock().unwrap();
            locked_index.take();
        }
        self.evolution = Some(ReadonlyEvolution::new(static_lifetime_repo));
    }

    pub fn reload_at(&mut self, operation: &Operation) {
        self.view.reload_at(operation);
        let repo_ref: &ReadonlyRepo = self;
        let static_lifetime_repo: &'static ReadonlyRepo = unsafe { std::mem::transmute(repo_ref) };
        {
            let mut locked_index = self.index.lock().unwrap();
            locked_index.take();
        }
        self.evolution = Some(ReadonlyEvolution::new(static_lifetime_repo));
    }
}

impl Repo for ReadonlyRepo {
    fn store(&self) -> &Arc<StoreWrapper> {
        &self.store
    }

    fn view(&self) -> &dyn View {
        &self.view
    }

    fn evolution(&self) -> &dyn Evolution {
        self.evolution.as_ref().unwrap()
    }
}
