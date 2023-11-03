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

use std::any::Any;
use std::fmt::{Debug, Error, Formatter};
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::{fs, str};

use async_trait::async_trait;
use itertools::Itertools;
use prost::Message;
use thiserror::Error;

use crate::backend::{
    make_root_commit, Backend, BackendError, BackendInitError, BackendLoadError, BackendResult,
    ChangeId, Commit, CommitId, Conflict, ConflictId, ConflictTerm, FileId, MergedTreeId,
    MillisSinceEpoch, ObjectId, Signature, SymlinkId, Timestamp, Tree, TreeId, TreeValue,
};
use crate::file_util::{IoResultExt as _, PathError};
use crate::lock::FileLock;
use crate::merge::{Merge, MergeBuilder};
use crate::repo_path::{RepoPath, RepoPathComponent};
use crate::stacked_table::{
    MutableTable, ReadonlyTable, TableSegment, TableStore, TableStoreError,
};

const HASH_LENGTH: usize = 20;
const CHANGE_ID_LENGTH: usize = 16;
/// Ref namespace used only for preventing GC.
const NO_GC_REF_NAMESPACE: &str = "refs/jj/keep/";
const CONFLICT_SUFFIX: &str = ".jjconflict";

#[derive(Debug, Error)]
pub enum GitBackendInitError {
    #[error("Failed to initialize git repository: {0}")]
    InitRepository(#[source] gix::init::Error),
    #[error("Failed to open git repository: {0}")]
    OpenRepository(#[source] gix::open::Error),
    #[error(transparent)]
    Path(PathError),
}

impl From<Box<GitBackendInitError>> for BackendInitError {
    fn from(err: Box<GitBackendInitError>) -> Self {
        BackendInitError(err)
    }
}

#[derive(Debug, Error)]
pub enum GitBackendLoadError {
    #[error("Failed to open git repository: {0}")]
    OpenRepository(#[source] gix::open::Error),
    #[error(transparent)]
    Path(PathError),
}

impl From<Box<GitBackendLoadError>> for BackendLoadError {
    fn from(err: Box<GitBackendLoadError>) -> Self {
        BackendLoadError(err)
    }
}

/// `GitBackend`-specific error that may occur after the backend is loaded.
#[derive(Debug, Error)]
pub enum GitBackendError {
    #[error("Failed to read non-git metadata: {0}")]
    ReadMetadata(#[source] TableStoreError),
    #[error("Failed to write non-git metadata: {0}")]
    WriteMetadata(#[source] TableStoreError),
}

impl From<GitBackendError> for BackendError {
    fn from(err: GitBackendError) -> Self {
        BackendError::Other(err.into())
    }
}

pub struct GitBackend {
    // While gix::Repository can be created from gix::ThreadSafeRepository, it's
    // cheaper to cache the thread-local instance behind a mutex than creating
    // one for each backend method call. Our GitBackend is most likely to be
    // used in a single-threaded context.
    base_repo: gix::ThreadSafeRepository,
    repo: Mutex<gix::Repository>,
    root_commit_id: CommitId,
    root_change_id: ChangeId,
    empty_tree_id: TreeId,
    extra_metadata_store: TableStore,
    cached_extra_metadata: Mutex<Option<Arc<ReadonlyTable>>>,
}

impl GitBackend {
    pub fn name() -> &'static str {
        "git"
    }

    fn new(base_repo: gix::ThreadSafeRepository, extra_metadata_store: TableStore) -> Self {
        let repo = Mutex::new(base_repo.to_thread_local());
        let root_commit_id = CommitId::from_bytes(&[0; HASH_LENGTH]);
        let root_change_id = ChangeId::from_bytes(&[0; CHANGE_ID_LENGTH]);
        let empty_tree_id = TreeId::from_hex("4b825dc642cb6eb9a060e54bf8d69288fbee4904");
        GitBackend {
            base_repo,
            repo,
            root_commit_id,
            root_change_id,
            empty_tree_id,
            extra_metadata_store,
            cached_extra_metadata: Mutex::new(None),
        }
    }

    pub fn init_internal(store_path: &Path) -> Result<Self, Box<GitBackendInitError>> {
        let git_repo_path = Path::new("git");
        let git_repo = gix::ThreadSafeRepository::init(
            store_path.join(git_repo_path),
            gix::create::Kind::Bare,
            gix::create::Options::default(),
        )
        .map_err(GitBackendInitError::InitRepository)?;
        Self::init_with_repo(store_path, git_repo_path, git_repo)
    }

    /// Initializes backend by creating a new Git repo at the specified
    /// workspace path. The workspace directory must exist.
    pub fn init_colocated(
        store_path: &Path,
        workspace_root: &Path,
    ) -> Result<Self, Box<GitBackendInitError>> {
        let canonical_workspace_root = {
            let path = store_path.join(workspace_root);
            path.canonicalize()
                .context(&path)
                .map_err(GitBackendInitError::Path)?
        };
        let git_repo = gix::ThreadSafeRepository::init(
            canonical_workspace_root,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
        )
        .map_err(GitBackendInitError::InitRepository)?;
        let git_repo_path = workspace_root.join(".git");
        Self::init_with_repo(store_path, &git_repo_path, git_repo)
    }

    /// Initializes backend with an existing Git repo at the specified path.
    pub fn init_external(
        store_path: &Path,
        git_repo_path: &Path,
    ) -> Result<Self, Box<GitBackendInitError>> {
        let canonical_git_repo_path = {
            let path = store_path.join(git_repo_path);
            path.canonicalize()
                .context(&path)
                .map_err(GitBackendInitError::Path)?
        };
        let git_repo = gix::ThreadSafeRepository::open(canonical_git_repo_path)
            .map_err(GitBackendInitError::OpenRepository)?;
        Self::init_with_repo(store_path, git_repo_path, git_repo)
    }

    fn init_with_repo(
        store_path: &Path,
        git_repo_path: &Path,
        git_repo: gix::ThreadSafeRepository,
    ) -> Result<Self, Box<GitBackendInitError>> {
        let extra_path = store_path.join("extra");
        fs::create_dir(&extra_path)
            .context(&extra_path)
            .map_err(GitBackendInitError::Path)?;
        let target_path = store_path.join("git_target");
        if cfg!(windows) && git_repo_path.is_relative() {
            // When a repository is created in Windows, format the path with *forward
            // slashes* and not backwards slashes. This makes it possible to use the same
            // repository under Windows Subsystem for Linux.
            //
            // This only works for relative paths. If the path is absolute, there's not much
            // we can do, and it simply won't work inside and outside WSL at the same time.
            let git_repo_path_string = git_repo_path
                .components()
                .map(|component| component.as_os_str().to_str().unwrap().to_owned())
                .join("/");
            fs::write(&target_path, git_repo_path_string.as_bytes())
                .context(&target_path)
                .map_err(GitBackendInitError::Path)?;
        } else {
            fs::write(&target_path, git_repo_path.to_str().unwrap().as_bytes())
                .context(&target_path)
                .map_err(GitBackendInitError::Path)?;
        };
        let extra_metadata_store = TableStore::init(extra_path, HASH_LENGTH);
        Ok(GitBackend::new(git_repo, extra_metadata_store))
    }

    pub fn load(store_path: &Path) -> Result<Self, Box<GitBackendLoadError>> {
        let git_repo_path = {
            let target_path = store_path.join("git_target");
            let git_repo_path_str = fs::read_to_string(&target_path)
                .context(&target_path)
                .map_err(GitBackendLoadError::Path)?;
            let git_repo_path = store_path.join(git_repo_path_str);
            git_repo_path
                .canonicalize()
                .context(&git_repo_path)
                .map_err(GitBackendLoadError::Path)?
        };
        let repo = gix::ThreadSafeRepository::open(git_repo_path)
            .map_err(GitBackendLoadError::OpenRepository)?;
        let extra_metadata_store = TableStore::load(store_path.join("extra"), HASH_LENGTH);
        Ok(GitBackend::new(repo, extra_metadata_store))
    }

    fn lock_git_repo(&self) -> MutexGuard<'_, gix::Repository> {
        self.repo.lock().unwrap()
    }

    /// Returns new thread-local instance to access to the underlying Git repo.
    pub fn git_repo(&self) -> gix::Repository {
        self.base_repo.to_thread_local()
    }

    /// Creates new owned git repository instance.
    pub fn open_git_repo(&self) -> Result<git2::Repository, git2::Error> {
        git2::Repository::open(self.git_repo_path())
    }

    /// Path to the `.git` directory or the repository itself if it's bare.
    pub fn git_repo_path(&self) -> &Path {
        self.base_repo.path()
    }

    /// Path to the working directory if the repository isn't bare.
    pub fn git_workdir(&self) -> Option<&Path> {
        self.base_repo.work_dir()
    }

    fn cached_extra_metadata_table(&self) -> BackendResult<Arc<ReadonlyTable>> {
        let mut locked_head = self.cached_extra_metadata.lock().unwrap();
        match locked_head.as_ref() {
            Some(head) => Ok(head.clone()),
            None => {
                let table = self
                    .extra_metadata_store
                    .get_head()
                    .map_err(GitBackendError::ReadMetadata)?;
                *locked_head = Some(table.clone());
                Ok(table)
            }
        }
    }

    fn read_extra_metadata_table_locked(&self) -> BackendResult<(Arc<ReadonlyTable>, FileLock)> {
        let table = self
            .extra_metadata_store
            .get_head_locked()
            .map_err(GitBackendError::ReadMetadata)?;
        Ok(table)
    }

    fn save_extra_metadata_table(
        &self,
        mut_table: MutableTable,
        _table_lock: &FileLock,
    ) -> BackendResult<()> {
        let table = self
            .extra_metadata_store
            .save_table(mut_table)
            .map_err(GitBackendError::WriteMetadata)?;
        // Since the parent table was the head, saved table are likely to be new head.
        // If it's not, cache will be reloaded when entry can't be found.
        *self.cached_extra_metadata.lock().unwrap() = Some(table);
        Ok(())
    }

    /// Imports the given commits and ancestors from the backing Git repo.
    #[tracing::instrument(skip(self, head_ids))]
    pub fn import_head_commits<'a>(
        &self,
        head_ids: impl IntoIterator<Item = &'a CommitId>,
        uses_tree_conflict_format: bool,
    ) -> BackendResult<()> {
        let table = self.cached_extra_metadata_table()?;
        let mut missing_head_ids = head_ids
            .into_iter()
            .filter(|&id| *id != self.root_commit_id && table.get_value(id.as_bytes()).is_none())
            .collect_vec();
        if missing_head_ids.is_empty() {
            return Ok(());
        }

        // These commits are imported from Git. Make our change ids persist (otherwise
        // future write_commit() could reassign new change id.)
        tracing::debug!(
            heads_count = missing_head_ids.len(),
            "import extra metadata entries"
        );
        let locked_repo = self.lock_git_repo();
        let (table, table_lock) = self.read_extra_metadata_table_locked()?;
        let mut mut_table = table.start_mutation();
        // Concurrent write_commit() might have updated the table before taking a lock.
        missing_head_ids.retain(|&id| mut_table.get_value(id.as_bytes()).is_none());
        import_extra_metadata_entries_from_heads(
            &locked_repo,
            &mut mut_table,
            &table_lock,
            &missing_head_ids,
            uses_tree_conflict_format,
        )?;
        for &id in &missing_head_ids {
            prevent_gc(&locked_repo, id)?;
        }
        self.save_extra_metadata_table(mut_table, &table_lock)
    }

    fn read_file_sync(&self, id: &FileId) -> BackendResult<Box<dyn Read>> {
        let git_blob_id = validate_git_object_id(id)?;
        let locked_repo = self.lock_git_repo();
        let mut blob = locked_repo
            .find_object(git_blob_id)
            .map_err(|err| map_not_found_err(err, id))?
            .try_into_blob()
            .map_err(|err| to_read_object_err(err, id))?;
        Ok(Box::new(Cursor::new(blob.take_data())))
    }
}

fn commit_from_git_without_root_parent(
    id: &CommitId,
    commit: &gix::objs::CommitRef,
    uses_tree_conflict_format: bool,
) -> Commit {
    // We reverse the bits of the commit id to create the change id. We don't want
    // to use the first bytes unmodified because then it would be ambiguous
    // if a given hash prefix refers to the commit id or the change id. It
    // would have been enough to pick the last 16 bytes instead of the
    // leading 16 bytes to address that. We also reverse the bits to make it less
    // likely that users depend on any relationship between the two ids.
    let change_id = ChangeId::new(
        id.as_bytes()[4..HASH_LENGTH]
            .iter()
            .rev()
            .map(|b| b.reverse_bits())
            .collect(),
    );
    let parents = commit
        .parents()
        .map(|oid| CommitId::from_bytes(oid.as_bytes()))
        .collect_vec();
    let tree_id = TreeId::from_bytes(commit.tree().as_bytes());
    // If this commit is a conflict, we'll update the root tree later, when we read
    // the extra metadata.
    let root_tree = if uses_tree_conflict_format {
        MergedTreeId::resolved(tree_id)
    } else {
        MergedTreeId::Legacy(tree_id)
    };
    // Use lossy conversion as commit message with "mojibake" is still better than
    // nothing.
    // TODO: what should we do with commit.encoding?
    let description = String::from_utf8_lossy(commit.message).into_owned();
    let author = signature_from_git(commit.author());
    let committer = signature_from_git(commit.committer());

    Commit {
        parents,
        predecessors: vec![],
        // If this commit has associated extra metadata, we may reset this later.
        root_tree,
        change_id,
        description,
        author,
        committer,
    }
}

const EMPTY_STRING_PLACEHOLDER: &str = "JJ_EMPTY_STRING";

fn signature_from_git(signature: gix::actor::SignatureRef) -> Signature {
    let name = signature.name;
    let name = if name != EMPTY_STRING_PLACEHOLDER {
        String::from_utf8_lossy(name).into_owned()
    } else {
        "".to_string()
    };
    let email = signature.email;
    let email = if email != EMPTY_STRING_PLACEHOLDER {
        String::from_utf8_lossy(email).into_owned()
    } else {
        "".to_string()
    };
    let timestamp = MillisSinceEpoch(signature.time.seconds * 1000);
    let tz_offset = signature.time.offset.div_euclid(60); // in minutes
    Signature {
        name,
        email,
        timestamp: Timestamp {
            timestamp,
            tz_offset,
        },
    }
}

fn signature_to_git(signature: &Signature) -> gix::actor::SignatureRef<'_> {
    // git does not support empty names or emails
    let name = if !signature.name.is_empty() {
        &signature.name
    } else {
        EMPTY_STRING_PLACEHOLDER
    };
    let email = if !signature.email.is_empty() {
        &signature.email
    } else {
        EMPTY_STRING_PLACEHOLDER
    };
    let time = gix::date::Time::new(
        signature.timestamp.timestamp.0.div_euclid(1000),
        signature.timestamp.tz_offset * 60, // in seconds
    );
    gix::actor::SignatureRef {
        name: name.into(),
        email: email.into(),
        time,
    }
}

fn serialize_extras(commit: &Commit) -> Vec<u8> {
    let mut proto = crate::protos::git_store::Commit {
        change_id: commit.change_id.to_bytes(),
        ..Default::default()
    };
    if let MergedTreeId::Merge(tree_ids) = &commit.root_tree {
        proto.uses_tree_conflict_format = true;
        if !tree_ids.is_resolved() {
            proto.root_tree = tree_ids.iter().map(|r| r.to_bytes()).collect();
        }
    }
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.to_bytes());
    }
    proto.encode_to_vec()
}

