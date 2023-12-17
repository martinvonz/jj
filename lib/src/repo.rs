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

use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::io::ErrorKind;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, slice};

use itertools::Itertools;
use once_cell::sync::OnceCell;
use thiserror::Error;
use tracing::instrument;

use self::dirty_cell::DirtyCell;
use crate::backend::{
    Backend, BackendError, BackendInitError, BackendLoadError, BackendResult, ChangeId, CommitId,
    MergedTreeId, ObjectId, SigningFn,
};
use crate::commit::{Commit, CommitByCommitterTimestamp};
use crate::commit_builder::CommitBuilder;
use crate::default_index::DefaultIndexStore;
use crate::default_submodule_store::DefaultSubmoduleStore;
use crate::file_util::{IoResultExt as _, PathError};
use crate::git_backend::GitBackend;
use crate::index::{HexPrefix, Index, IndexStore, MutableIndex, PrefixResolution, ReadonlyIndex};
use crate::local_backend::LocalBackend;
use crate::op_heads_store::{self, OpHeadResolutionError, OpHeadsStore};
use crate::op_store::{
    OpStore, OpStoreError, OperationId, RefTarget, RemoteRef, RemoteRefState, WorkspaceId,
};
use crate::operation::Operation;
use crate::refs::{
    diff_named_ref_targets, diff_named_remote_refs, merge_ref_targets, merge_remote_refs,
};
use crate::revset::{ChangeIdIndex, RevsetExpression};
use crate::rewrite::{DescendantRebaser, RebaseOptions};
use crate::settings::{RepoSettings, UserSettings};
use crate::signing::{SignInitError, Signer};
use crate::simple_op_heads_store::SimpleOpHeadsStore;
use crate::simple_op_store::SimpleOpStore;
use crate::store::Store;
use crate::submodule_store::SubmoduleStore;
use crate::transaction::Transaction;
use crate::tree::TreeMergeError;
use crate::view::View;
use crate::{backend, dag_walk, op_store, revset};

pub trait Repo {
    fn store(&self) -> &Arc<Store>;

    fn op_store(&self) -> &Arc<dyn OpStore>;

    fn index(&self) -> &dyn Index;

    fn view(&self) -> &View;

    fn submodule_store(&self) -> &Arc<dyn SubmoduleStore>;

    fn resolve_change_id(&self, change_id: &ChangeId) -> Option<Vec<CommitId>> {
        // Replace this if we added more efficient lookup method.
        let prefix = HexPrefix::from_bytes(change_id.as_bytes());
        match self.resolve_change_id_prefix(&prefix) {
            PrefixResolution::NoMatch => None,
            PrefixResolution::SingleMatch(entries) => Some(entries),
            PrefixResolution::AmbiguousMatch => panic!("complete change_id should be unambiguous"),
        }
    }

    fn resolve_change_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<Vec<CommitId>>;

    fn shortest_unique_change_id_prefix_len(&self, target_id_bytes: &ChangeId) -> usize;
}

pub struct ReadonlyRepo {
    repo_path: PathBuf,
    store: Arc<Store>,
    op_store: Arc<dyn OpStore>,
    op_heads_store: Arc<dyn OpHeadsStore>,
    operation: Operation,
    settings: RepoSettings,
    index_store: Arc<dyn IndexStore>,
    submodule_store: Arc<dyn SubmoduleStore>,
    index: OnceCell<Box<dyn ReadonlyIndex>>,
    change_id_index: OnceCell<Box<dyn ChangeIdIndex>>,
    // TODO: This should eventually become part of the index and not be stored fully in memory.
    view: View,
}

impl Debug for ReadonlyRepo {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Repo")
            .field("repo_path", &self.repo_path)
            .field("store", &self.store)
            .finish()
    }
}

