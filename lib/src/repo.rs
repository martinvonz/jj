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
use crate::evolution::{Evolution, MutableEvolution, ReadonlyEvolution};
use crate::git_store::GitStore;
use crate::index::ReadonlyIndex;
use crate::local_store::LocalStore;
use crate::operation::Operation;
use crate::settings::{RepoSettings, UserSettings};
use crate::store;
use crate::store::{Store, StoreError};
use crate::store_wrapper::StoreWrapper;
use crate::transaction::Transaction;
use crate::view::{MutableView, ReadonlyView, View};
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

// TODO: Should we implement From<&ReadonlyRepo> and From<&MutableRepo> for
// RepoRef?
#[derive(Clone, Copy)]
pub enum RepoRef<'a, 'r: 'a> {
    Readonly(&'a ReadonlyRepo),
    Mutable(&'a MutableRepo<'r>),
}

impl<'a, 'r> RepoRef<'a, 'r> {
    pub fn store(&self) -> &'a Arc<StoreWrapper> {
        match self {
            RepoRef::Readonly(repo) => repo.store(),
            RepoRef::Mutable(repo) => repo.store(),
        }
    }

    pub fn view(&self) -> &'a dyn View {
        match self {
            RepoRef::Readonly(repo) => repo.view(),
            RepoRef::Mutable(repo) => repo.view(),
        }
    }

    pub fn evolution(&self) -> &'a dyn Evolution {
        match self {
            RepoRef::Readonly(repo) => repo.evolution(),
            RepoRef::Mutable(repo) => repo.evolution(),
        }
    }
}

pub struct ReadonlyRepo {
    repo_path: PathBuf,
    wc_path: PathBuf,
    store: Arc<StoreWrapper>,
    settings: RepoSettings,
    index: Mutex<Option<Arc<ReadonlyIndex>>>,
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

#[derive(Error, Debug, PartialEq)]
pub enum RepoLoadError {
    #[error("There is no Jujube repo in {0}")]
    NoRepoHere(PathBuf),
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

    /// Initializes a repo with a new Git store in .jj/git/ (bare Git repo)
    pub fn init_internal_git(settings: &UserSettings, wc_path: PathBuf) -> Arc<ReadonlyRepo> {
        let repo_path = wc_path.join(".jj");
        fs::create_dir(repo_path.clone()).unwrap();
        let git_store_path = repo_path.join("git");
        git2::Repository::init_bare(&git_store_path).unwrap();
        let store_path = repo_path.join("store");
        let git_store_path = fs::canonicalize(git_store_path).unwrap();
        let mut store_file = File::create(store_path).unwrap();
        store_file.write_all(b"git: git").unwrap();
        let store = Box::new(GitStore::load(git_store_path));
        ReadonlyRepo::init(settings, repo_path, wc_path, store)
    }

    /// Initializes a repo with an existing Git store at the specified path
    pub fn init_external_git(
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
            .write_all(format!("git: {}", git_store_path.to_str().unwrap()).as_bytes())
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
        let working_copy = WorkingCopy::init(
            store.clone(),
            wc_path.clone(),
            repo_path.join("working_copy"),
        );

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
        ReadonlyIndex::init(repo_path.join("index"));

        let evolution = ReadonlyEvolution::new(static_lifetime_repo);
        Arc::get_mut(&mut repo).unwrap().evolution = Some(evolution);

        repo.working_copy_locked()
            .check_out(checkout_commit)
            .expect("failed to check out root commit");
        repo
    }

    pub fn load(
        user_settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<Arc<ReadonlyRepo>, RepoLoadError> {
        let repo_path = wc_path.join(".jj");
        // TODO: Check if ancestor directory has a .jj/
        if !repo_path.is_dir() {
            return Err(RepoLoadError::NoRepoHere(wc_path));
        }
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
            let git_store_path =
                fs::canonicalize(repo_path.join(PathBuf::from(git_store_path_str))).unwrap();
            store = Box::new(GitStore::load(git_store_path));
        }
        let store = StoreWrapper::new(store);
        let repo_settings = user_settings.with_repo(&repo_path).unwrap();
        let working_copy = WorkingCopy::load(
            store.clone(),
            wc_path.clone(),
            repo_path.join("working_copy"),
        );
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
        Arc::get_mut(&mut repo).unwrap().evolution = Some(evolution);
        Ok(repo)
    }