fn deserialize_extras(commit: &mut Commit, bytes: &[u8]) {
    let proto = crate::protos::git_store::Commit::decode(bytes).unwrap();
    commit.change_id = ChangeId::new(proto.change_id);
    if proto.uses_tree_conflict_format {
        if !proto.root_tree.is_empty() {
            let merge_builder: MergeBuilder<_> = proto
                .root_tree
                .iter()
                .map(|id_bytes| TreeId::from_bytes(id_bytes))
                .collect();
            commit.root_tree = MergedTreeId::Merge(merge_builder.build());
        } else {
            // uses_tree_conflict_format was set but there was no root_tree override in the
            // proto, which means we should just promote the tree id from the
            // git commit to be a known-conflict-free tree
            let MergedTreeId::Legacy(legacy_tree_id) = &commit.root_tree else {
                panic!("root tree should have been initialized to a legacy id");
            };
            commit.root_tree = MergedTreeId::resolved(legacy_tree_id.clone());
        }
    }
    for predecessor in &proto.predecessors {
        commit.predecessors.push(CommitId::from_bytes(predecessor));
    }
}

/// Creates a random ref in refs/jj/. Used for preventing GC of commits we
/// create.
fn create_no_gc_ref() -> String {
    let random_bytes: [u8; 16] = rand::random();
    format!("{NO_GC_REF_NAMESPACE}{}", hex::encode(random_bytes))
}

