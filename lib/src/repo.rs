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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::conflicts;
use thiserror::Error;

use crate::commit::Commit;
use crate::commit_builder::{new_change_id, signature, CommitBuilder};
use crate::dag_walk::topo_order_reverse;
use crate::evolution::{EvolutionRef, MutableEvolution, ReadonlyEvolution};
use crate::git_store::GitStore;
use crate::index::{IndexRef, MutableIndex, ReadonlyIndex};
use crate::index_store::IndexStore;
use crate::local_store::LocalStore;
use crate::op_heads_store::OpHeadsStore;
use crate::op_store;
use crate::op_store::{OpStore, OperationId};
use crate::operation::Operation;
use crate::settings::{RepoSettings, UserSettings};
use crate::simple_op_store::SimpleOpStore;
use crate::store;
use crate::store::{CommitId, Store, StoreError};
use crate::store_wrapper::StoreWrapper;
use crate::transaction::Transaction;
use crate::view::{MutableView, ReadonlyView, ViewRef};
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

    pub fn op_store(&self) -> &'a Arc<dyn OpStore> {
        match self {
            RepoRef::Readonly(repo) => repo.op_store(),
            RepoRef::Mutable(repo) => repo.op_store(),
        }
    }

    pub fn index(&self) -> IndexRef {
        match self {
            RepoRef::Readonly(repo) => IndexRef::Readonly(repo.index()),
            RepoRef::Mutable(repo) => IndexRef::Mutable(repo.index()),
        }
    }

    pub fn view(&self) -> ViewRef<'a> {
        match self {
            RepoRef::Readonly(repo) => ViewRef::Readonly(repo.view()),
            RepoRef::Mutable(repo) => ViewRef::Mutable(repo.view()),
        }
    }

    pub fn evolution(&self) -> EvolutionRef<'a, 'a, 'r> {
        match self {
            RepoRef::Readonly(repo) => EvolutionRef::Readonly(repo.evolution()),
            RepoRef::Mutable(repo) => EvolutionRef::Mutable(repo.evolution()),
        }
    }
}

pub struct ReadonlyRepo {
    repo_path: PathBuf,
    wc_path: PathBuf,
    store: Arc<StoreWrapper>,
    op_store: Arc<dyn OpStore>,
    op_heads_store: Arc<OpHeadsStore>,
    op_id: OperationId,
    settings: RepoSettings,
    index_store: Arc<IndexStore>,
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
        let store = Box::new(GitStore::load(&git_store_path));
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
        let store = Box::new(GitStore::load(&git_store_path));
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

        std::fs::create_dir(repo_path.join("op_store")).unwrap();
        let op_store: Arc<dyn OpStore> = Arc::new(SimpleOpStore::init(repo_path.join("op_store")));

        let op_heads_dir = repo_path.join("op_heads");
        std::fs::create_dir(&op_heads_dir).unwrap();
        let (op_heads_store, init_op_id, root_view) =
            OpHeadsStore::init(op_heads_dir, &op_store, checkout_commit.id().clone());
        let op_heads_store = Arc::new(op_heads_store);

        fs::create_dir(repo_path.join("index")).unwrap();
        let index_store = Arc::new(IndexStore::init(repo_path.join("index")));

        let view = ReadonlyView::new(store.clone(), root_view);

        let repo = ReadonlyRepo {
            repo_path,
            wc_path,
            store,
            op_store,
            op_heads_store,
            op_id: init_op_id,
            settings: repo_settings,
            index_store,
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

        repo.working_copy_locked()
            .check_out(checkout_commit)
            .expect("failed to check out root commit");
        repo
    }