    pub fn as_repo_ref(&self) -> RepoRef {
        RepoRef::Readonly(&self)
    }

    pub fn repo_path(&self) -> &PathBuf {
        &self.repo_path
    }

    pub fn working_copy_path(&self) -> &PathBuf {
        &self.wc_path
    }

    pub fn view(&self) -> &ReadonlyView {
        &self.view
    }

    pub fn evolution<'a>(&'a self) -> &ReadonlyEvolution<'a> {
        let evolution: &ReadonlyEvolution<'static> = self.evolution.as_ref().unwrap();
        let evolution: &ReadonlyEvolution<'a> = unsafe { std::mem::transmute(evolution) };
        evolution
    }

    pub fn index(&self) -> Arc<ReadonlyIndex> {
        let mut locked_index = self.index.lock().unwrap();
        if locked_index.is_none() {
            let op_id = self.view.base_op_head_id().clone();
            locked_index.replace(ReadonlyIndex::load(
                self,
                self.repo_path.join("index"),
                op_id,
            ));
        }
        locked_index.as_ref().unwrap().clone()
    }

    pub fn reindex(&mut self) -> Arc<ReadonlyIndex> {
        ReadonlyIndex::reinit(self.repo_path.join("index"));
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
        let mut_repo = MutableRepo::new(self, &self.view, &self.evolution.as_ref().unwrap());
        Transaction::new(mut_repo, description)
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

pub struct MutableRepo<'r> {
    repo: &'r ReadonlyRepo,
    view: Option<MutableView>,
    evolution: Option<MutableEvolution<'static, 'static>>,
}

impl<'r> MutableRepo<'r> {
    pub fn new(
        repo: &'r ReadonlyRepo,
        view: &ReadonlyView,
        evolution: &ReadonlyEvolution<'r>,
    ) -> Arc<MutableRepo<'r>> {
        let mut_view = view.start_modification();
        let mut mut_repo = Arc::new(MutableRepo {
            repo,
            view: Some(mut_view),
            evolution: None,
        });
        let repo_ref: &MutableRepo = mut_repo.as_ref();
        let static_lifetime_repo: &'static MutableRepo = unsafe { std::mem::transmute(repo_ref) };
        let mut_evolution: MutableEvolution<'_, '_> =
            evolution.start_modification(static_lifetime_repo);
        let static_lifetime_mut_evolution: MutableEvolution<'static, 'static> =
            unsafe { std::mem::transmute(mut_evolution) };
        Arc::get_mut(&mut mut_repo).unwrap().evolution = Some(static_lifetime_mut_evolution);
        mut_repo
    }

    pub fn as_repo_ref(&self) -> RepoRef {
        RepoRef::Mutable(&self)
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        self.repo.store()
    }

    pub fn base_repo(&self) -> &'r ReadonlyRepo {
        self.repo
    }

    pub fn view(&self) -> &dyn View {
        self.view.as_ref().unwrap()
    }

    pub fn view_mut(&mut self) -> &mut MutableView {
        self.view.as_mut().unwrap()
    }

    pub fn take_view(mut self) -> MutableView {
        self.view.take().unwrap()
    }

    pub fn evolution(&self) -> &dyn Evolution {
        self.evolution.as_ref().unwrap()
    }

    pub fn evolution_mut<'m>(&'m mut self) -> &'m mut MutableEvolution<'r, 'm> {
        let evolution: &mut MutableEvolution<'static, 'static> = self.evolution.as_mut().unwrap();
        let evolution: &mut MutableEvolution<'r, 'm> = unsafe { std::mem::transmute(evolution) };
        evolution
    }
}