fn prevent_gc(git_repo: &gix::Repository, id: &CommitId) -> Result<(), BackendError> {
    git_repo
        .reference(
            format!("{NO_GC_REF_NAMESPACE}{}", id.hex()),
            validate_git_object_id(id).unwrap(),
            gix::refs::transaction::PreviousValue::Any,
            "used by jj",
        )
        .map_err(|err| BackendError::Other(Box::new(err)))?;
    Ok(())
}

fn validate_git_object_id(id: &impl ObjectId) -> Result<gix::ObjectId, BackendError> {
    if id.as_bytes().len() != HASH_LENGTH {
        return Err(BackendError::InvalidHashLength {
            expected: HASH_LENGTH,
            actual: id.as_bytes().len(),
            object_type: id.object_type(),
            hash: id.hex(),
        });
    }
    Ok(id.as_bytes().into())
}

fn map_not_found_err(err: gix::object::find::existing::Error, id: &impl ObjectId) -> BackendError {
    if matches!(err, gix::object::find::existing::Error::NotFound { .. }) {
        BackendError::ObjectNotFound {
            object_type: id.object_type(),
            hash: id.hex(),
            source: Box::new(err),
        }
    } else {
        to_read_object_err(err, id)
    }
}

fn to_read_object_err(
    err: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    id: &impl ObjectId,
) -> BackendError {
    BackendError::ReadObject {
        object_type: id.object_type(),
        hash: id.hex(),
        source: err.into(),
    }
}

fn to_invalid_utf8_err(source: str::Utf8Error, id: &impl ObjectId) -> BackendError {
    BackendError::InvalidUtf8 {
        object_type: id.object_type(),
        hash: id.hex(),
        source,
    }
}