#[derive(Error, Debug)]
pub enum RepoInitError {
    #[error(transparent)]
    Backend(#[from] BackendInitError),
    #[error(transparent)]
    Path(#[from] PathError),
}

impl ReadonlyRepo {
    pub fn default_op_store_initializer() -> &'static OpStoreInitializer<'static> {
        &|_settings, store_path| Box::new(SimpleOpStore::init(store_path))
    }

    pub fn default_op_heads_store_initializer() -> &'static OpHeadsStoreInitializer<'static> {
        &|_settings, store_path| {
            let store = SimpleOpHeadsStore::init(store_path);
            Box::new(store)
        }
    }

    pub fn default_index_store_initializer() -> &'static IndexStoreInitializer<'static> {
        &|_settings, store_path| Ok(Box::new(DefaultIndexStore::init(store_path)))
    }

    pub fn default_submodule_store_initializer() -> &'static SubmoduleStoreInitializer<'static> {
        &|_settings, store_path| Box::new(DefaultSubmoduleStore::init(store_path))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn init(
        user_settings: &UserSettings,
        repo_path: &Path,
        backend_initializer: &BackendInitializer,
        signer: Signer,
        op_store_initializer: &OpStoreInitializer,
        op_heads_store_initializer: &OpHeadsStoreInitializer,
        index_store_initializer: &IndexStoreInitializer,
        submodule_store_initializer: &SubmoduleStoreInitializer,
    ) -> Result<Arc<ReadonlyRepo>, RepoInitError> {
        let repo_path = repo_path.canonicalize().context(repo_path)?;

        let store_path = repo_path.join("store");
        fs::create_dir(&store_path).context(&store_path)?;
        let backend = backend_initializer(user_settings, &store_path)?;
        let backend_path = store_path.join("type");
        fs::write(&backend_path, backend.name()).context(&backend_path)?;
        let store = Store::new(backend, signer, user_settings.use_tree_conflict_format());
        let repo_settings = user_settings.with_repo(&repo_path).unwrap();

        let op_store_path = repo_path.join("op_store");
        fs::create_dir(&op_store_path).context(&op_store_path)?;
        let op_store = op_store_initializer(user_settings, &op_store_path);
        let op_store_type_path = op_store_path.join("type");
        fs::write(&op_store_type_path, op_store.name()).context(&op_store_type_path)?;
        let op_store: Arc<dyn OpStore> = Arc::from(op_store);

        let mut root_view = op_store::View::default();
        root_view.head_ids.insert(store.root_commit_id().clone());
        root_view
            .public_head_ids
            .insert(store.root_commit_id().clone());

        let op_heads_path = repo_path.join("op_heads");
        fs::create_dir(&op_heads_path).context(&op_heads_path)?;
        let operation_metadata =
            crate::transaction::create_op_metadata(user_settings, "initialize repo".to_string());
        let root_view_id = op_store.write_view(&root_view).unwrap();
        let init_operation = op_store::Operation {
            view_id: root_view_id,
            parents: vec![],
            metadata: operation_metadata,
        };
        let init_operation_id = op_store.write_operation(&init_operation).unwrap();
        let init_operation = Operation::new(op_store.clone(), init_operation_id, init_operation);
        let op_heads_store = op_heads_store_initializer(user_settings, &op_heads_path);
        op_heads_store.update_op_heads(&[], init_operation.id());
        let op_heads_type_path = op_heads_path.join("type");
        fs::write(&op_heads_type_path, op_heads_store.name()).context(&op_heads_type_path)?;
        let op_heads_store = Arc::from(op_heads_store);

        let index_path = repo_path.join("index");
        fs::create_dir(&index_path).context(&index_path)?;
        let index_store = index_store_initializer(user_settings, &index_path)?;
        let index_type_path = index_path.join("type");
        fs::write(&index_type_path, index_store.name()).context(&index_type_path)?;
        let index_store = Arc::from(index_store);

        let submodule_store_path = repo_path.join("submodule_store");
        fs::create_dir(&submodule_store_path).context(&submodule_store_path)?;
        let submodule_store = submodule_store_initializer(user_settings, &submodule_store_path);
        let submodule_store_type_path = submodule_store_path.join("type");
        fs::write(&submodule_store_type_path, submodule_store.name())
            .context(&submodule_store_type_path)?;
        let submodule_store = Arc::from(submodule_store);

        let view = View::new(root_view);
        Ok(Arc::new(ReadonlyRepo {
            repo_path,
            store,
            op_store,
            op_heads_store,
            operation: init_operation,
            settings: repo_settings,
            index_store,
            index: OnceCell::new(),
            change_id_index: OnceCell::new(),
            view,
            submodule_store,
        }))
    }

    pub fn loader(&self) -> RepoLoader {
        RepoLoader {
            repo_path: self.repo_path.clone(),
            repo_settings: self.settings.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            index_store: self.index_store.clone(),
            submodule_store: self.submodule_store.clone(),
        }
    }

    pub fn repo_path(&self) -> &PathBuf {
        &self.repo_path
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

    pub fn readonly_index(&self) -> &dyn ReadonlyIndex {
        self.index
            .get_or_init(|| {
                // TODO: somehow propagate error, but it's weird if all callers
                // had Result<T, IndexReadError> signature.
                self.index_store
                    .get_index_at_op(&self.operation, &self.store)
                    .unwrap()
            })
            .deref()
    }

    fn change_id_index(&self) -> &dyn ChangeIdIndex {
        self.change_id_index
            .get_or_init(|| {
                // TODO: maybe add abstraction over 'static/'index revset
                // evaluation, and use it.
                let expression = RevsetExpression::all().resolve_programmatic(self);
                let revset = self
                    .readonly_index()
                    .evaluate_revset_static(&expression, self.store())
                    .unwrap();
                revset.change_id_index()
            })
            .as_ref()
    }

    pub fn op_heads_store(&self) -> &Arc<dyn OpHeadsStore> {
        &self.op_heads_store
    }

    pub fn index_store(&self) -> &Arc<dyn IndexStore> {
        &self.index_store
    }

    pub fn settings(&self) -> &RepoSettings {
        &self.settings
    }

    pub fn start_transaction(
        self: &Arc<ReadonlyRepo>,
        user_settings: &UserSettings,
    ) -> Transaction {
        let mut_repo = MutableRepo::new(self.clone(), self.readonly_index(), &self.view);
        Transaction::new(mut_repo, user_settings)
    }

    pub fn reload_at_head(
        &self,
        user_settings: &UserSettings,
    ) -> Result<Arc<ReadonlyRepo>, OpHeadResolutionError<RepoLoaderError>> {
        self.loader().load_at_head(user_settings)
    }

    #[instrument]
    pub fn reload_at(&self, operation: &Operation) -> Result<Arc<ReadonlyRepo>, RepoLoaderError> {
        self.loader().load_at(operation)
    }
}

impl Repo for ReadonlyRepo {
    fn store(&self) -> &Arc<Store> {
        &self.store
    }

    fn op_store(&self) -> &Arc<dyn OpStore> {
        &self.op_store
    }

    fn index(&self) -> &dyn Index {
        self.readonly_index().as_index()
    }

    fn view(&self) -> &View {
        &self.view
    }

    fn submodule_store(&self) -> &Arc<dyn SubmoduleStore> {
        &self.submodule_store
    }

    fn resolve_change_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<Vec<CommitId>> {
        self.change_id_index().resolve_prefix(prefix)
    }

    fn shortest_unique_change_id_prefix_len(&self, target_id: &ChangeId) -> usize {
        self.change_id_index().shortest_unique_prefix_len(target_id)
    }
}

pub type BackendInitializer<'a> =
    dyn Fn(&UserSettings, &Path) -> Result<Box<dyn Backend>, BackendInitError> + 'a;
pub type OpStoreInitializer<'a> = dyn Fn(&UserSettings, &Path) -> Box<dyn OpStore> + 'a;
pub type OpHeadsStoreInitializer<'a> = dyn Fn(&UserSettings, &Path) -> Box<dyn OpHeadsStore> + 'a;
pub type IndexStoreInitializer<'a> =
    dyn Fn(&UserSettings, &Path) -> Result<Box<dyn IndexStore>, BackendInitError> + 'a;
pub type SubmoduleStoreInitializer<'a> =
    dyn Fn(&UserSettings, &Path) -> Box<dyn SubmoduleStore> + 'a;

type BackendFactory =
    Box<dyn Fn(&UserSettings, &Path) -> Result<Box<dyn Backend>, BackendLoadError>>;
type OpStoreFactory = Box<dyn Fn(&UserSettings, &Path) -> Box<dyn OpStore>>;
type OpHeadsStoreFactory = Box<dyn Fn(&UserSettings, &Path) -> Box<dyn OpHeadsStore>>;
type IndexStoreFactory =
    Box<dyn Fn(&UserSettings, &Path) -> Result<Box<dyn IndexStore>, BackendLoadError>>;
type SubmoduleStoreFactory = Box<dyn Fn(&UserSettings, &Path) -> Box<dyn SubmoduleStore>>;

pub struct StoreFactories {
    backend_factories: HashMap<String, BackendFactory>,
    op_store_factories: HashMap<String, OpStoreFactory>,
    op_heads_store_factories: HashMap<String, OpHeadsStoreFactory>,
    index_store_factories: HashMap<String, IndexStoreFactory>,
    submodule_store_factories: HashMap<String, SubmoduleStoreFactory>,
}

impl Default for StoreFactories {
    fn default() -> Self {
        let mut factories = StoreFactories::empty();

        // Backends
        factories.add_backend(
            LocalBackend::name(),
            Box::new(|_settings, store_path| Ok(Box::new(LocalBackend::load(store_path)))),
        );
        factories.add_backend(
            GitBackend::name(),
            Box::new(|settings, store_path| Ok(Box::new(GitBackend::load(settings, store_path)?))),
        );

        // OpStores
        factories.add_op_store(
            SimpleOpStore::name(),
            Box::new(|_settings, store_path| Box::new(SimpleOpStore::load(store_path))),
        );

        // OpHeadsStores
        factories.add_op_heads_store(
            SimpleOpHeadsStore::name(),
            Box::new(|_settings, store_path| Box::new(SimpleOpHeadsStore::load(store_path))),
        );

        // Index
        factories.add_index_store(
            DefaultIndexStore::name(),
            Box::new(|_settings, store_path| Ok(Box::new(DefaultIndexStore::load(store_path)))),
        );

        // SubmoduleStores
        factories.add_submodule_store(
            DefaultSubmoduleStore::name(),
            Box::new(|_settings, store_path| Box::new(DefaultSubmoduleStore::load(store_path))),
        );

        factories
    }
}

#[derive(Debug, Error)]
pub enum StoreLoadError {
    #[error("Unsupported {store} backend type '{store_type}'")]
    UnsupportedType {
        store: &'static str,
        store_type: String,
    },
    #[error("Failed to read {store} backend type: {source}")]
    ReadError {
        store: &'static str,
        source: PathError,
    },
    #[error(transparent)]
    Backend(#[from] BackendLoadError),
    #[error(transparent)]
    Signing(#[from] SignInitError),
}

impl StoreFactories {
    pub fn empty() -> Self {
        StoreFactories {
            backend_factories: HashMap::new(),
            op_store_factories: HashMap::new(),
            op_heads_store_factories: HashMap::new(),
            index_store_factories: HashMap::new(),
            submodule_store_factories: HashMap::new(),
        }
    }

    pub fn add_backend(&mut self, name: &str, factory: BackendFactory) {
        self.backend_factories.insert(name.to_string(), factory);
    }

    pub fn load_backend(
        &self,
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Box<dyn Backend>, StoreLoadError> {
        // For compatibility with existing repos. TODO: Delete in 0.8+.
        if store_path.join("backend").is_file() {
            fs::rename(store_path.join("backend"), store_path.join("type"))
                .expect("Failed to rename 'backend' file to 'type'");
        }
        // For compatibility with existing repos. TODO: Delete default in 0.8+.
        let backend_type = read_store_type_compat("commit", store_path.join("type"), || {
            if store_path.join("git_target").is_file() {
                GitBackend::name()
            } else {
                LocalBackend::name()
            }
        })?;
        let backend_factory = self.backend_factories.get(&backend_type).ok_or_else(|| {
            StoreLoadError::UnsupportedType {
                store: "commit",
                store_type: backend_type.to_string(),
            }
        })?;
        Ok(backend_factory(settings, store_path)?)
    }

    pub fn add_op_store(&mut self, name: &str, factory: OpStoreFactory) {
        self.op_store_factories.insert(name.to_string(), factory);
    }

    pub fn load_op_store(
        &self,
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Box<dyn OpStore>, StoreLoadError> {
        // For compatibility with existing repos. TODO: Delete default in 0.8+.
        let op_store_type =
            read_store_type_compat("operation", store_path.join("type"), SimpleOpStore::name)?;
        let op_store_factory = self.op_store_factories.get(&op_store_type).ok_or_else(|| {
            StoreLoadError::UnsupportedType {
                store: "operation",
                store_type: op_store_type.to_string(),
            }
        })?;
        Ok(op_store_factory(settings, store_path))
    }

    pub fn add_op_heads_store(&mut self, name: &str, factory: OpHeadsStoreFactory) {
        self.op_heads_store_factories
            .insert(name.to_string(), factory);
    }

    pub fn load_op_heads_store(
        &self,
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Box<dyn OpHeadsStore>, StoreLoadError> {
        // For compatibility with existing repos. TODO: Delete default in 0.8+.
        let op_heads_store_type = read_store_type_compat(
            "operation heads",
            store_path.join("type"),
            SimpleOpHeadsStore::name,
        )?;
        let op_heads_store_factory = self
            .op_heads_store_factories
            .get(&op_heads_store_type)
            .ok_or_else(|| StoreLoadError::UnsupportedType {
                store: "operation heads",
                store_type: op_heads_store_type.to_string(),
            })?;
        Ok(op_heads_store_factory(settings, store_path))
    }

    pub fn add_index_store(&mut self, name: &str, factory: IndexStoreFactory) {
        self.index_store_factories.insert(name.to_string(), factory);
    }

    pub fn load_index_store(
        &self,
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Box<dyn IndexStore>, StoreLoadError> {
        // For compatibility with existing repos. TODO: Delete default in 0.9+
        let index_store_type =
            read_store_type_compat("index", store_path.join("type"), DefaultIndexStore::name)?;
        let index_store_factory = self
            .index_store_factories
            .get(&index_store_type)
            .ok_or_else(|| StoreLoadError::UnsupportedType {
                store: "index",
                store_type: index_store_type.to_string(),
            })?;
        Ok(index_store_factory(settings, store_path)?)
    }

    pub fn add_submodule_store(&mut self, name: &str, factory: SubmoduleStoreFactory) {
        self.submodule_store_factories
            .insert(name.to_string(), factory);
    }

    pub fn load_submodule_store(
        &self,
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Box<dyn SubmoduleStore>, StoreLoadError> {
        // For compatibility with repos without repo/submodule_store.
        // TODO Delete default in TBD version
        let submodule_store_type = read_store_type_compat(
            "submodule_store",
            store_path.join("type"),
            DefaultSubmoduleStore::name,
        )?;
        let submodule_store_factory = self
            .submodule_store_factories
            .get(&submodule_store_type)
            .ok_or_else(|| StoreLoadError::UnsupportedType {
                store: "submodule_store",
                store_type: submodule_store_type.to_string(),
            })?;

        Ok(submodule_store_factory(settings, store_path))
    }
}

pub fn read_store_type_compat(
    store: &'static str,
    path: impl AsRef<Path>,
    default: impl FnOnce() -> &'static str,
) -> Result<String, StoreLoadError> {
    let path = path.as_ref();
    let read_or_write_default = || match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let default_type = default();
            fs::create_dir(path.parent().unwrap()).ok();
            fs::write(path, default_type)?;
            Ok(default_type.to_owned())
        }
        Err(err) => Err(err),
    };
    read_or_write_default()
        .context(path)
        .map_err(|source| StoreLoadError::ReadError { store, source })
}

#[derive(Debug, Error)]
pub enum RepoLoaderError {
    #[error(transparent)]
    TreeMerge(#[from] TreeMergeError),
    #[error(transparent)]
    OpStore(#[from] OpStoreError),
}

#[derive(Clone)]
pub struct RepoLoader {
    repo_path: PathBuf,
    repo_settings: RepoSettings,
    store: Arc<Store>,
    op_store: Arc<dyn OpStore>,
    op_heads_store: Arc<dyn OpHeadsStore>,
    index_store: Arc<dyn IndexStore>,
    submodule_store: Arc<dyn SubmoduleStore>,
}

impl RepoLoader {
    pub fn init(
        user_settings: &UserSettings,
        repo_path: &Path,
        store_factories: &StoreFactories,
    ) -> Result<Self, StoreLoadError> {
        let store = Store::new(
            store_factories.load_backend(user_settings, &repo_path.join("store"))?,
            Signer::from_settings(user_settings)?,
            user_settings.use_tree_conflict_format(),
        );
        let repo_settings = user_settings.with_repo(repo_path).unwrap();
        let op_store =
            Arc::from(store_factories.load_op_store(user_settings, &repo_path.join("op_store"))?);
        let op_heads_store = Arc::from(
            store_factories.load_op_heads_store(user_settings, &repo_path.join("op_heads"))?,
        );
        let index_store =
            Arc::from(store_factories.load_index_store(user_settings, &repo_path.join("index"))?);
        let submodule_store = Arc::from(
            store_factories
                .load_submodule_store(user_settings, &repo_path.join("submodule_store"))?,
        );
        Ok(Self {
            repo_path: repo_path.to_path_buf(),
            repo_settings,
            store,
            op_store,
            op_heads_store,
            index_store,
            submodule_store,
        })
    }

    pub fn repo_path(&self) -> &PathBuf {
        &self.repo_path
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn index_store(&self) -> &Arc<dyn IndexStore> {
        &self.index_store
    }

    pub fn op_store(&self) -> &Arc<dyn OpStore> {
        &self.op_store
    }

    pub fn op_heads_store(&self) -> &Arc<dyn OpHeadsStore> {
        &self.op_heads_store
    }

    pub fn load_at_head(
        &self,
        user_settings: &UserSettings,
    ) -> Result<Arc<ReadonlyRepo>, OpHeadResolutionError<RepoLoaderError>> {
        let op = op_heads_store::resolve_op_heads(
            self.op_heads_store.as_ref(),
            &self.op_store,
            |op_heads| self._resolve_op_heads(op_heads, user_settings),
        )?;
        let view = View::new(op.view()?.take_store_view());
        Ok(self._finish_load(op, view))
    }

    #[instrument(skip(self))]
    pub fn load_at(&self, op: &Operation) -> Result<Arc<ReadonlyRepo>, RepoLoaderError> {
        let view = View::new(op.view()?.take_store_view());
        Ok(self._finish_load(op.clone(), view))
    }

    pub fn create_from(
        &self,
        operation: Operation,
        view: View,
        index: Box<dyn ReadonlyIndex>,
    ) -> Arc<ReadonlyRepo> {
        let repo = ReadonlyRepo {
            repo_path: self.repo_path.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            operation,
            settings: self.repo_settings.clone(),
            index_store: self.index_store.clone(),
            submodule_store: self.submodule_store.clone(),
            index: OnceCell::with_value(index),
            change_id_index: OnceCell::new(),
            view,
        };
        Arc::new(repo)
    }

    fn _resolve_op_heads(
        &self,
        op_heads: Vec<Operation>,
        user_settings: &UserSettings,
    ) -> Result<Operation, RepoLoaderError> {
        let base_repo = self.load_at(&op_heads[0])?;
        let mut tx = base_repo.start_transaction(user_settings);
        for other_op_head in op_heads.into_iter().skip(1) {
            tx.merge_operation(other_op_head)?;
            tx.mut_repo().rebase_descendants(user_settings)?;
        }
        let merged_repo = tx
            .write("resolve concurrent operations")
            .leave_unpublished();
        Ok(merged_repo.operation().clone())
    }

    fn _finish_load(&self, operation: Operation, view: View) -> Arc<ReadonlyRepo> {
        let repo = ReadonlyRepo {
            repo_path: self.repo_path.clone(),
            store: self.store.clone(),
            op_store: self.op_store.clone(),
            op_heads_store: self.op_heads_store.clone(),
            operation,
            settings: self.repo_settings.clone(),
            index_store: self.index_store.clone(),
            submodule_store: self.submodule_store.clone(),
            index: OnceCell::new(),
            change_id_index: OnceCell::new(),
            view,
        };
        Arc::new(repo)
    }
}

pub struct MutableRepo {
    base_repo: Arc<ReadonlyRepo>,
    index: Box<dyn MutableIndex>,
    view: DirtyCell<View>,
    rewritten_commits: HashMap<CommitId, HashSet<CommitId>>,
    abandoned_commits: HashSet<CommitId>,
}

impl MutableRepo {
    pub fn new(
        base_repo: Arc<ReadonlyRepo>,
        index: &dyn ReadonlyIndex,
        view: &View,
    ) -> MutableRepo {
        let mut_view = view.clone();
        let mut_index = index.start_modification();
        MutableRepo {
            base_repo,
            index: mut_index,
            view: DirtyCell::with_clean(mut_view),
            rewritten_commits: Default::default(),
            abandoned_commits: Default::default(),
        }
    }

    pub fn base_repo(&self) -> &Arc<ReadonlyRepo> {
        &self.base_repo
    }

    fn view_mut(&mut self) -> &mut View {
        self.view.get_mut()
    }

    pub fn mutable_index(&self) -> &dyn MutableIndex {
        self.index.as_ref()
    }

    pub fn has_changes(&self) -> bool {
        !(self.abandoned_commits.is_empty()
            && self.rewritten_commits.is_empty()
            && self.view() == &self.base_repo.view)
    }

    pub fn consume(self) -> (Box<dyn MutableIndex>, View) {
        self.view.ensure_clean(|v| self.enforce_view_invariants(v));
        (self.index, self.view.into_inner())
    }

    pub fn new_commit(
        &mut self,
        settings: &UserSettings,
        parents: Vec<CommitId>,
        tree_id: MergedTreeId,
    ) -> CommitBuilder {
        CommitBuilder::for_new_commit(self, settings, parents, tree_id)
    }

    pub fn rewrite_commit(
        &mut self,
        settings: &UserSettings,
        predecessor: &Commit,
    ) -> CommitBuilder {
        CommitBuilder::for_rewrite_from(self, settings, predecessor)
        // CommitBuilder::write will record the rewrite in
        // `self.rewritten_commits`
    }

    pub fn write_commit(
        &mut self,
        commit: backend::Commit,
        sign_with: Option<&mut SigningFn>,
    ) -> BackendResult<Commit> {
        let commit = self.store().write_commit(commit, sign_with)?;
        self.add_head(&commit)?;
        Ok(commit)
    }

    /// Record a commit as having been rewritten to one or more ids in this
    /// transaction.
    ///
    /// This record is used by `rebase_descendants` to know which commits have
    /// children that need to be rebased, and where to rebase them to. See the
    /// docstring for `record_rewritten_commit` for details.
    //  TODO(ilyagr): Make this API saner, e.g. make `self.rewritten_commits` public
    // and make empty values correspond to abandoned commits.
    pub fn set_rewritten_commit(
        &mut self,
        old_id: CommitId,
        new_ids: impl IntoIterator<Item = CommitId>,
    ) {
        assert_ne!(old_id, *self.store().root_commit_id());
        self.rewritten_commits
            .insert(old_id, new_ids.into_iter().collect());
    }

    /// Record a commit as having been rewritten in this transaction. If it was
    /// already rewritten, mark it as divergent (unlike `set_rewritten_commit`)
    ///
    /// This record is used by `rebase_descendants` to know which commits have
    /// children that need to be rebased, and where to rebase the children (as
    /// well as branches) to.
    ///
    /// The `rebase_descendants` logic treats these records as follows:
    ///
    /// - If a commit is recorded as rewritten to a single commit, its
    ///   descendants would be rebased to become descendants of `new_id`. Any
    ///   branches at `old_id` are also moved to `new_id`.
    /// - If a commit is recorded as rewritten to more than one commit, it is
    ///   assumed to have become divergent. Its descendants are *not* rebased.
    ///   However, the *branches* that were at `old_id` are moved to each of the
    ///   new ids, and thus become conflicted.
    ///
    /// In neither case would `rebase_descendants` modify the `old_id` commit
    /// itself.
    pub fn record_rewritten_commit(&mut self, old_id: CommitId, new_id: CommitId) {
        assert_ne!(old_id, *self.store().root_commit_id());
        self.rewritten_commits
            .entry(old_id)
            .or_default()
            .insert(new_id);
    }

    pub fn clear_rewritten_commits(&mut self) {
        self.rewritten_commits.clear();
    }

    /// Record a commit as having been abandoned in this transaction.
    ///
    /// This record is used by `rebase_descendants` to know which commits have
    /// children that need to be rebased, and where to rebase the children (as
    /// well as branches) to.
    ///
    /// The `rebase_descendants` logic will rebase the descendants of `old_id`
    /// to become the descendants of parent(s) of `old_id`. Any branches at
    /// `old_id` would be moved to the parent(s) of `old_id` as well.
    pub fn record_abandoned_commit(&mut self, old_id: CommitId) {
        assert_ne!(old_id, *self.store().root_commit_id());
        self.abandoned_commits.insert(old_id);
    }

    pub fn clear_abandoned_commits(&mut self) {
        self.abandoned_commits.clear();
    }

    pub fn has_rewrites(&self) -> bool {
        !(self.rewritten_commits.is_empty() && self.abandoned_commits.is_empty())
    }

    /// Creates a `DescendantRebaser` to rebase descendants of the recorded
    /// rewritten and abandoned commits.
    // TODO(ilyagr): Inline this. It's only used in tests.
    pub fn create_descendant_rebaser<'settings, 'repo>(
        &'repo mut self,
        settings: &'settings UserSettings,
    ) -> DescendantRebaser<'settings, 'repo> {
        DescendantRebaser::new(
            settings,
            self,
            self.rewritten_commits.clone(),
            self.abandoned_commits.clone(),
        )
    }

    fn rebase_descendants_return_rebaser<'settings, 'repo>(
        &'repo mut self,
        settings: &'settings UserSettings,
        options: RebaseOptions,
    ) -> Result<Option<DescendantRebaser<'settings, 'repo>>, TreeMergeError> {
        if !self.has_rewrites() {
            // Optimization
            return Ok(None);
        }
        let mut rebaser = self.create_descendant_rebaser(settings);
        *rebaser.mut_options() = options;
        rebaser.rebase_all()?;
        Ok(Some(rebaser))
    }

    // TODO(ilyagr): Either document that this also moves branches (rename the
    // function and the related functions?) or change things so that this only
    // rebases descendants.
    pub fn rebase_descendants_with_options(
        &mut self,
        settings: &UserSettings,
        options: RebaseOptions,
    ) -> Result<usize, TreeMergeError> {
        Ok(self
            .rebase_descendants_return_rebaser(settings, options)?
            .map_or(0, |rebaser| rebaser.rebased().len()))
    }

    pub fn rebase_descendants(&mut self, settings: &UserSettings) -> Result<usize, TreeMergeError> {
        self.rebase_descendants_with_options(settings, Default::default())
    }

    pub fn rebase_descendants_return_map(
        &mut self,
        settings: &UserSettings,
    ) -> Result<HashMap<CommitId, CommitId>, TreeMergeError> {
        Ok(self
            // We do not set RebaseOptions here, since this function does not currently return
            // enough information to describe the results of a rebase if some commits got
            // abandoned
            .rebase_descendants_return_rebaser(settings, Default::default())?
            .map_or(HashMap::new(), |rebaser| rebaser.rebased().clone()))
    }

    pub fn set_wc_commit(
        &mut self,
        workspace_id: WorkspaceId,
        commit_id: CommitId,
    ) -> Result<(), RewriteRootCommit> {
        if &commit_id == self.store().root_commit_id() {
            return Err(RewriteRootCommit);
        }
        self.view_mut().set_wc_commit(workspace_id, commit_id);
        Ok(())
    }

    pub fn remove_wc_commit(&mut self, workspace_id: &WorkspaceId) {
        self.view_mut().remove_wc_commit(workspace_id);
    }

    pub fn check_out(
        &mut self,
        workspace_id: WorkspaceId,
        settings: &UserSettings,
        commit: &Commit,
    ) -> Result<Commit, CheckOutCommitError> {
        let wc_commit = self
            .new_commit(
                settings,
                vec![commit.id().clone()],
                commit.tree_id().clone(),
            )
            .write()?;
        self.edit(workspace_id, &wc_commit)?;
        Ok(wc_commit)
    }

    pub fn edit(
        &mut self,
        workspace_id: WorkspaceId,
        commit: &Commit,
    ) -> Result<(), EditCommitError> {
        fn local_branch_target_ids(view: &View) -> impl Iterator<Item = &CommitId> {
            view.local_branches()
                .flat_map(|(_, target)| target.added_ids())
        }

        let maybe_wc_commit_id = self
            .view
            .with_ref(|v| v.get_wc_commit_id(&workspace_id).cloned());
        if let Some(wc_commit_id) = maybe_wc_commit_id {
            let wc_commit = self
                .store()
                .get_commit(&wc_commit_id)
                .map_err(EditCommitError::WorkingCopyCommitNotFound)?;
            if wc_commit.is_discardable()
                && self
                    .view
                    .with_ref(|v| local_branch_target_ids(v).all(|id| id != wc_commit.id()))
                && self.view().heads().contains(wc_commit.id())
            {
                // Abandon the working-copy commit we're leaving if it's empty, not pointed by
                // local branch, and a head commit.
                self.record_abandoned_commit(wc_commit_id);
            }
        }
        self.set_wc_commit(workspace_id, commit.id().clone())
            .map_err(|RewriteRootCommit| EditCommitError::RewriteRootCommit)
    }

    fn enforce_view_invariants(&self, view: &mut View) {
        let view = view.store_view_mut();
        view.public_head_ids = self
            .index()
            .heads(&mut view.public_head_ids.iter())
            .iter()
            .cloned()
            .collect();
        view.head_ids.extend(view.public_head_ids.iter().cloned());
        view.head_ids = self
            .index()
            .heads(&mut view.head_ids.iter())
            .iter()
            .cloned()
            .collect();
    }

    /// Ensures that the given `head` and ancestor commits are reachable from
    /// the visible heads.
    pub fn add_head(&mut self, head: &Commit) -> BackendResult<()> {
        self.add_heads(slice::from_ref(head))
    }

    /// Ensures that the given `heads` and ancestor commits are reachable from
    /// the visible heads.
    ///
    /// The `heads` may contain redundant commits such as already visible ones
    /// and ancestors of the other heads. The `heads` and ancestor commits
    /// should exist in the store.
    pub fn add_heads(&mut self, heads: &[Commit]) -> BackendResult<()> {
        let current_heads = self.view.get_mut().heads();
        // Use incremental update for common case of adding a single commit on top a
        // current head. TODO: Also use incremental update when adding a single
        // commit on top a non-head.
        match heads {
            [] => {}
            [head]
                if head
                    .parent_ids()
                    .iter()
                    .all(|parent_id| current_heads.contains(parent_id)) =>
            {
                self.index.add_commit(head);
                self.view.get_mut().add_head(head.id());
                for parent_id in head.parent_ids() {
                    self.view.get_mut().remove_head(parent_id);
                }
            }
            _ => {
                let missing_commits = dag_walk::topo_order_reverse_ord_ok(
                    heads
                        .iter()
                        .cloned()
                        .map(CommitByCommitterTimestamp)
                        .map(Ok),
                    |CommitByCommitterTimestamp(commit)| commit.id().clone(),
                    |CommitByCommitterTimestamp(commit)| {
                        commit
                            .parent_ids()
                            .iter()
                            .filter(|id| !self.index().has_id(id))
                            .map(|id| self.store().get_commit(id))
                            .map_ok(CommitByCommitterTimestamp)
                            .collect_vec()
                    },
                )?;
                for CommitByCommitterTimestamp(missing_commit) in missing_commits.iter().rev() {
                    self.index.add_commit(missing_commit);
                }
                for head in heads {
                    self.view.get_mut().add_head(head.id());
                }
                self.view.mark_dirty();
            }
        }
        Ok(())
    }

    pub fn remove_head(&mut self, head: &CommitId) {
        self.view_mut().remove_head(head);
        self.view.mark_dirty();
    }

    pub fn add_public_head(&mut self, head: &Commit) {
        self.view_mut().add_public_head(head.id());
        self.view.mark_dirty();
    }

    pub fn remove_public_head(&mut self, head: &CommitId) {
        self.view_mut().remove_public_head(head);
        self.view.mark_dirty();
    }

    /// Returns true if any local or remote branch of the given `name` exists.
    #[must_use]
    pub fn has_branch(&self, name: &str) -> bool {
        self.view.with_ref(|v| v.has_branch(name))
    }

    pub fn remove_branch(&mut self, name: &str) {
        self.view_mut().remove_branch(name);
    }

    pub fn get_local_branch(&self, name: &str) -> RefTarget {
        self.view.with_ref(|v| v.get_local_branch(name).clone())
    }

    pub fn set_local_branch_target(&mut self, name: &str, target: RefTarget) {
        self.view_mut().set_local_branch_target(name, target);
    }

    pub fn merge_local_branch(
        &mut self,
        name: &str,
        base_target: &RefTarget,
        other_target: &RefTarget,
    ) {
        let view = self.view.get_mut();
        let index = self.index.as_index();
        let self_target = view.get_local_branch(name);
        let new_target = merge_ref_targets(index, self_target, base_target, other_target);
        view.set_local_branch_target(name, new_target);
    }

    pub fn get_remote_branch(&self, name: &str, remote_name: &str) -> RemoteRef {
        self.view
            .with_ref(|v| v.get_remote_branch(name, remote_name).clone())
    }

    pub fn set_remote_branch(&mut self, name: &str, remote_name: &str, remote_ref: RemoteRef) {
        self.view_mut()
            .set_remote_branch(name, remote_name, remote_ref);
    }

    fn merge_remote_branch(
        &mut self,
        name: &str,
        remote_name: &str,
        base_ref: &RemoteRef,
        other_ref: &RemoteRef,
    ) {
        let view = self.view.get_mut();
        let index = self.index.as_index();
        let self_ref = view.get_remote_branch(name, remote_name);
        let new_ref = merge_remote_refs(index, self_ref, base_ref, other_ref);
        view.set_remote_branch(name, remote_name, new_ref);
    }

    /// Merges the specified remote branch in to local branch, and starts
    /// tracking it.
    pub fn track_remote_branch(&mut self, name: &str, remote_name: &str) {
        let mut remote_ref = self.get_remote_branch(name, remote_name);
        let base_target = remote_ref.tracking_target();
        self.merge_local_branch(name, base_target, &remote_ref.target);
        remote_ref.state = RemoteRefState::Tracking;
        self.set_remote_branch(name, remote_name, remote_ref);
    }

    /// Stops tracking the specified remote branch.
    pub fn untrack_remote_branch(&mut self, name: &str, remote_name: &str) {
        let mut remote_ref = self.get_remote_branch(name, remote_name);
        remote_ref.state = RemoteRefState::New;
        self.set_remote_branch(name, remote_name, remote_ref);
    }

    pub fn remove_remote(&mut self, remote_name: &str) {
        self.view_mut().remove_remote(remote_name);
    }

    pub fn rename_remote(&mut self, old: &str, new: &str) {
        self.view_mut().rename_remote(old, new);
    }

    pub fn get_tag(&self, name: &str) -> RefTarget {
        self.view.with_ref(|v| v.get_tag(name).clone())
    }

    pub fn set_tag_target(&mut self, name: &str, target: RefTarget) {
        self.view_mut().set_tag_target(name, target);
    }

    pub fn merge_tag(&mut self, name: &str, base_target: &RefTarget, other_target: &RefTarget) {
        let view = self.view.get_mut();
        let index = self.index.as_index();
        let self_target = view.get_tag(name);
        let new_target = merge_ref_targets(index, self_target, base_target, other_target);
        view.set_tag_target(name, new_target);
    }

    pub fn get_git_ref(&self, name: &str) -> RefTarget {
        self.view.with_ref(|v| v.get_git_ref(name).clone())
    }

    pub fn set_git_ref_target(&mut self, name: &str, target: RefTarget) {
        self.view_mut().set_git_ref_target(name, target);
    }

    fn merge_git_ref(&mut self, name: &str, base_target: &RefTarget, other_target: &RefTarget) {
        let view = self.view.get_mut();
        let index = self.index.as_index();
        let self_target = view.get_git_ref(name);
        let new_target = merge_ref_targets(index, self_target, base_target, other_target);
        view.set_git_ref_target(name, new_target);
    }

    pub fn git_head(&self) -> RefTarget {
        self.view.with_ref(|v| v.git_head().clone())
    }

    pub fn set_git_head_target(&mut self, target: RefTarget) {
        self.view_mut().set_git_head_target(target);
    }

    pub fn set_view(&mut self, data: op_store::View) {
        self.view_mut().set_view(data);
        self.view.mark_dirty();
    }

    pub fn merge(&mut self, base_repo: &ReadonlyRepo, other_repo: &ReadonlyRepo) {
        // First, merge the index, so we can take advantage of a valid index when
        // merging the view. Merging in base_repo's index isn't typically
        // necessary, but it can be if base_repo is ahead of either self or other_repo
        // (e.g. because we're undoing an operation that hasn't been published).
        self.index.merge_in(base_repo.readonly_index());
        self.index.merge_in(other_repo.readonly_index());

        self.view.ensure_clean(|v| self.enforce_view_invariants(v));
        self.merge_view(&base_repo.view, &other_repo.view);
        self.view.mark_dirty();
    }

    fn merge_view(&mut self, base: &View, other: &View) {
        // Merge working-copy commits. If there's a conflict, we keep the self side.
        for (workspace_id, base_wc_commit) in base.wc_commit_ids() {
            let self_wc_commit = self.view().get_wc_commit_id(workspace_id);
            let other_wc_commit = other.get_wc_commit_id(workspace_id);
            if other_wc_commit == Some(base_wc_commit) || other_wc_commit == self_wc_commit {
                // The other side didn't change or both sides changed in the
                // same way.
            } else if let Some(other_wc_commit) = other_wc_commit {
                if self_wc_commit == Some(base_wc_commit) {
                    self.view_mut()
                        .set_wc_commit(workspace_id.clone(), other_wc_commit.clone());
                }
            } else {
                // The other side removed the workspace. We want to remove it even if the self
                // side changed the working-copy commit.
                self.view_mut().remove_wc_commit(workspace_id);
            }
        }
        for (workspace_id, other_wc_commit) in other.wc_commit_ids() {
            if self.view().get_wc_commit_id(workspace_id).is_none()
                && base.get_wc_commit_id(workspace_id).is_none()
            {
                // The other side added the workspace.
                self.view_mut()
                    .set_wc_commit(workspace_id.clone(), other_wc_commit.clone());
            }
        }

        for removed_head in base.public_heads().difference(other.public_heads()) {
            self.view_mut().remove_public_head(removed_head);
        }
        for added_head in other.public_heads().difference(base.public_heads()) {
            self.view_mut().add_public_head(added_head);
        }

        let base_heads = base.heads().iter().cloned().collect_vec();
        let own_heads = self.view().heads().iter().cloned().collect_vec();
        let other_heads = other.heads().iter().cloned().collect_vec();
        self.record_rewrites(&base_heads, &own_heads);
        self.record_rewrites(&base_heads, &other_heads);
        // No need to remove heads removed by `other` because we already marked them
        // abandoned or rewritten.
        for added_head in other.heads().difference(base.heads()) {
            self.view_mut().add_head(added_head);
        }

        let changed_local_branches =
            diff_named_ref_targets(base.local_branches(), other.local_branches());
        for (name, (base_target, other_target)) in changed_local_branches {
            self.merge_local_branch(name, base_target, other_target);
        }

        let changed_tags = diff_named_ref_targets(base.tags(), other.tags());
        for (name, (base_target, other_target)) in changed_tags {
            self.merge_tag(name, base_target, other_target);
        }

        let changed_git_refs = diff_named_ref_targets(base.git_refs(), other.git_refs());
        for (name, (base_target, other_target)) in changed_git_refs {
            self.merge_git_ref(name, base_target, other_target);
        }

        let changed_remote_branches =
            diff_named_remote_refs(base.all_remote_branches(), other.all_remote_branches());
        for ((name, remote_name), (base_ref, other_ref)) in changed_remote_branches {
            self.merge_remote_branch(name, remote_name, base_ref, other_ref);
        }

        let new_git_head_target = merge_ref_targets(
            self.index(),
            self.view().git_head(),
            base.git_head(),
            other.git_head(),
        );
        self.set_git_head_target(new_git_head_target);
    }

    /// Finds and records commits that were rewritten or abandoned between
    /// `old_heads` and `new_heads`.
    fn record_rewrites(&mut self, old_heads: &[CommitId], new_heads: &[CommitId]) {
        let mut removed_changes: HashMap<ChangeId, Vec<CommitId>> = HashMap::new();
        for (commit_id, change_id) in revset::walk_revs(self, old_heads, new_heads)
            .unwrap()
            .commit_change_ids()
        {
            removed_changes
                .entry(change_id)
                .or_default()
                .push(commit_id);
        }
        if removed_changes.is_empty() {
            return;
        }

        let mut rewritten_changes = HashSet::new();
        let mut rewritten_commits: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
        for (commit_id, change_id) in revset::walk_revs(self, new_heads, old_heads)
            .unwrap()
            .commit_change_ids()
        {
            if let Some(old_commits) = removed_changes.get(&change_id) {
                for old_commit in old_commits {
                    rewritten_commits
                        .entry(old_commit.clone())
                        .or_default()
                        .push(commit_id.clone());
                }
            }
            rewritten_changes.insert(change_id);
        }
        for (old_commit, new_commits) in rewritten_commits {
            for new_commit in new_commits {
                self.record_rewritten_commit(old_commit.clone(), new_commit);
            }
        }

        for (change_id, removed_commit_ids) in &removed_changes {
            if !rewritten_changes.contains(change_id) {
                for removed_commit_id in removed_commit_ids {
                    self.record_abandoned_commit(removed_commit_id.clone());
                }
            }
        }
    }
}

impl Repo for MutableRepo {
    fn store(&self) -> &Arc<Store> {
        self.base_repo.store()
    }

    fn op_store(&self) -> &Arc<dyn OpStore> {
        self.base_repo.op_store()
    }

    fn index(&self) -> &dyn Index {
        self.index.as_index()
    }

    fn submodule_store(&self) -> &Arc<dyn SubmoduleStore> {
        self.base_repo.submodule_store()
    }

    fn view(&self) -> &View {
        self.view
            .get_or_ensure_clean(|v| self.enforce_view_invariants(v))
    }

    fn resolve_change_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<Vec<CommitId>> {
        let revset = RevsetExpression::all().evaluate_programmatic(self).unwrap();
        let change_id_index = revset.change_id_index();
        change_id_index.resolve_prefix(prefix)
    }

    fn shortest_unique_change_id_prefix_len(&self, target_id: &ChangeId) -> usize {
        let revset = RevsetExpression::all().evaluate_programmatic(self).unwrap();
        let change_id_index = revset.change_id_index();
        change_id_index.shortest_unique_prefix_len(target_id)
    }
}

/// Error from attempts to check out the root commit for editing
#[derive(Debug, Error)]
#[error("Cannot rewrite the root commit")]
pub struct RewriteRootCommit;

/// Error from attempts to edit a commit
#[derive(Debug, Error)]
pub enum EditCommitError {
    #[error("Current working-copy commit not found: {0}")]
    WorkingCopyCommitNotFound(BackendError),
    #[error("Cannot rewrite the root commit")]
    RewriteRootCommit,
}

/// Error from attempts to check out a commit
#[derive(Debug, Error)]
pub enum CheckOutCommitError {
    #[error("Failed to create new working-copy commit: {0}")]
    CreateCommit(#[from] BackendError),
    #[error("Failed to edit commit: {0}")]
    EditCommit(#[from] EditCommitError),
}

mod dirty_cell {
    use std::cell::{OnceCell, RefCell};

    /// Cell that lazily updates the value after `mark_dirty()`.
    ///
    /// A clean value can be immutably borrowed within the `self` lifetime.
    #[derive(Clone, Debug)]
    pub struct DirtyCell<T> {
        // Either clean or dirty value is set. The value is boxed to reduce stack space
        // and memcopy overhead.
        clean: OnceCell<Box<T>>,
        dirty: RefCell<Option<Box<T>>>,
    }

    impl<T> DirtyCell<T> {
        pub fn with_clean(value: T) -> Self {
            DirtyCell {
                clean: OnceCell::from(Box::new(value)),
                dirty: RefCell::new(None),
            }
        }

        pub fn get_or_ensure_clean(&self, f: impl FnOnce(&mut T)) -> &T {
            self.clean.get_or_init(|| {
                // Panics if ensure_clean() is invoked from with_ref() callback for example.
                let mut value = self.dirty.borrow_mut().take().unwrap();
                f(&mut value);
                value
            })
        }

        pub fn ensure_clean(&self, f: impl FnOnce(&mut T)) {
            self.get_or_ensure_clean(f);
        }

        pub fn into_inner(self) -> T {
            *self
                .clean
                .into_inner()
                .or_else(|| self.dirty.into_inner())
                .unwrap()
        }

        pub fn with_ref<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            if let Some(value) = self.clean.get() {
                f(value)
            } else {
                f(self.dirty.borrow().as_ref().unwrap())
            }
        }

        pub fn get_mut(&mut self) -> &mut T {
            self.clean
                .get_mut()
                .or_else(|| self.dirty.get_mut().as_mut())
                .unwrap()
        }

        pub fn mark_dirty(&mut self) {
            if let Some(value) = self.clean.take() {
                *self.dirty.get_mut() = Some(value);
            }
        }
    }
}