    pub fn load(
        user_settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<Arc<ReadonlyRepo>, RepoLoadError> {
        ReadonlyRepo::loader(user_settings, wc_path)?.load_at_head()
    }

    pub fn loader(
        user_settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<RepoLoader, RepoLoadError> {
        RepoLoader::init(user_settings, wc_path)
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

    pub fn op_id(&self) -> &OperationId {
        &self.op_id
    }

    pub fn op(&self) -> Operation {
        let store_op = self.op_store.read_operation(&self.op_id).unwrap();
        Operation::new(self.op_store.clone(), self.op_id.clone(), store_op)
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
            let op_id = self.op_id.clone();
            let op = self.op_store.read_operation(&op_id).unwrap();
            let op = Operation::new(self.op_store.clone(), op_id, op);
            locked_index.replace(self.index_store.get_index_at_op(&op, self.store.as_ref()));
        }
        locked_index.as_ref().unwrap().clone()
    }

    pub fn reindex(&mut self) -> Arc<ReadonlyIndex> {
        self.index_store.reinit();
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

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        &self.op_store
    }

    pub fn op_heads_store(&self) -> &Arc<OpHeadsStore> {
        &self.op_heads_store
    }

    pub fn index_store(&self) -> &Arc<IndexStore> {
        &self.index_store
    }

    pub fn settings(&self) -> &RepoSettings {
        &self.settings
    }

    pub fn start_transaction(&self, description: &str) -> Transaction {
        let mut_repo = MutableRepo::new(
            self,
            self.index(),
            &self.view,
            &self.evolution.as_ref().unwrap(),
        );
        Transaction::new(mut_repo, description)
    }

    pub fn reload(&mut self) {
        let repo_loader = RepoLoader {
            wc_path: self.working_copy_path().clone(),
            repo_path: self.repo_path.clone(),
            repo_settings: self.settings.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            index_store: self.index_store.clone(),
        };
        let operation = self
            .op_heads_store
            .get_single_op_head(&repo_loader)
            .unwrap();
        self.op_id = operation.id().clone();
        self.view = ReadonlyView::new(self.store.clone(), operation.view().take_store_view());
        let repo_ref: &ReadonlyRepo = self;
        let static_lifetime_repo: &'static ReadonlyRepo = unsafe { std::mem::transmute(repo_ref) };
        {
            let mut locked_index = self.index.lock().unwrap();
            locked_index.take();
        }
        self.evolution = Some(ReadonlyEvolution::new(static_lifetime_repo));
    }

    pub fn reload_at(&mut self, operation: &Operation) {
        self.op_id = operation.id().clone();
        self.view = ReadonlyView::new(self.store.clone(), operation.view().take_store_view());
        let repo_ref: &ReadonlyRepo = self;
        let static_lifetime_repo: &'static ReadonlyRepo = unsafe { std::mem::transmute(repo_ref) };
        {
            let mut locked_index = self.index.lock().unwrap();
            locked_index.take();
        }
        self.evolution = Some(ReadonlyEvolution::new(static_lifetime_repo));
    }
}

pub struct RepoLoader {
    wc_path: PathBuf,
    repo_path: PathBuf,
    repo_settings: RepoSettings,
    store: Arc<StoreWrapper>,
    op_store: Arc<dyn OpStore>,
    op_heads_store: Arc<OpHeadsStore>,
    index_store: Arc<IndexStore>,
}

impl RepoLoader {
    fn init(user_settings: &UserSettings, wc_path: PathBuf) -> Result<RepoLoader, RepoLoadError> {
        let repo_path = wc_path.join(".jj");
        // TODO: Check if ancestor directory has a .jj/
        if !repo_path.is_dir() {
            return Err(RepoLoadError::NoRepoHere(wc_path));
        }
        let store = RepoLoader::load_store(&repo_path);
        let repo_settings = user_settings.with_repo(&repo_path).unwrap();
        let op_store: Arc<dyn OpStore> = Arc::new(SimpleOpStore::load(repo_path.join("op_store")));
        let op_heads_store = Arc::new(OpHeadsStore::load(repo_path.join("op_heads")));
        let index_store = Arc::new(IndexStore::load(repo_path.join("index")));
        Ok(RepoLoader {
            wc_path,
            repo_path,
            repo_settings,
            store,
            op_store,
            op_heads_store,
            index_store,
        })
    }

    // TODO: This probably belongs in StoreWrapper (once that type has a better
    // name)
    fn load_store(repo_path: &Path) -> Arc<StoreWrapper> {
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
            store = Box::new(GitStore::load(&git_store_path));
        }
        StoreWrapper::new(store)
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        &self.store
    }

    pub fn index_store(&self) -> &Arc<IndexStore> {
        &self.index_store
    }

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        &self.op_store
    }

    pub fn load_at_head(self) -> Result<Arc<ReadonlyRepo>, RepoLoadError> {
        let op = self.op_heads_store.get_single_op_head(&self).unwrap();
        let view = ReadonlyView::new(self.store.clone(), op.view().take_store_view());
        self._finish_load(op.id().clone(), view)
    }

    pub fn load_at(self, op: &Operation) -> Result<Arc<ReadonlyRepo>, RepoLoadError> {
        let view = ReadonlyView::new(self.store.clone(), op.view().take_store_view());
        self._finish_load(op.id().clone(), view)
    }