fn import_extra_metadata_entries_from_heads(
    git_repo: &gix::Repository,
    mut_table: &mut MutableTable,
    _table_lock: &FileLock,
    missing_head_ids: &[&CommitId],
    uses_tree_conflict_format: bool,
) -> BackendResult<()> {
    let mut work_ids = missing_head_ids.iter().map(|&id| id.clone()).collect_vec();
    while let Some(id) = work_ids.pop() {
        let git_object = git_repo
            .find_object(validate_git_object_id(&id)?)
            .map_err(|err| map_not_found_err(err, &id))?;
        let git_commit = git_object
            .try_to_commit_ref()
            .map_err(|err| to_read_object_err(err, &id))?;
        // TODO(#1624): Should we read the root tree here and check if it has a
        // `.jjconflict-...` entries? That could happen if the user used `git` to e.g.
        // change the description of a commit with tree-level conflicts.
        let commit =
            commit_from_git_without_root_parent(&id, &git_commit, uses_tree_conflict_format);
        mut_table.add_entry(id.to_bytes(), serialize_extras(&commit));
        work_ids.extend(
            commit
                .parents
                .into_iter()
                .filter(|id| mut_table.get_value(id.as_bytes()).is_none()),
        );
    }
    Ok(())
}

impl Debug for GitBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("GitBackend")
            .field("path", &self.git_repo_path())
            .finish()
    }
}

