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

use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

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
use crate::op_store::{OpStore, OperationId, RefTarget};
use crate::operation::Operation;
use crate::settings::{RepoSettings, UserSettings};
use crate::simple_op_store::SimpleOpStore;
use crate::store::{CommitId, Store, StoreError};
use crate::store_wrapper::StoreWrapper;
use crate::transaction::Transaction;
use crate::view::View;
use crate::working_copy::WorkingCopy;
use crate::{conflicts, op_store, store};

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
pub enum RepoRef<'a> {
    Readonly(&'a ReadonlyRepo),
    Mutable(&'a MutableRepo),
}

impl<'a> RepoRef<'a> {
    pub fn store(&self) -> &Arc<StoreWrapper> {
        match self {
            RepoRef::Readonly(repo) => repo.store(),
            RepoRef::Mutable(repo) => repo.store(),
        }
    }

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        match self {
            RepoRef::Readonly(repo) => repo.op_store(),
            RepoRef::Mutable(repo) => repo.op_store(),
        }
    }

    pub fn index(&self) -> IndexRef<'a> {
        match self {
            RepoRef::Readonly(repo) => IndexRef::Readonly(repo.index()),
            RepoRef::Mutable(repo) => IndexRef::Mutable(repo.index()),
        }
    }

    pub fn view(&self) -> &View {
        match self {
            RepoRef::Readonly(repo) => repo.view(),
            RepoRef::Mutable(repo) => repo.view(),
        }
    }

    pub fn evolution(&self) -> EvolutionRef {
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
    operation: Operation,
    settings: RepoSettings,
    index_store: Arc<IndexStore>,
    index: Mutex<Option<Arc<ReadonlyIndex>>>,
    working_copy: Arc<Mutex<WorkingCopy>>,
    view: View,
    evolution: Mutex<Option<Arc<ReadonlyEvolution>>>,
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
pub enum RepoInitError {
    #[error("The destination repo ({0}) already exists")]
    DestinationExists(PathBuf),
}

#[derive(Error, Debug, PartialEq)]
pub enum RepoLoadError {
    #[error("There is no Jujutsu repo in {0}")]
    NoRepoHere(PathBuf),
}

impl ReadonlyRepo {
    pub fn init_local(
        settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<Arc<ReadonlyRepo>, RepoInitError> {
        let repo_path = ReadonlyRepo::init_repo_dir(&wc_path)?;
        let store_path = repo_path.join("store");
        fs::create_dir(&store_path).unwrap();
        let store = Box::new(LocalStore::init(store_path));
        Ok(ReadonlyRepo::init(settings, repo_path, wc_path, store))
    }

    /// Initializes a repo with a new Git store in .jj/git/ (bare Git repo)
    pub fn init_internal_git(
        settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<Arc<ReadonlyRepo>, RepoInitError> {
        let repo_path = ReadonlyRepo::init_repo_dir(&wc_path)?;
        let git_store_path = repo_path.join("git");
        git2::Repository::init_bare(&git_store_path).unwrap();
        let store_path = repo_path.join("store");
        let git_store_path = fs::canonicalize(git_store_path).unwrap();
        let mut store_file = File::create(store_path).unwrap();
        store_file.write_all(b"git: git").unwrap();
        let store = Box::new(GitStore::load(&git_store_path));
        Ok(ReadonlyRepo::init(settings, repo_path, wc_path, store))
    }

    /// Initializes a repo with an existing Git store at the specified path
    pub fn init_external_git(
        settings: &UserSettings,
        wc_path: PathBuf,
        git_store_path: PathBuf,
    ) -> Result<Arc<ReadonlyRepo>, RepoInitError> {
        let repo_path = ReadonlyRepo::init_repo_dir(&wc_path)?;
        let store_path = repo_path.join("store");
        let git_store_path = fs::canonicalize(git_store_path).unwrap();
        let mut store_file = File::create(store_path).unwrap();
        store_file
            .write_all(format!("git: {}", git_store_path.to_str().unwrap()).as_bytes())
            .unwrap();
        let store = Box::new(GitStore::load(&git_store_path));
        Ok(ReadonlyRepo::init(settings, repo_path, wc_path, store))
    }

    fn init_repo_dir(wc_path: &Path) -> Result<PathBuf, RepoInitError> {
        let repo_path = wc_path.join(".jj");
        if repo_path.exists() {
            Err(RepoInitError::DestinationExists(repo_path))
        } else {
            fs::create_dir(&repo_path).unwrap();
            Ok(repo_path)
        }
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
        let mut root_view = op_store::View::new(checkout_commit.id().clone());
        root_view.head_ids.insert(checkout_commit.id().clone());
        root_view
            .public_head_ids
            .insert(store.root_commit_id().clone());
        let (op_heads_store, init_op) = OpHeadsStore::init(op_heads_dir, &op_store, &root_view);
        let op_heads_store = Arc::new(op_heads_store);

        fs::create_dir(repo_path.join("index")).unwrap();
        let index_store = Arc::new(IndexStore::init(repo_path.join("index")));

        let view = View::new(root_view);

        let repo = ReadonlyRepo {
            repo_path,
            wc_path,
            store,
            op_store,
            op_heads_store,
            operation: init_op,
            settings: repo_settings,
            index_store,
            index: Mutex::new(None),
            working_copy: Arc::new(Mutex::new(working_copy)),
            view,
            evolution: Mutex::new(None),
        };
        let repo = Arc::new(repo);

        repo.working_copy_locked()
            .check_out(checkout_commit)
            .expect("failed to check out root commit");
        repo
    }

    pub fn load(
        user_settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<Arc<ReadonlyRepo>, RepoLoadError> {
        Ok(RepoLoader::init(user_settings, wc_path)?.load_at_head())
    }

    pub fn loader(&self) -> RepoLoader {
        RepoLoader {
            wc_path: self.wc_path.clone(),
            repo_path: self.repo_path.clone(),
            repo_settings: self.settings.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            index_store: self.index_store.clone(),
        }
    }

    pub fn as_repo_ref(&self) -> RepoRef {
        RepoRef::Readonly(self)
    }

    pub fn repo_path(&self) -> &PathBuf {
        &self.repo_path
    }

    pub fn working_copy_path(&self) -> &PathBuf {
        &self.wc_path
    }

    pub fn op_id(&self) -> &OperationId {
        self.operation.id()
    }

    pub fn operation(&self) -> &Operation {
        &self.operation
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn evolution(&self) -> Arc<ReadonlyEvolution> {
        let mut locked_evolution = self.evolution.lock().unwrap();
        if locked_evolution.is_none() {
            locked_evolution.replace(Arc::new(ReadonlyEvolution::new(self)));
        }
        locked_evolution.as_ref().unwrap().clone()
    }

    pub fn index(&self) -> &Arc<ReadonlyIndex> {
        let mut locked_index = self.index.lock().unwrap();
        if locked_index.is_none() {
            locked_index.replace(
                self.index_store
                    .get_index_at_op(&self.operation, self.store.as_ref()),
            );
        }
        let index: &Arc<ReadonlyIndex> = locked_index.as_ref().unwrap();
        // Extend lifetime from that of mutex lock to that of self. Safe since we never
        // change value once it's been set (except in `reindex()` but that
        // requires a mutable reference).
        let index: &Arc<ReadonlyIndex> = unsafe { std::mem::transmute(index) };
        index
    }

    pub fn reindex(&mut self) -> &Arc<ReadonlyIndex> {
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

    pub fn start_transaction(self: &Arc<ReadonlyRepo>, description: &str) -> Transaction {
        let locked_evolution = self.evolution.lock().unwrap();
        let mut_repo = MutableRepo::new(
            self.clone(),
            self.index().clone(),
            &self.view,
            locked_evolution.as_ref(),
        );
        Transaction::new(mut_repo, description)
    }

    pub fn reload(&self) -> Arc<ReadonlyRepo> {
        self.loader().load_at_head()
    }

    pub fn reload_at(&self, operation: &Operation) -> Arc<ReadonlyRepo> {
        self.loader().load_at(operation)
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

fn find_repo_dir(mut wc_dir: &Path) -> Option<PathBuf> {
    loop {
        let repo_path = wc_dir.join(".jj");
        if repo_path.is_dir() {
            return Some(repo_path);
        }
        if let Some(wc_dir_parent) = wc_dir.parent() {
            wc_dir = wc_dir_parent;
        } else {
            return None;
        }
    }
}

impl RepoLoader {
    pub fn init(
        user_settings: &UserSettings,
        wc_path: PathBuf,
    ) -> Result<RepoLoader, RepoLoadError> {
        let repo_path = find_repo_dir(&wc_path).ok_or(RepoLoadError::NoRepoHere(wc_path))?;
        let wc_path = repo_path.parent().unwrap().to_owned();
        let store = StoreWrapper::load_store(&repo_path);
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

    pub fn store(&self) -> &Arc<StoreWrapper> {
        &self.store
    }

    pub fn index_store(&self) -> &Arc<IndexStore> {
        &self.index_store
    }

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        &self.op_store
    }

    pub fn op_heads_store(&self) -> &Arc<OpHeadsStore> {
        &self.op_heads_store
    }

    pub fn load_at_head(&self) -> Arc<ReadonlyRepo> {
        let op = self.op_heads_store.get_single_op_head(self).unwrap();
        let view = View::new(op.view().take_store_view());
        self._finish_load(op, view)
    }

    pub fn load_at(&self, op: &Operation) -> Arc<ReadonlyRepo> {
        let view = View::new(op.view().take_store_view());
        self._finish_load(op.clone(), view)
    }

    pub fn create_from(
        &self,
        operation: Operation,
        view: View,
        working_copy: Arc<Mutex<WorkingCopy>>,
        index: Arc<ReadonlyIndex>,
        evolution: Option<Arc<ReadonlyEvolution>>,
    ) -> Arc<ReadonlyRepo> {
        let repo = ReadonlyRepo {
            repo_path: self.repo_path.clone(),
            wc_path: self.wc_path.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            operation,
            settings: self.repo_settings.clone(),
            index_store: self.index_store.clone(),
            index: Mutex::new(Some(index)),
            working_copy,
            view,
            evolution: Mutex::new(evolution),
        };
        Arc::new(repo)
    }

    fn _finish_load(&self, operation: Operation, view: View) -> Arc<ReadonlyRepo> {
        let working_copy = WorkingCopy::load(
            self.store.clone(),
            self.wc_path.clone(),
            self.repo_path.join("working_copy"),
        );
        let repo = ReadonlyRepo {
            repo_path: self.repo_path.clone(),
            wc_path: self.wc_path.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            operation,
            settings: self.repo_settings.clone(),
            index_store: self.index_store.clone(),
            index: Mutex::new(None),
            working_copy: Arc::new(Mutex::new(working_copy)),
            view,
            evolution: Mutex::new(None),
        };
        Arc::new(repo)
    }
}

pub struct MutableRepo {
    base_repo: Arc<ReadonlyRepo>,
    index: MutableIndex,
    view: View,
    evolution: Mutex<Option<MutableEvolution>>,
}

impl MutableRepo {
    pub fn new(
        base_repo: Arc<ReadonlyRepo>,
        index: Arc<ReadonlyIndex>,
        view: &View,
        evolution: Option<&Arc<ReadonlyEvolution>>,
    ) -> Arc<MutableRepo> {
        let mut_view = view.start_modification();
        let mut_index = MutableIndex::incremental(index);
        let mut_evolution = evolution.map(|evolution| evolution.start_modification());
        Arc::new(MutableRepo {
            base_repo,
            index: mut_index,
            view: mut_view,
            evolution: Mutex::new(mut_evolution),
        })
    }

    pub fn as_repo_ref(&self) -> RepoRef {
        RepoRef::Mutable(self)
    }

    pub fn base_repo(&self) -> &Arc<ReadonlyRepo> {
        &self.base_repo
    }

    pub fn store(&self) -> &Arc<StoreWrapper> {
        self.base_repo.store()
    }

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        self.base_repo.op_store()
    }

    pub fn index(&self) -> &MutableIndex {
        &self.index
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn consume(self) -> (MutableIndex, View, Option<MutableEvolution>) {
        (self.index, self.view, self.evolution.lock().unwrap().take())
    }

    pub fn evolution(&self) -> &MutableEvolution {
        let mut locked_evolution = self.evolution.lock().unwrap();
        if locked_evolution.is_none() {
            locked_evolution.replace(MutableEvolution::new(self));
        }
        let evolution = locked_evolution.as_ref().unwrap();
        // Extend lifetime from lifetime of MutexGuard to lifetime of self. Safe because
        // the value won't change again except for by invalidate_evolution(), which
        // requires a mutable reference.
        unsafe { std::mem::transmute(evolution) }
    }

    pub fn evolution_mut(&mut self) -> Option<&mut MutableEvolution> {
        let mut locked_evolution = self.evolution.lock().unwrap();
        let maybe_evolution = locked_evolution.as_mut();
        // Extend lifetime from lifetime of MutexGuard to lifetime of self. Safe because
        // the value won't change again except for by invalidate_evolution(), which
        // requires a mutable reference.
        unsafe { std::mem::transmute(maybe_evolution) }
    }

    pub fn invalidate_evolution(&mut self) {
        self.evolution.lock().unwrap().take();
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

    pub fn heads(&self) -> &HashSet<CommitId> {
        self.view.heads()
    }

    fn enforce_view_invariants(&mut self) {
        let view = self.view.store_view_mut();
        // TODO: This is surely terribly slow on large repos, at least in its current
        // form. We should avoid calling it in most cases (avoid adding a head that's
        // already reachable in the view).
        view.public_head_ids = self
            .index
            .heads(view.public_head_ids.iter())
            .iter()
            .cloned()
            .collect();
        view.head_ids.extend(view.public_head_ids.iter().cloned());
        for ref_target in view.git_refs.values() {
            match ref_target {
                RefTarget::Normal(id) => {
                    view.head_ids.insert(id.clone());
                }
                RefTarget::Conflict { removes, adds } => {
                    view.head_ids.extend(removes.iter().cloned());
                    view.head_ids.extend(adds.iter().cloned());
                }
            }
        }
        view.head_ids = self
            .index
            .heads(view.head_ids.iter())
            .iter()
            .cloned()
            .collect();
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
            self.view.add_head(head.id());
            for parent_id in head.parent_ids() {
                self.view.remove_head(&parent_id);
            }
            if let Some(evolution) = self.evolution_mut() {
                evolution.add_commit(head)
            }
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
            self.view.add_head(head.id());
            self.enforce_view_invariants();
            self.invalidate_evolution();
        }
    }

    pub fn remove_head(&mut self, head: &Commit) {
        self.view.remove_head(head.id());
        self.enforce_view_invariants();
        self.invalidate_evolution();
    }

    pub fn add_public_head(&mut self, head: &Commit) {
        self.view.add_public_head(head.id());
        self.enforce_view_invariants();
        if let Some(evolution) = self.evolution_mut() {
            evolution.add_commit(head)
        }
    }

    pub fn remove_public_head(&mut self, head: &Commit) {
        self.view.remove_public_head(head.id());
        self.invalidate_evolution();
    }

    pub fn insert_git_ref(&mut self, name: String, target: RefTarget) {
        self.view.insert_git_ref(name, target);
    }

    pub fn remove_git_ref(&mut self, name: &str) {
        self.view.remove_git_ref(name);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        self.view.set_view(data);
        self.enforce_view_invariants();
        self.invalidate_evolution();
    }

    pub fn merge(&mut self, base_repo: &ReadonlyRepo, other_repo: &ReadonlyRepo) {
        // First, merge the index, so we can take advantage of a valid index when
        // merging the view. Merging in base_repo's index isn't typically
        // necessary, but it can be if base_repo is ahead of either self or other_repo
        // (e.g. because we're undoing an operation that hasn't been published).
        self.index.merge_in(base_repo.index());
        self.index.merge_in(other_repo.index());

        self.view
            .merge(self.index.as_index_ref(), &base_repo.view, &other_repo.view);
        self.enforce_view_invariants();

        self.invalidate_evolution();
    }
}