    fn _finish_load(
        self,
        op_id: OperationId,
        view: ReadonlyView,
    ) -> Result<Arc<ReadonlyRepo>, RepoLoadError> {
        let working_copy = WorkingCopy::load(
            self.store.clone(),
            self.wc_path.clone(),
            self.repo_path.join("working_copy"),
        );
        let repo = ReadonlyRepo {
            repo_path: self.repo_path,
            wc_path: self.wc_path,
            store: self.store,
            op_store: self.op_store,
            op_heads_store: self.op_heads_store,
            op_id,
            settings: self.repo_settings,
            index_store: self.index_store,
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
}

pub struct MutableRepo<'r> {
    repo: &'r ReadonlyRepo,
    index: MutableIndex,
    view: MutableView,
    evolution: Option<MutableEvolution<'static, 'static>>,
}

impl<'r> MutableRepo<'r> {
    pub fn new(
        repo: &'r ReadonlyRepo,
        index: Arc<ReadonlyIndex>,
        view: &ReadonlyView,
        evolution: &ReadonlyEvolution<'r>,
    ) -> Arc<MutableRepo<'r>> {
        let mut_view = view.start_modification();
        let mut_index = MutableIndex::incremental(index);
        let mut mut_repo = Arc::new(MutableRepo {
            repo,
            index: mut_index,
            view: mut_view,
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

    pub fn base_repo(&self) -> &'r ReadonlyRepo {
        self.repo
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        self.repo.store()
    }

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        self.repo.op_store()
    }

    pub fn index(&self) -> &MutableIndex {
        &self.index
    }

    pub fn view(&self) -> &MutableView {
        &self.view
    }

    pub fn consume(self) -> (MutableIndex, MutableView) {
        (self.index, self.view)
    }

    pub fn evolution<'m>(&'m self) -> &MutableEvolution<'r, 'm> {
        let evolution: &MutableEvolution<'static, 'static> = self.evolution.as_ref().unwrap();
        let evolution: &MutableEvolution<'r, 'm> = unsafe { std::mem::transmute(evolution) };
        evolution
    }

    pub fn write_commit(&mut self, commit: store::Commit) -> Commit {
        let commit = self.store().write_commit(commit);
        self.add_head(&commit);
        commit
    }

    pub fn set_checkout(&mut self, id: CommitId) {
        self.view.set_checkout(id);
    }

    pub fn check_out(&mut self, settings: &UserSettings, commit: &Commit) -> Commit {
        let current_checkout_id = self.view.checkout().clone();
        let current_checkout = self.store().get_commit(&current_checkout_id).unwrap();
        assert!(current_checkout.is_open(), "current checkout is closed");
        if current_checkout.is_empty()
            && !(current_checkout.is_pruned() || self.evolution().is_obsolete(&current_checkout_id))
        {
            // Prune the checkout we're leaving if it's empty.
            // TODO: Also prune it if the only changes are conflicts that got materialized.
            CommitBuilder::for_rewrite_from(settings, self.store(), &current_checkout)
                .set_pruned(true)
                .write_to_repo(self);
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
            .write_to_repo(self);
        } else {
            // Otherwise the commit was open and didn't have any conflicts, so just use
            // that commit as is.
            open_commit = commit.clone();
        }
        let id = open_commit.id().clone();
        self.view.set_checkout(id);
        open_commit
    }

    pub fn add_head(&mut self, head: &Commit) {
        let current_heads = self.view.heads();
        // Use incremental update for common case of adding a single commit on top a
        // current head. TODO: Also use incremental update when adding a single
        // commit on top a non-head.
        if head
            .parent_ids()
            .iter()
            .all(|parent_id| current_heads.contains(parent_id))
        {
            self.index.add_commit(head);
            self.view.add_head(head);
            self.evolution.as_mut().unwrap().add_commit(head);
        } else {
            let missing_commits = topo_order_reverse(
                vec![head.clone()],
                Box::new(|commit: &Commit| commit.id().clone()),
                Box::new(|commit: &Commit| -> Vec<Commit> {
                    commit
                        .parents()
                        .into_iter()
                        .filter(|parent| !self.index.has_id(parent.id()))
                        .collect()
                }),
            );
            for missing_commit in missing_commits.iter().rev() {
                self.index.add_commit(missing_commit);
            }
            self.view.add_head(head);
            self.evolution.as_mut().unwrap().invalidate();
        }
    }

    pub fn remove_head(&mut self, head: &Commit) {
        self.view.remove_head(head);
        self.evolution.as_mut().unwrap().invalidate();
    }

    pub fn add_public_head(&mut self, head: &Commit) {
        self.view.add_public_head(head);
        self.evolution.as_mut().unwrap().add_commit(head);
    }

    pub fn remove_public_head(&mut self, head: &Commit) {
        self.view.remove_public_head(head);
        self.evolution.as_mut().unwrap().invalidate();
    }

    pub fn insert_git_ref(&mut self, name: String, commit_id: CommitId) {
        self.view.insert_git_ref(name, commit_id);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        self.view.remove_git_ref(name);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        self.view.set_view(data);
        self.evolution.as_mut().unwrap().invalidate();
    }
}