#[async_trait]
impl Backend for GitBackend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        Self::name()
    }

    fn commit_id_length(&self) -> usize {
        HASH_LENGTH
    }

    fn change_id_length(&self) -> usize {
        CHANGE_ID_LENGTH
    }

    fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    fn root_change_id(&self) -> &ChangeId {
        &self.root_change_id
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    async fn read_file(&self, _path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        self.read_file_sync(id)
    }

    fn write_file(&self, _path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        let mut bytes = Vec::new();
        contents.read_to_end(&mut bytes).unwrap();
        let locked_repo = self.lock_git_repo();
        let oid = locked_repo
            .write_blob(bytes)
            .map_err(|err| BackendError::WriteObject {
                object_type: "file",
                source: Box::new(err),
            })?;
        Ok(FileId::new(oid.as_bytes().to_vec()))
    }

    async fn read_symlink(&self, _path: &RepoPath, id: &SymlinkId) -> Result<String, BackendError> {
        let git_blob_id = validate_git_object_id(id)?;
        let locked_repo = self.lock_git_repo();
        let mut blob = locked_repo
            .find_object(git_blob_id)
            .map_err(|err| map_not_found_err(err, id))?
            .try_into_blob()
            .map_err(|err| to_read_object_err(err, id))?;
        let target = String::from_utf8(blob.take_data())
            .map_err(|err| to_invalid_utf8_err(err.utf8_error(), id))?
            .to_owned();
        Ok(target)
    }

    fn write_symlink(&self, _path: &RepoPath, target: &str) -> Result<SymlinkId, BackendError> {
        let locked_repo = self.lock_git_repo();
        let oid =
            locked_repo
                .write_blob(target.as_bytes())
                .map_err(|err| BackendError::WriteObject {
                    object_type: "symlink",
                    source: Box::new(err),
                })?;
        Ok(SymlinkId::new(oid.as_bytes().to_vec()))
    }

    async fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        if id == &self.empty_tree_id {
            return Ok(Tree::default());
        }
        let git_tree_id = validate_git_object_id(id)?;

        let locked_repo = self.lock_git_repo();
        let git_tree = locked_repo
            .find_object(git_tree_id)
            .map_err(|err| map_not_found_err(err, id))?
            .try_into_tree()
            .map_err(|err| to_read_object_err(err, id))?;
        let mut tree = Tree::default();
        for entry in git_tree.iter() {
            let entry = entry.map_err(|err| to_read_object_err(err, id))?;
            let name =
                str::from_utf8(entry.filename()).map_err(|err| to_invalid_utf8_err(err, id))?;
            let (name, value) = match entry.mode() {
                gix::object::tree::EntryMode::Tree => {
                    let id = TreeId::from_bytes(entry.oid().as_bytes());
                    (name, TreeValue::Tree(id))
                }
                gix::object::tree::EntryMode::Blob => {
                    let id = FileId::from_bytes(entry.oid().as_bytes());
                    if let Some(basename) = name.strip_suffix(CONFLICT_SUFFIX) {
                        (
                            basename,
                            TreeValue::Conflict(ConflictId::from_bytes(entry.oid().as_bytes())),
                        )
                    } else {
                        (
                            name,
                            TreeValue::File {
                                id,
                                executable: false,
                            },
                        )
                    }
                }
                gix::object::tree::EntryMode::BlobExecutable => {
                    let id = FileId::from_bytes(entry.oid().as_bytes());
                    (
                        name,
                        TreeValue::File {
                            id,
                            executable: true,
                        },
                    )
                }
                gix::object::tree::EntryMode::Link => {
                    let id = SymlinkId::from_bytes(entry.oid().as_bytes());
                    (name, TreeValue::Symlink(id))
                }
                gix::object::tree::EntryMode::Commit => {
                    let id = CommitId::from_bytes(entry.oid().as_bytes());
                    (name, TreeValue::GitSubmodule(id))
                }
            };
            tree.set(RepoPathComponent::from(name), value);
        }
        Ok(tree)
    }

    fn write_tree(&self, _path: &RepoPath, contents: &Tree) -> BackendResult<TreeId> {
        // Tree entries to be written must be sorted by Entry::filename(), which
        // is slightly different from the order of our backend::Tree.
        let entries = contents
            .entries()
            .map(|entry| {
                let name = entry.name().string();
                match entry.value() {
                    TreeValue::File {
                        id,
                        executable: false,
                    } => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryMode::Blob,
                        filename: name.into(),
                        oid: id.as_bytes().into(),
                    },
                    TreeValue::File {
                        id,
                        executable: true,
                    } => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryMode::BlobExecutable,
                        filename: name.into(),
                        oid: id.as_bytes().into(),
                    },
                    TreeValue::Symlink(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryMode::Link,
                        filename: name.into(),
                        oid: id.as_bytes().into(),
                    },
                    TreeValue::Tree(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryMode::Tree,
                        filename: name.into(),
                        oid: id.as_bytes().into(),
                    },
                    TreeValue::GitSubmodule(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryMode::Commit,
                        filename: name.into(),
                        oid: id.as_bytes().into(),
                    },
                    TreeValue::Conflict(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryMode::Blob,
                        filename: (name + CONFLICT_SUFFIX).into(),
                        oid: id.as_bytes().into(),
                    },
                }
            })
            .sorted_unstable()
            .collect();
        let locked_repo = self.lock_git_repo();
        let oid = locked_repo
            .write_object(gix::objs::Tree { entries })
            .map_err(|err| BackendError::WriteObject {
                object_type: "tree",
                source: Box::new(err),
            })?;
        Ok(TreeId::from_bytes(oid.as_bytes()))
    }

    fn read_conflict(&self, _path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        let mut file = self.read_file_sync(&FileId::new(id.to_bytes()))?;
        let mut data = String::new();
        file.read_to_string(&mut data)
            .map_err(|err| BackendError::ReadObject {
                object_type: "conflict".to_owned(),
                hash: id.hex(),
                source: err.into(),
            })?;
        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        Ok(Conflict {
            removes: conflict_term_list_from_json(json.get("removes").unwrap()),
            adds: conflict_term_list_from_json(json.get("adds").unwrap()),
        })
    }

    fn write_conflict(&self, _path: &RepoPath, conflict: &Conflict) -> BackendResult<ConflictId> {
        let json = serde_json::json!({
            "removes": conflict_term_list_to_json(&conflict.removes),
            "adds": conflict_term_list_to_json(&conflict.adds),
        });
        let json_string = json.to_string();
        let bytes = json_string.as_bytes();
        let locked_repo = self.lock_git_repo();
        let oid = locked_repo
            .write_blob(bytes)
            .map_err(|err| BackendError::WriteObject {
                object_type: "conflict",
                source: Box::new(err),
            })?;
        Ok(ConflictId::from_bytes(oid.as_bytes()))
    }

    #[tracing::instrument(skip(self))]
    async fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if *id == self.root_commit_id {
            return Ok(make_root_commit(
                self.root_change_id().clone(),
                self.empty_tree_id.clone(),
            ));
        }
        let git_commit_id = validate_git_object_id(id)?;

        let mut commit = {
            let locked_repo = self.lock_git_repo();
            let git_object = locked_repo
                .find_object(git_commit_id)
                .map_err(|err| map_not_found_err(err, id))?;
            let git_commit = git_object
                .try_to_commit_ref()
                .map_err(|err| to_read_object_err(err, id))?;
            commit_from_git_without_root_parent(id, &git_commit, false)
        };
        if commit.parents.is_empty() {
            commit.parents.push(self.root_commit_id.clone());
        };

        let table = self.cached_extra_metadata_table()?;
        if let Some(extras) = table.get_value(id.as_bytes()) {
            deserialize_extras(&mut commit, extras);
        } else {
            // TODO: Remove this hack and map to ObjectNotFound error if we're sure that
            // there are no reachable ancestor commits without extras metadata. Git commits
            // imported by jj < 0.8.0 might not have extras (#924).
            // https://github.com/martinvonz/jj/issues/2343
            tracing::info!("unimported Git commit found");
            let uses_tree_conflict_format = false;
            self.import_head_commits([id], uses_tree_conflict_format)?;
            let table = self.cached_extra_metadata_table()?;
            let extras = table.get_value(id.as_bytes()).unwrap();
            deserialize_extras(&mut commit, extras);
        }
        Ok(commit)
    }

    fn write_commit(&self, mut contents: Commit) -> BackendResult<(CommitId, Commit)> {
        let locked_repo = self.lock_git_repo();
        let git_tree_id = match &contents.root_tree {
            MergedTreeId::Legacy(tree_id) => validate_git_object_id(tree_id)?,
            MergedTreeId::Merge(tree_ids) => match tree_ids.as_resolved() {
                Some(tree_id) => validate_git_object_id(tree_id)?,
                None => write_tree_conflict(&locked_repo, tree_ids)?,
            },
        };
        let author = signature_to_git(&contents.author);
        let mut committer = signature_to_git(&contents.committer);
        let message = &contents.description;
        if contents.parents.is_empty() {
            return Err(BackendError::Other(
                "Cannot write a commit with no parents".into(),
            ));
        }
        let mut parents = vec![];
        for parent_id in &contents.parents {
            if *parent_id == self.root_commit_id {
                // Git doesn't have a root commit, so if the parent is the root commit, we don't
                // add it to the list of parents to write in the Git commit. We also check that
                // there are no other parents since Git cannot represent a merge between a root
                // commit and another commit.
                if contents.parents.len() > 1 {
                    return Err(BackendError::Other(
                        "The Git backend does not support creating merge commits with the root \
                         commit as one of the parents."
                            .into(),
                    ));
                }
            } else {
                parents.push(validate_git_object_id(parent_id)?);
            }
        }
        let extras = serialize_extras(&contents);
        // If two writers write commits of the same id with different metadata, they
        // will both succeed and the metadata entries will be "merged" later. Since
        // metadata entry is keyed by the commit id, one of the entries would be lost.
        // To prevent such race condition locally, we extend the scope covered by the
        // table lock. This is still racy if multiple machines are involved and the
        // repository is rsync-ed.
        let (table, table_lock) = self.read_extra_metadata_table_locked()?;
        let id = loop {
            let git_id = locked_repo
                .commit_as(
                    committer,
                    author,
                    create_no_gc_ref(),
                    message,
                    git_tree_id,
                    parents.iter().copied(),
                )
                .map_err(|err| BackendError::WriteObject {
                    object_type: "commit",
                    source: Box::new(err),
                })?;
            let id = CommitId::from_bytes(git_id.as_bytes());
            match table.get_value(id.as_bytes()) {
                Some(existing_extras) if existing_extras != extras => {
                    // It's possible a commit already exists with the same commit id but different
                    // change id. Adjust the timestamp until this is no longer the case.
                    committer.time.seconds -= 1;
                }
                _ => {
                    break id;
                }
            }
        };
        // Update the signature to match the one that was actually written to the object
        // store
        contents.committer.timestamp.timestamp = MillisSinceEpoch(committer.time.seconds * 1000);
        let mut mut_table = table.start_mutation();
        mut_table.add_entry(id.to_bytes(), extras);
        self.save_extra_metadata_table(mut_table, &table_lock)?;
        Ok((id, contents))
    }
}

/// Write a tree conflict as a special tree with `.jjconflict-base-N` and
/// `.jjconflict-base-N` subtrees. This ensure that the parts are not GC'd.
fn write_tree_conflict(
    repo: &gix::Repository,
    conflict: &Merge<TreeId>,
) -> Result<gix::ObjectId, BackendError> {
    // Tree entries to be written must be sorted by Entry::filename().
    let entries = itertools::chain(
        conflict
            .removes()
            .iter()
            .enumerate()
            .map(|(i, tree_id)| (format!(".jjconflict-base-{i}"), tree_id)),
        conflict
            .adds()
            .iter()
            .enumerate()
            .map(|(i, tree_id)| (format!(".jjconflict-side-{i}"), tree_id)),
    )
    .map(|(name, tree_id)| gix::objs::tree::Entry {
        mode: gix::object::tree::EntryMode::Tree,
        filename: name.into(),
        oid: tree_id.as_bytes().into(),
    })
    .sorted_unstable()
    .collect();
    let id = repo
        .write_object(gix::objs::Tree { entries })
        .map_err(|err| BackendError::WriteObject {
            object_type: "tree",
            source: Box::new(err),
        })?;
    Ok(id.detach())
}

fn conflict_term_list_to_json(parts: &[ConflictTerm]) -> serde_json::Value {
    serde_json::Value::Array(parts.iter().map(conflict_term_to_json).collect())
}

fn conflict_term_list_from_json(json: &serde_json::Value) -> Vec<ConflictTerm> {
    json.as_array()
        .unwrap()
        .iter()
        .map(conflict_term_from_json)
        .collect()
}

fn conflict_term_to_json(part: &ConflictTerm) -> serde_json::Value {
    serde_json::json!({
        "value": tree_value_to_json(&part.value),
    })
}

fn conflict_term_from_json(json: &serde_json::Value) -> ConflictTerm {
    let json_value = json.get("value").unwrap();
    ConflictTerm {
        value: tree_value_from_json(json_value),
    }
}

fn tree_value_to_json(value: &TreeValue) -> serde_json::Value {
    match value {
        TreeValue::File { id, executable } => serde_json::json!({
             "file": {
                 "id": id.hex(),
                 "executable": executable,
             },
        }),
        TreeValue::Symlink(id) => serde_json::json!({
             "symlink_id": id.hex(),
        }),
        TreeValue::Tree(id) => serde_json::json!({
             "tree_id": id.hex(),
        }),
        TreeValue::GitSubmodule(id) => serde_json::json!({
             "submodule_id": id.hex(),
        }),
        TreeValue::Conflict(id) => serde_json::json!({
             "conflict_id": id.hex(),
        }),
    }
}

fn tree_value_from_json(json: &serde_json::Value) -> TreeValue {
    if let Some(json_file) = json.get("file") {
        TreeValue::File {
            id: FileId::new(bytes_vec_from_json(json_file.get("id").unwrap())),
            executable: json_file.get("executable").unwrap().as_bool().unwrap(),
        }
    } else if let Some(json_id) = json.get("symlink_id") {
        TreeValue::Symlink(SymlinkId::new(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("tree_id") {
        TreeValue::Tree(TreeId::new(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("submodule_id") {
        TreeValue::GitSubmodule(CommitId::new(bytes_vec_from_json(json_id)))
    } else if let Some(json_id) = json.get("conflict_id") {
        TreeValue::Conflict(ConflictId::new(bytes_vec_from_json(json_id)))
    } else {
        panic!("unexpected json value in conflict: {json:#?}");
    }
}

fn bytes_vec_from_json(value: &serde_json::Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap()).unwrap()
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use git2::Oid;
    use pollster::FutureExt;
    use test_case::test_case;

    use super::*;
    use crate::backend::{FileId, MillisSinceEpoch};

    #[test_case(false; "legacy tree format")]
    #[test_case(true; "tree-level conflict format")]
    fn read_plain_git_commit(uses_tree_conflict_format: bool) {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(&git_repo_path).unwrap();

        // Add a commit with some files in
        let blob1 = git_repo.blob(b"content1").unwrap();
        let blob2 = git_repo.blob(b"normal").unwrap();
        let mut dir_tree_builder = git_repo.treebuilder(None).unwrap();
        dir_tree_builder.insert("normal", blob1, 0o100644).unwrap();
        dir_tree_builder.insert("symlink", blob2, 0o120000).unwrap();
        let dir_tree_id = dir_tree_builder.write().unwrap();
        let mut root_tree_builder = git_repo.treebuilder(None).unwrap();
        root_tree_builder
            .insert("dir", dir_tree_id, 0o040000)
            .unwrap();
        let root_tree_id = root_tree_builder.write().unwrap();
        let git_author = git2::Signature::new(
            "git author",
            "git.author@example.com",
            &git2::Time::new(1000, 60),
        )
        .unwrap();
        let git_committer = git2::Signature::new(
            "git committer",
            "git.committer@example.com",
            &git2::Time::new(2000, -480),
        )
        .unwrap();
        let git_tree = git_repo.find_tree(root_tree_id).unwrap();
        let git_commit_id = git_repo
            .commit(
                None,
                &git_author,
                &git_committer,
                "git commit message",
                &git_tree,
                &[],
            )
            .unwrap();
        let commit_id = CommitId::from_hex("efdcea5ca4b3658149f899ca7feee6876d077263");
        // The change id is the leading reverse bits of the commit id
        let change_id = ChangeId::from_hex("c64ee0b6e16777fe53991f9281a6cd25");
        // Check that the git commit above got the hash we expect
        assert_eq!(git_commit_id.as_bytes(), commit_id.as_bytes());

        // Add an empty commit on top
        let git_commit_id2 = git_repo
            .commit(
                None,
                &git_author,
                &git_committer,
                "git commit message 2",
                &git_tree,
                &[&git_repo.find_commit(git_commit_id).unwrap()],
            )
            .unwrap();
        let commit_id2 = CommitId::from_bytes(git_commit_id2.as_bytes());

        let backend = GitBackend::init_external(store_path, &git_repo_path).unwrap();

        // Import the head commit and its ancestors
        backend
            .import_head_commits([&commit_id2], uses_tree_conflict_format)
            .unwrap();
        // Ref should be created only for the head commit
        let git_refs = backend
            .open_git_repo()
            .unwrap()
            .references_glob("refs/jj/keep/*")
            .unwrap()
            .map(|git_ref| git_ref.unwrap().target().unwrap())
            .collect_vec();
        assert_eq!(git_refs, vec![git_commit_id2]);

        let commit = backend.read_commit(&commit_id).block_on().unwrap();
        assert_eq!(&commit.change_id, &change_id);
        assert_eq!(commit.parents, vec![CommitId::from_bytes(&[0; 20])]);
        assert_eq!(commit.predecessors, vec![]);
        assert_eq!(
            commit.root_tree.to_merge(),
            Merge::resolved(TreeId::from_bytes(root_tree_id.as_bytes()))
        );
        if uses_tree_conflict_format {
            assert_matches!(commit.root_tree, MergedTreeId::Merge(_));
        } else {
            assert_matches!(commit.root_tree, MergedTreeId::Legacy(_));
        }
        assert_eq!(commit.description, "git commit message");
        assert_eq!(commit.author.name, "git author");
        assert_eq!(commit.author.email, "git.author@example.com");
        assert_eq!(
            commit.author.timestamp.timestamp,
            MillisSinceEpoch(1000 * 1000)
        );
        assert_eq!(commit.author.timestamp.tz_offset, 60);
        assert_eq!(commit.committer.name, "git committer");
        assert_eq!(commit.committer.email, "git.committer@example.com");
        assert_eq!(
            commit.committer.timestamp.timestamp,
            MillisSinceEpoch(2000 * 1000)
        );
        assert_eq!(commit.committer.timestamp.tz_offset, -480);

        let root_tree = backend
            .read_tree(
                &RepoPath::root(),
                &TreeId::from_bytes(root_tree_id.as_bytes()),
            )
            .block_on()
            .unwrap();
        let mut root_entries = root_tree.entries();
        let dir = root_entries.next().unwrap();
        assert_eq!(root_entries.next(), None);
        assert_eq!(dir.name().as_str(), "dir");
        assert_eq!(
            dir.value(),
            &TreeValue::Tree(TreeId::from_bytes(dir_tree_id.as_bytes()))
        );

        let dir_tree = backend
            .read_tree(
                &RepoPath::from_internal_string("dir"),
                &TreeId::from_bytes(dir_tree_id.as_bytes()),
            )
            .block_on()
            .unwrap();
        let mut entries = dir_tree.entries();
        let file = entries.next().unwrap();
        let symlink = entries.next().unwrap();
        assert_eq!(entries.next(), None);
        assert_eq!(file.name().as_str(), "normal");
        assert_eq!(
            file.value(),
            &TreeValue::File {
                id: FileId::from_bytes(blob1.as_bytes()),
                executable: false
            }
        );
        assert_eq!(symlink.name().as_str(), "symlink");
        assert_eq!(
            symlink.value(),
            &TreeValue::Symlink(SymlinkId::from_bytes(blob2.as_bytes()))
        );

        let commit2 = backend.read_commit(&commit_id2).block_on().unwrap();
        assert_eq!(commit2.parents, vec![commit_id.clone()]);
        assert_eq!(commit.predecessors, vec![]);
        assert_eq!(
            commit.root_tree.to_merge(),
            Merge::resolved(TreeId::from_bytes(root_tree_id.as_bytes()))
        );
        if uses_tree_conflict_format {
            assert_matches!(commit.root_tree, MergedTreeId::Merge(_));
        } else {
            assert_matches!(commit.root_tree, MergedTreeId::Legacy(_));
        }
    }

    #[test]
    fn read_git_commit_without_importing() {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(&git_repo_path).unwrap();

        let signature = git2::Signature::now("Someone", "someone@example.com").unwrap();
        let empty_tree_id = Oid::from_str("4b825dc642cb6eb9a060e54bf8d69288fbee4904").unwrap();
        let empty_tree = git_repo.find_tree(empty_tree_id).unwrap();
        let git_commit_id = git_repo
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "git commit message",
                &empty_tree,
                &[],
            )
            .unwrap();

        let backend = GitBackend::init_external(store_path, &git_repo_path).unwrap();

        // read_commit() without import_head_commits() works as of now. This might be
        // changed later.
        assert!(backend
            .read_commit(&CommitId::from_bytes(git_commit_id.as_bytes()))
            .block_on()
            .is_ok());
        assert!(
            backend
                .cached_extra_metadata_table()
                .unwrap()
                .get_value(git_commit_id.as_bytes())
                .is_some(),
            "extra metadata should have been be created"
        );
    }

    #[test]
    fn read_empty_string_placeholder() {
        let git_signature1 = gix::actor::SignatureRef {
            name: EMPTY_STRING_PLACEHOLDER.into(),
            email: "git.author@example.com".into(),
            time: gix::date::Time::new(1000, 60 * 60),
        };
        let signature1 = signature_from_git(git_signature1);
        assert!(signature1.name.is_empty());
        assert_eq!(signature1.email, "git.author@example.com");
        let git_signature2 = gix::actor::SignatureRef {
            name: "git committer".into(),
            email: EMPTY_STRING_PLACEHOLDER.into(),
            time: gix::date::Time::new(2000, -480 * 60),
        };
        let signature2 = signature_from_git(git_signature2);
        assert_eq!(signature2.name, "git committer");
        assert!(signature2.email.is_empty());
    }

    #[test]
    fn write_empty_string_placeholder() {
        let signature1 = Signature {
            name: "".to_string(),
            email: "someone@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        };
        let git_signature1 = signature_to_git(&signature1);
        assert_eq!(git_signature1.name, EMPTY_STRING_PLACEHOLDER);
        assert_eq!(git_signature1.email, "someone@example.com");
        let signature2 = Signature {
            name: "Someone".to_string(),
            email: "".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        };
        let git_signature2 = signature_to_git(&signature2);
        assert_eq!(git_signature2.name, "Someone");
        assert_eq!(git_signature2.email, EMPTY_STRING_PLACEHOLDER);
    }

    /// Test that parents get written correctly
    #[test]
    fn git_commit_parents() {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(&git_repo_path).unwrap();

        let backend = GitBackend::init_external(store_path, &git_repo_path).unwrap();
        let mut commit = Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: MergedTreeId::Legacy(backend.empty_tree_id().clone()),
            change_id: ChangeId::from_hex("abc123"),
            description: "".to_string(),
            author: create_signature(),
            committer: create_signature(),
        };

        // No parents
        commit.parents = vec![];
        assert_matches!(
            backend.write_commit(commit.clone()),
            Err(BackendError::Other(err)) if err.to_string().contains("no parents")
        );

        // Only root commit as parent
        commit.parents = vec![backend.root_commit_id().clone()];
        let first_id = backend.write_commit(commit.clone()).unwrap().0;
        let first_commit = backend.read_commit(&first_id).block_on().unwrap();
        assert_eq!(first_commit, commit);
        let first_git_commit = git_repo.find_commit(git_id(&first_id)).unwrap();
        assert_eq!(first_git_commit.parent_ids().collect_vec(), vec![]);

        // Only non-root commit as parent
        commit.parents = vec![first_id.clone()];
        let second_id = backend.write_commit(commit.clone()).unwrap().0;
        let second_commit = backend.read_commit(&second_id).block_on().unwrap();
        assert_eq!(second_commit, commit);
        let second_git_commit = git_repo.find_commit(git_id(&second_id)).unwrap();
        assert_eq!(
            second_git_commit.parent_ids().collect_vec(),
            vec![git_id(&first_id)]
        );

        // Merge commit
        commit.parents = vec![first_id.clone(), second_id.clone()];
        let merge_id = backend.write_commit(commit.clone()).unwrap().0;
        let merge_commit = backend.read_commit(&merge_id).block_on().unwrap();
        assert_eq!(merge_commit, commit);
        let merge_git_commit = git_repo.find_commit(git_id(&merge_id)).unwrap();
        assert_eq!(
            merge_git_commit.parent_ids().collect_vec(),
            vec![git_id(&first_id), git_id(&second_id)]
        );

        // Merge commit with root as one parent
        commit.parents = vec![first_id, backend.root_commit_id().clone()];
        assert_matches!(
            backend.write_commit(commit),
            Err(BackendError::Other(err)) if err.to_string().contains("root commit")
        );
    }

    #[test]
    fn write_tree_conflicts() {
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(&git_repo_path).unwrap();

        let backend = GitBackend::init_external(store_path, &git_repo_path).unwrap();
        let create_tree = |i| {
            let blob_id = git_repo.blob(b"content {i}").unwrap();
            let mut tree_builder = git_repo.treebuilder(None).unwrap();
            tree_builder
                .insert(format!("file{i}"), blob_id, 0o100644)
                .unwrap();
            TreeId::from_bytes(tree_builder.write().unwrap().as_bytes())
        };

        let root_tree = Merge::new(
            vec![create_tree(0), create_tree(1)],
            vec![create_tree(2), create_tree(3), create_tree(4)],
        );
        let mut commit = Commit {
            parents: vec![backend.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: MergedTreeId::Merge(root_tree.clone()),
            change_id: ChangeId::from_hex("abc123"),
            description: "".to_string(),
            author: create_signature(),
            committer: create_signature(),
        };

        // When writing a tree-level conflict, the root tree on the git side has the
        // individual trees as subtrees.
        let read_commit_id = backend.write_commit(commit.clone()).unwrap().0;
        let read_commit = backend.read_commit(&read_commit_id).block_on().unwrap();
        assert_eq!(read_commit, commit);
        let git_commit = git_repo
            .find_commit(Oid::from_bytes(read_commit_id.as_bytes()).unwrap())
            .unwrap();
        let git_tree = git_repo.find_tree(git_commit.tree_id()).unwrap();
        assert!(git_tree.iter().all(|entry| entry.filemode() == 0o040000));
        let mut iter = git_tree.iter();
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-base-0"));
        assert_eq!(entry.id().as_bytes(), root_tree.removes()[0].as_bytes());
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-base-1"));
        assert_eq!(entry.id().as_bytes(), root_tree.removes()[1].as_bytes());
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-side-0"));
        assert_eq!(entry.id().as_bytes(), root_tree.adds()[0].as_bytes());
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-side-1"));
        assert_eq!(entry.id().as_bytes(), root_tree.adds()[1].as_bytes());
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-side-2"));
        assert_eq!(entry.id().as_bytes(), root_tree.adds()[2].as_bytes());
        assert!(iter.next().is_none());

        // When writing a single tree using the new format, it's represented by a
        // regular git tree.
        commit.root_tree = MergedTreeId::resolved(create_tree(5));
        let read_commit_id = backend.write_commit(commit.clone()).unwrap().0;
        let read_commit = backend.read_commit(&read_commit_id).block_on().unwrap();
        assert_eq!(read_commit, commit);
        let git_commit = git_repo
            .find_commit(Oid::from_bytes(read_commit_id.as_bytes()).unwrap())
            .unwrap();
        assert_eq!(
            MergedTreeId::resolved(TreeId::from_bytes(git_commit.tree_id().as_bytes())),
            commit.root_tree
        );
    }

    #[test]
    fn commit_has_ref() {
        let temp_dir = testutils::new_temp_dir();
        let backend = GitBackend::init_internal(temp_dir.path()).unwrap();
        let signature = Signature {
            name: "Someone".to_string(),
            email: "someone@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        };
        let commit = Commit {
            parents: vec![backend.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: MergedTreeId::Legacy(backend.empty_tree_id().clone()),
            change_id: ChangeId::new(vec![]),
            description: "initial".to_string(),
            author: signature.clone(),
            committer: signature,
        };
        let commit_id = backend.write_commit(commit).unwrap().0;
        let git_refs = backend
            .open_git_repo()
            .unwrap()
            .references_glob("refs/jj/keep/*")
            .unwrap()
            .map(|git_ref| git_ref.unwrap().target().unwrap())
            .collect_vec();
        assert_eq!(git_refs, vec![git_id(&commit_id)]);
    }

    #[test]
    fn overlapping_git_commit_id() {
        let temp_dir = testutils::new_temp_dir();
        let backend = GitBackend::init_internal(temp_dir.path()).unwrap();
        let mut commit1 = Commit {
            parents: vec![backend.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: MergedTreeId::Legacy(backend.empty_tree_id().clone()),
            change_id: ChangeId::new(vec![]),
            description: "initial".to_string(),
            author: create_signature(),
            committer: create_signature(),
        };
        // libgit2 doesn't seem to preserve negative timestamps, so set it to at least 1
        // second after the epoch, so the timestamp adjustment can remove 1
        // second and it will still be nonnegative
        commit1.committer.timestamp.timestamp = MillisSinceEpoch(1000);
        let (commit_id1, mut commit2) = backend.write_commit(commit1).unwrap();
        commit2.predecessors.push(commit_id1.clone());
        // `write_commit` should prevent the ids from being the same by changing the
        // committer timestamp of the commit it actually writes.
        let (commit_id2, mut actual_commit2) = backend.write_commit(commit2.clone()).unwrap();
        // The returned matches the ID
        assert_eq!(
            backend.read_commit(&commit_id2).block_on().unwrap(),
            actual_commit2
        );
        assert_ne!(commit_id2, commit_id1);
        // The committer timestamp should differ
        assert_ne!(
            actual_commit2.committer.timestamp.timestamp,
            commit2.committer.timestamp.timestamp
        );
        // The rest of the commit should be the same
        actual_commit2.committer.timestamp.timestamp =
            commit2.committer.timestamp.timestamp.clone();
        assert_eq!(actual_commit2, commit2);
    }

    fn git_id(commit_id: &CommitId) -> Oid {
        Oid::from_bytes(commit_id.as_bytes()).unwrap()
    }

    fn create_signature() -> Signature {
        Signature {
            name: "Someone".to_string(),
            email: "someone@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        }
    }
}
