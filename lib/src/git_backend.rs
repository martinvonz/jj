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
use std::collections::HashSet;
use std::fmt::{Debug, Error, Formatter};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;
use std::{fs, io, str};

use async_trait::async_trait;
use gix::bstr::BString;
use gix::objs::{CommitRef, CommitRefIter, WriteTo};
use itertools::Itertools;
use prost::Message;
use smallvec::SmallVec;
use thiserror::Error;

use crate::backend::{
    make_root_commit, Backend, BackendError, BackendInitError, BackendLoadError, BackendResult,
    ChangeId, Commit, CommitId, Conflict, ConflictId, ConflictTerm, FileId, MergedTreeId,
    MillisSinceEpoch, SecureSig, Signature, SigningFn, SymlinkId, Timestamp, Tree, TreeId,
    TreeValue,
};
use crate::file_util::{IoResultExt as _, PathError};
use crate::index::Index;
use crate::lock::FileLock;
use crate::merge::{Merge, MergeBuilder};
use crate::object_id::ObjectId;
use crate::repo_path::{RepoPath, RepoPathComponentBuf};
use crate::settings::UserSettings;
use crate::stacked_table::{
    MutableTable, ReadonlyTable, TableSegment, TableStore, TableStoreError,
};

const HASH_LENGTH: usize = 20;
const CHANGE_ID_LENGTH: usize = 16;
/// Ref namespace used only for preventing GC.
const NO_GC_REF_NAMESPACE: &str = "refs/jj/keep/";
const CONFLICT_SUFFIX: &str = ".jjconflict";

const JJ_TREES_COMMIT_HEADER: &[u8] = b"jj:trees";

#[derive(Debug, Error)]
pub enum GitBackendInitError {
    #[error("Failed to initialize git repository")]
    InitRepository(#[source] gix::init::Error),
    #[error("Failed to open git repository")]
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
    #[error("Failed to open git repository")]
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
    #[error("Failed to read non-git metadata")]
    ReadMetadata(#[source] TableStoreError),
    #[error("Failed to write non-git metadata")]
    WriteMetadata(#[source] TableStoreError),
}

impl From<GitBackendError> for BackendError {
    fn from(err: GitBackendError) -> Self {
        BackendError::Other(err.into())
    }
}

#[derive(Debug, Error)]
pub enum GitGcError {
    #[error("Failed to run git gc command")]
    GcCommand(#[source] std::io::Error),
    #[error("git gc command exited with an error: {0}")]
    GcCommandErrorStatus(ExitStatus),
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
    /// Whether tree of imported commit should be promoted to non-legacy format.
    imported_commit_uses_tree_conflict_format: bool,
}

impl GitBackend {
    pub fn name() -> &'static str {
        "git"
    }

    fn new(
        base_repo: gix::ThreadSafeRepository,
        extra_metadata_store: TableStore,
        imported_commit_uses_tree_conflict_format: bool,
    ) -> Self {
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
            imported_commit_uses_tree_conflict_format,
        }
    }

    pub fn init_internal(
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Self, Box<GitBackendInitError>> {
        let git_repo_path = Path::new("git");
        let git_repo = gix::ThreadSafeRepository::init_opts(
            store_path.join(git_repo_path),
            gix::create::Kind::Bare,
            gix::create::Options::default(),
            gix_open_opts_from_settings(settings),
        )
        .map_err(GitBackendInitError::InitRepository)?;
        Self::init_with_repo(settings, store_path, git_repo_path, git_repo)
    }

    /// Initializes backend by creating a new Git repo at the specified
    /// workspace path. The workspace directory must exist.
    pub fn init_colocated(
        settings: &UserSettings,
        store_path: &Path,
        workspace_root: &Path,
    ) -> Result<Self, Box<GitBackendInitError>> {
        let canonical_workspace_root = {
            let path = store_path.join(workspace_root);
            path.canonicalize()
                .context(&path)
                .map_err(GitBackendInitError::Path)?
        };
        let git_repo = gix::ThreadSafeRepository::init_opts(
            canonical_workspace_root,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix_open_opts_from_settings(settings),
        )
        .map_err(GitBackendInitError::InitRepository)?;
        let git_repo_path = workspace_root.join(".git");
        Self::init_with_repo(settings, store_path, &git_repo_path, git_repo)
    }

    /// Initializes backend with an existing Git repo at the specified path.
    pub fn init_external(
        settings: &UserSettings,
        store_path: &Path,
        git_repo_path: &Path,
    ) -> Result<Self, Box<GitBackendInitError>> {
        let canonical_git_repo_path = {
            let path = store_path.join(git_repo_path);
            canonicalize_git_repo_path(&path)
                .context(&path)
                .map_err(GitBackendInitError::Path)?
        };
        let git_repo = gix::ThreadSafeRepository::open_opts(
            canonical_git_repo_path,
            gix_open_opts_from_settings(settings),
        )
        .map_err(GitBackendInitError::OpenRepository)?;
        Self::init_with_repo(settings, store_path, git_repo_path, git_repo)
    }

    fn init_with_repo(
        settings: &UserSettings,
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
        Ok(GitBackend::new(
            git_repo,
            extra_metadata_store,
            settings.use_tree_conflict_format(),
        ))
    }

    pub fn load(
        settings: &UserSettings,
        store_path: &Path,
    ) -> Result<Self, Box<GitBackendLoadError>> {
        let git_repo_path = {
            let target_path = store_path.join("git_target");
            let git_repo_path_str = fs::read_to_string(&target_path)
                .context(&target_path)
                .map_err(GitBackendLoadError::Path)?;
            let git_repo_path = store_path.join(git_repo_path_str);
            canonicalize_git_repo_path(&git_repo_path)
                .context(&git_repo_path)
                .map_err(GitBackendLoadError::Path)?
        };
        let repo = gix::ThreadSafeRepository::open_opts(
            git_repo_path,
            gix_open_opts_from_settings(settings),
        )
        .map_err(GitBackendLoadError::OpenRepository)?;
        let extra_metadata_store = TableStore::load(store_path.join("extra"), HASH_LENGTH);
        Ok(GitBackend::new(
            repo,
            extra_metadata_store,
            settings.use_tree_conflict_format(),
        ))
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
    ///
    /// The `head_ids` may contain commits that have already been imported, but
    /// the caller should filter them out to eliminate redundant I/O processing.
    #[tracing::instrument(skip(self, head_ids))]
    pub fn import_head_commits<'a>(
        &self,
        head_ids: impl IntoIterator<Item = &'a CommitId>,
    ) -> BackendResult<()> {
        let head_ids: HashSet<&CommitId> = head_ids
            .into_iter()
            .filter(|&id| *id != self.root_commit_id)
            .collect();
        if head_ids.is_empty() {
            return Ok(());
        }

        // Create no-gc ref even if known to the extras table. Concurrent GC
        // process might have deleted the no-gc ref.
        let locked_repo = self.lock_git_repo();
        locked_repo
            .edit_references(head_ids.iter().copied().map(to_no_gc_ref_update))
            .map_err(|err| BackendError::Other(Box::new(err)))?;

        // These commits are imported from Git. Make our change ids persist (otherwise
        // future write_commit() could reassign new change id.)
        tracing::debug!(
            heads_count = head_ids.len(),
            "import extra metadata entries"
        );
        let (table, table_lock) = self.read_extra_metadata_table_locked()?;
        let mut mut_table = table.start_mutation();
        import_extra_metadata_entries_from_heads(
            &locked_repo,
            &mut mut_table,
            &table_lock,
            &head_ids,
            self.imported_commit_uses_tree_conflict_format,
        )?;
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

/// Canonicalizes the given `path` except for the last `".git"` component.
///
/// The last path component matters when opening a Git repo without `core.bare`
/// config. This config is usually set, but the "repo" tool will set up such
/// repositories and symlinks. Opening such repo with fully-canonicalized path
/// would turn a colocated Git repo into a bare repo.
pub fn canonicalize_git_repo_path(path: &Path) -> io::Result<PathBuf> {
    if path.ends_with(".git") {
        let workdir = path.parent().unwrap();
        workdir.canonicalize().map(|dir| dir.join(".git"))
    } else {
        path.canonicalize()
    }
}

fn gix_open_opts_from_settings(settings: &UserSettings) -> gix::open::Options {
    let user_name = settings.user_name();
    let user_email = settings.user_email();
    gix::open::Options::default()
        .config_overrides([
            // Committer has to be configured to record reflog. Author isn't
            // needed, but let's copy the same values.
            format!("author.name={user_name}"),
            format!("author.email={user_email}"),
            format!("committer.name={user_name}"),
            format!("committer.email={user_email}"),
        ])
        // The git_target path should point the repository, not the working directory.
        .open_path_as_is(true)
}

/// Reads the `jj:trees` header from the commit.
fn root_tree_from_header(git_commit: &CommitRef) -> Result<Option<MergedTreeId>, ()> {
    for (key, value) in &git_commit.extra_headers {
        if *key == JJ_TREES_COMMIT_HEADER {
            let mut tree_ids = SmallVec::new();
            for hex in str::from_utf8(value.as_ref()).or(Err(()))?.split(' ') {
                let tree_id = TreeId::try_from_hex(hex).or(Err(()))?;
                if tree_id.as_bytes().len() != HASH_LENGTH {
                    return Err(());
                }
                tree_ids.push(tree_id);
            }
            if tree_ids.len() % 2 == 0 {
                return Err(());
            }
            return Ok(Some(MergedTreeId::Merge(Merge::from_vec(tree_ids))));
        }
    }
    Ok(None)
}

fn commit_from_git_without_root_parent(
    id: &CommitId,
    git_object: &gix::Object,
    uses_tree_conflict_format: bool,
) -> Result<Commit, BackendError> {
    let commit = git_object
        .try_to_commit_ref()
        .map_err(|err| to_read_object_err(err, id))?;

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
    let root_tree = root_tree_from_header(&commit)
        .map_err(|()| to_read_object_err("Invalid jj:trees header", id))?;
    let root_tree = root_tree.unwrap_or_else(|| {
        if uses_tree_conflict_format {
            MergedTreeId::resolved(tree_id)
        } else {
            MergedTreeId::Legacy(tree_id)
        }
    });
    // Use lossy conversion as commit message with "mojibake" is still better than
    // nothing.
    // TODO: what should we do with commit.encoding?
    let description = String::from_utf8_lossy(commit.message).into_owned();
    let author = signature_from_git(commit.author());
    let committer = signature_from_git(commit.committer());

    // If the commit is signed, extract both the signature and the signed data
    // (which is the commit buffer with the gpgsig header omitted).
    // We have to re-parse the raw commit data because gix CommitRef does not give
    // us the sogned data, only the signature.
    // Ideally, we could use try_to_commit_ref_iter at the beginning of this
    // function and extract everything from that. For now, this works
    let secure_sig = commit
        .extra_headers
        .iter()
        // gix does not recognize gpgsig-sha256, but prevent future footguns by checking for it too
        .any(|(k, _)| *k == "gpgsig" || *k == "gpgsig-sha256")
        .then(|| CommitRefIter::signature(&git_object.data))
        .transpose()
        .map_err(|err| to_read_object_err(err, id))?
        .flatten()
        .map(|(sig, data)| SecureSig {
            data: data.to_bstring().into(),
            sig: sig.into_owned().into(),
        });

    Ok(Commit {
        parents,
        predecessors: vec![],
        // If this commit has associated extra metadata, we may reset this later.
        root_tree,
        change_id,
        description,
        author,
        committer,
        secure_sig,
    })
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
            let merge = merge_builder.build();
            // Check that the trees from the extras match the one we found in the jj:trees
            // header
            if let MergedTreeId::Merge(existing_merge) = &commit.root_tree {
                assert!(existing_merge.is_resolved() || *existing_merge == merge);
            }
            commit.root_tree = MergedTreeId::Merge(merge);
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

/// Returns `RefEdit` that will create a ref in `refs/jj/keep` if not exist.
/// Used for preventing GC of commits we create.
fn to_no_gc_ref_update(id: &CommitId) -> gix::refs::transaction::RefEdit {
    let name = format!("{NO_GC_REF_NAMESPACE}{}", id.hex());
    let new = gix::refs::Target::Peeled(validate_git_object_id(id).unwrap());
    let expected = gix::refs::transaction::PreviousValue::ExistingMustMatch(new.clone());
    gix::refs::transaction::RefEdit {
        change: gix::refs::transaction::Change::Update {
            log: gix::refs::transaction::LogChange {
                message: "used by jj".into(),
                ..Default::default()
            },
            expected,
            new,
        },
        name: name.try_into().unwrap(),
        deref: false,
    }
}

fn to_ref_deletion(git_ref: gix::refs::Reference) -> gix::refs::transaction::RefEdit {
    let expected = gix::refs::transaction::PreviousValue::ExistingMustMatch(git_ref.target);
    gix::refs::transaction::RefEdit {
        change: gix::refs::transaction::Change::Delete {
            expected,
            log: gix::refs::transaction::RefLog::AndReference,
        },
        name: git_ref.name,
        deref: false,
    }
}

/// Recreates `refs/jj/keep` refs for the `new_heads`, and removes the other
/// unreachable and non-head refs.
fn recreate_no_gc_refs(
    git_repo: &gix::Repository,
    new_heads: impl IntoIterator<Item = CommitId>,
    keep_newer: SystemTime,
) -> Result<(), BackendError> {
    // Calculate diff between existing no-gc refs and new heads.
    let new_heads: HashSet<CommitId> = new_heads.into_iter().collect();
    let mut no_gc_refs_to_keep_count: usize = 0;
    let mut no_gc_refs_to_delete: Vec<gix::refs::Reference> = Vec::new();
    let git_references = git_repo
        .references()
        .map_err(|err| BackendError::Other(err.into()))?;
    let no_gc_refs_iter = git_references
        .prefixed(NO_GC_REF_NAMESPACE)
        .map_err(|err| BackendError::Other(err.into()))?;
    for git_ref in no_gc_refs_iter {
        let git_ref = git_ref.map_err(BackendError::Other)?.detach();
        let oid = git_ref.target.try_id().ok_or_else(|| {
            let name = git_ref.name.as_bstr();
            BackendError::Other(format!("Symbolic no-gc ref found: {name}").into())
        })?;
        let id = CommitId::from_bytes(oid.as_bytes());
        let name_good = git_ref.name.as_bstr()[NO_GC_REF_NAMESPACE.len()..] == id.hex();
        if new_heads.contains(&id) && name_good {
            no_gc_refs_to_keep_count += 1;
            continue;
        }
        // Check timestamp of loose ref, but this is still racy on re-import
        // because:
        // - existing packed ref won't be demoted to loose ref
        // - existing loose ref won't be touched
        //
        // TODO: might be better to switch to a dummy merge, where new no-gc ref
        // will always have a unique name. Doing that with the current
        // ref-per-head strategy would increase the number of the no-gc refs.
        // https://github.com/martinvonz/jj/pull/2659#issuecomment-1837057782
        let loose_ref_path = git_repo.path().join(git_ref.name.to_path());
        if let Ok(metadata) = loose_ref_path.metadata() {
            let mtime = metadata.modified().expect("unsupported platform?");
            if mtime > keep_newer {
                tracing::trace!(?git_ref, "not deleting new");
                no_gc_refs_to_keep_count += 1;
                continue;
            }
        }
        // Also deletes no-gc ref of random name created by old jj.
        tracing::trace!(?git_ref, ?name_good, "will delete");
        no_gc_refs_to_delete.push(git_ref);
    }
    tracing::info!(
        new_heads_count = new_heads.len(),
        no_gc_refs_to_keep_count,
        no_gc_refs_to_delete_count = no_gc_refs_to_delete.len(),
        "collected reachable refs"
    );

    // It's slow to delete packed refs one by one, so update refs all at once.
    let ref_edits = itertools::chain(
        no_gc_refs_to_delete.into_iter().map(to_ref_deletion),
        new_heads.iter().map(to_no_gc_ref_update),
    );
    git_repo
        .edit_references(ref_edits)
        .map_err(|err| BackendError::Other(err.into()))?;

    Ok(())
}

fn run_git_gc(git_dir: &Path) -> Result<(), GitGcError> {
    let mut git = Command::new("git");
    git.arg("--git-dir=."); // turn off discovery
    git.arg("gc");
    // Don't specify it by GIT_DIR/--git-dir. On Windows, the "\\?\" path might
    // not be supported by git.
    git.current_dir(git_dir);
    // TODO: pass output to UI layer instead of printing directly here
    let status = git.status().map_err(GitGcError::GcCommand)?;
    if !status.success() {
        return Err(GitGcError::GcCommandErrorStatus(status));
    }
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
    Ok(id.as_bytes().try_into().unwrap())
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
    head_ids: &HashSet<&CommitId>,
    uses_tree_conflict_format: bool,
) -> BackendResult<()> {
    let mut work_ids = head_ids
        .iter()
        .filter(|&id| mut_table.get_value(id.as_bytes()).is_none())
        .map(|&id| id.clone())
        .collect_vec();
    while let Some(id) = work_ids.pop() {
        let git_object = git_repo
            .find_object(validate_git_object_id(&id)?)
            .map_err(|err| map_not_found_err(err, &id))?;
        // TODO(#1624): Should we read the root tree here and check if it has a
        // `.jjconflict-...` entries? That could happen if the user used `git` to e.g.
        // change the description of a commit with tree-level conflicts.
        let commit =
            commit_from_git_without_root_parent(&id, &git_object, uses_tree_conflict_format)?;
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

    fn concurrency(&self) -> usize {
        1
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
            let (name, value) = match entry.mode().kind() {
                gix::object::tree::EntryKind::Tree => {
                    let id = TreeId::from_bytes(entry.oid().as_bytes());
                    (name, TreeValue::Tree(id))
                }
                gix::object::tree::EntryKind::Blob => {
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
                gix::object::tree::EntryKind::BlobExecutable => {
                    let id = FileId::from_bytes(entry.oid().as_bytes());
                    (
                        name,
                        TreeValue::File {
                            id,
                            executable: true,
                        },
                    )
                }
                gix::object::tree::EntryKind::Link => {
                    let id = SymlinkId::from_bytes(entry.oid().as_bytes());
                    (name, TreeValue::Symlink(id))
                }
                gix::object::tree::EntryKind::Commit => {
                    let id = CommitId::from_bytes(entry.oid().as_bytes());
                    (name, TreeValue::GitSubmodule(id))
                }
            };
            tree.set(RepoPathComponentBuf::from(name), value);
        }
        Ok(tree)
    }

    fn write_tree(&self, _path: &RepoPath, contents: &Tree) -> BackendResult<TreeId> {
        // Tree entries to be written must be sorted by Entry::filename(), which
        // is slightly different from the order of our backend::Tree.
        let entries = contents
            .entries()
            .map(|entry| {
                let name = entry.name().as_str();
                match entry.value() {
                    TreeValue::File {
                        id,
                        executable: false,
                    } => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryKind::Blob.into(),
                        filename: name.into(),
                        oid: id.as_bytes().try_into().unwrap(),
                    },
                    TreeValue::File {
                        id,
                        executable: true,
                    } => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryKind::BlobExecutable.into(),
                        filename: name.into(),
                        oid: id.as_bytes().try_into().unwrap(),
                    },
                    TreeValue::Symlink(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryKind::Link.into(),
                        filename: name.into(),
                        oid: id.as_bytes().try_into().unwrap(),
                    },
                    TreeValue::Tree(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryKind::Tree.into(),
                        filename: name.into(),
                        oid: id.as_bytes().try_into().unwrap(),
                    },
                    TreeValue::GitSubmodule(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryKind::Commit.into(),
                        filename: name.into(),
                        oid: id.as_bytes().try_into().unwrap(),
                    },
                    TreeValue::Conflict(id) => gix::objs::tree::Entry {
                        mode: gix::object::tree::EntryKind::Blob.into(),
                        filename: (name.to_owned() + CONFLICT_SUFFIX).into(),
                        oid: id.as_bytes().try_into().unwrap(),
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
            commit_from_git_without_root_parent(id, &git_object, false)?
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
            self.import_head_commits([id])?;
            let table = self.cached_extra_metadata_table()?;
            let extras = table.get_value(id.as_bytes()).unwrap();
            deserialize_extras(&mut commit, extras);
        }
        Ok(commit)
    }

    fn write_commit(
        &self,
        mut contents: Commit,
        mut sign_with: Option<&mut SigningFn>,
    ) -> BackendResult<(CommitId, Commit)> {
        assert!(contents.secure_sig.is_none(), "commit.secure_sig was set");

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
        let mut parents = SmallVec::new();
        for parent_id in &contents.parents {
            if *parent_id == self.root_commit_id {
                // Git doesn't have a root commit, so if the parent is the root commit, we don't
                // add it to the list of parents to write in the Git commit. We also check that
                // there are no other parents since Git cannot represent a merge between a root
                // commit and another commit.
                if contents.parents.len() > 1 {
                    return Err(BackendError::Unsupported(
                        "The Git backend does not support creating merge commits with the root \
                         commit as one of the parents."
                            .to_owned(),
                    ));
                }
            } else {
                parents.push(validate_git_object_id(parent_id)?);
            }
        }
        let mut extra_headers = vec![];
        if let MergedTreeId::Merge(tree_ids) = &contents.root_tree {
            if !tree_ids.is_resolved() {
                let value = tree_ids.iter().map(|id| id.hex()).join(" ").into_bytes();
                extra_headers.push((
                    BString::new(JJ_TREES_COMMIT_HEADER.to_vec()),
                    BString::new(value),
                ));
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
            let mut commit = gix::objs::Commit {
                message: message.to_owned().into(),
                tree: git_tree_id,
                author: author.into(),
                committer: committer.into(),
                encoding: None,
                parents: parents.clone(),
                extra_headers: extra_headers.clone(),
            };

            if let Some(sign) = &mut sign_with {
                // we don't use gix pool, but at least use their heuristic
                let mut data = Vec::with_capacity(512);
                commit.write_to(&mut data).unwrap();

                let sig = sign(&data).map_err(|err| BackendError::WriteObject {
                    object_type: "commit",
                    source: Box::new(err),
                })?;
                commit
                    .extra_headers
                    .push(("gpgsig".into(), sig.clone().into()));
                contents.secure_sig = Some(SecureSig { data, sig });
            }

            let git_id =
                locked_repo
                    .write_object(&commit)
                    .map_err(|err| BackendError::WriteObject {
                        object_type: "commit",
                        source: Box::new(err),
                    })?;

            match table.get_value(git_id.as_bytes()) {
                Some(existing_extras) if existing_extras != extras => {
                    // It's possible a commit already exists with the same commit id but different
                    // change id. Adjust the timestamp until this is no longer the case.
                    committer.time.seconds -= 1;
                }
                _ => break CommitId::from_bytes(git_id.as_bytes()),
            }
        };

        // Everything up to this point had no permanent effect on the repo except
        // GC-able objects
        locked_repo
            .edit_reference(to_no_gc_ref_update(&id))
            .map_err(|err| BackendError::Other(Box::new(err)))?;

        // Update the signature to match the one that was actually written to the object
        // store
        contents.committer.timestamp.timestamp = MillisSinceEpoch(committer.time.seconds * 1000);
        let mut mut_table = table.start_mutation();
        mut_table.add_entry(id.to_bytes(), extras);
        self.save_extra_metadata_table(mut_table, &table_lock)?;
        Ok((id, contents))
    }

    #[tracing::instrument(skip(self, index))]
    fn gc(&self, index: &dyn Index, keep_newer: SystemTime) -> BackendResult<()> {
        let git_repo = self.lock_git_repo();
        let new_heads = index
            .all_heads_for_gc()
            .map_err(|err| BackendError::Other(err.into()))?
            .filter(|id| *id != self.root_commit_id);
        recreate_no_gc_refs(&git_repo, new_heads, keep_newer)?;
        // TODO: remove unreachable entries from extras table if segment file
        // mtime <= keep_newer? (it won't be consistent with no-gc refs
        // preserved by the keep_newer timestamp though)
        // TODO: remove unreachable extras table segments
        // TODO: pass in keep_newer to "git gc" command
        run_git_gc(self.git_repo_path()).map_err(|err| BackendError::Other(err.into()))
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
            .enumerate()
            .map(|(i, tree_id)| (format!(".jjconflict-base-{i}"), tree_id)),
        conflict
            .adds()
            .enumerate()
            .map(|(i, tree_id)| (format!(".jjconflict-side-{i}"), tree_id)),
    )
    .map(|(name, tree_id)| gix::objs::tree::Entry {
        mode: gix::object::tree::EntryKind::Tree.into(),
        filename: name.into(),
        oid: tree_id.as_bytes().try_into().unwrap(),
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
    use hex::ToHex;
    use pollster::FutureExt;
    use test_case::test_case;

    use super::*;
    use crate::content_hash::blake2b_hash;

    #[test_case(false; "legacy tree format")]
    #[test_case(true; "tree-level conflict format")]
    fn read_plain_git_commit(uses_tree_conflict_format: bool) {
        let settings = {
            let config = config::Config::builder()
                .set_override("format.tree-level-conflicts", uses_tree_conflict_format)
                .unwrap()
                .build()
                .unwrap();
            UserSettings::from_config(config)
        };
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(git_repo_path).unwrap();

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

        let backend = GitBackend::init_external(&settings, store_path, git_repo.path()).unwrap();

        // Import the head commit and its ancestors
        backend.import_head_commits([&commit_id2]).unwrap();
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
                RepoPath::root(),
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
                RepoPath::from_internal_string("dir"),
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
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(git_repo_path).unwrap();

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

        let backend = GitBackend::init_external(&settings, store_path, git_repo.path()).unwrap();

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
    fn read_signed_git_commit() {
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(git_repo_path).unwrap();

        let signature = git2::Signature::now("Someone", "someone@example.com").unwrap();
        let empty_tree_id = Oid::from_str("4b825dc642cb6eb9a060e54bf8d69288fbee4904").unwrap();
        let empty_tree = git_repo.find_tree(empty_tree_id).unwrap();

        let commit_buf = git_repo
            .commit_create_buffer(
                &signature,
                &signature,
                "git commit message",
                &empty_tree,
                &[],
            )
            .unwrap();

        // libgit2-rs works with &strs here for some reason
        let commit_buf = std::str::from_utf8(&commit_buf).unwrap();
        let secure_sig =
            "here are some ASCII bytes to be used as a test signature\n\ndefinitely not PGP";

        let git_commit_id = git_repo
            .commit_signed(commit_buf, secure_sig, None)
            .unwrap();

        let backend = GitBackend::init_external(&settings, store_path, git_repo.path()).unwrap();

        let commit = backend
            .read_commit(&CommitId::from_bytes(git_commit_id.as_bytes()))
            .block_on()
            .unwrap();

        let sig = commit.secure_sig.expect("failed to read the signature");

        // converting to string for nicer assert diff
        assert_eq!(std::str::from_utf8(&sig.sig).unwrap(), secure_sig);
        assert_eq!(std::str::from_utf8(&sig.data).unwrap(), commit_buf);
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
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(git_repo_path).unwrap();

        let backend = GitBackend::init_external(&settings, store_path, git_repo.path()).unwrap();
        let mut commit = Commit {
            parents: vec![],
            predecessors: vec![],
            root_tree: MergedTreeId::Legacy(backend.empty_tree_id().clone()),
            change_id: ChangeId::from_hex("abc123"),
            description: "".to_string(),
            author: create_signature(),
            committer: create_signature(),
            secure_sig: None,
        };

        // No parents
        commit.parents = vec![];
        assert_matches!(
            backend.write_commit(commit.clone(), None),
            Err(BackendError::Other(err)) if err.to_string().contains("no parents")
        );

        // Only root commit as parent
        commit.parents = vec![backend.root_commit_id().clone()];
        let first_id = backend.write_commit(commit.clone(), None).unwrap().0;
        let first_commit = backend.read_commit(&first_id).block_on().unwrap();
        assert_eq!(first_commit, commit);
        let first_git_commit = git_repo.find_commit(git_id(&first_id)).unwrap();
        assert_eq!(first_git_commit.parent_ids().collect_vec(), vec![]);

        // Only non-root commit as parent
        commit.parents = vec![first_id.clone()];
        let second_id = backend.write_commit(commit.clone(), None).unwrap().0;
        let second_commit = backend.read_commit(&second_id).block_on().unwrap();
        assert_eq!(second_commit, commit);
        let second_git_commit = git_repo.find_commit(git_id(&second_id)).unwrap();
        assert_eq!(
            second_git_commit.parent_ids().collect_vec(),
            vec![git_id(&first_id)]
        );

        // Merge commit
        commit.parents = vec![first_id.clone(), second_id.clone()];
        let merge_id = backend.write_commit(commit.clone(), None).unwrap().0;
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
            backend.write_commit(commit, None),
            Err(BackendError::Unsupported(message)) if message.contains("root commit")
        );
    }

    #[test]
    fn write_tree_conflicts() {
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let store_path = temp_dir.path();
        let git_repo_path = temp_dir.path().join("git");
        let git_repo = git2::Repository::init(git_repo_path).unwrap();

        let backend = GitBackend::init_external(&settings, store_path, git_repo.path()).unwrap();
        let create_tree = |i| {
            let blob_id = git_repo.blob(b"content {i}").unwrap();
            let mut tree_builder = git_repo.treebuilder(None).unwrap();
            tree_builder
                .insert(format!("file{i}"), blob_id, 0o100644)
                .unwrap();
            TreeId::from_bytes(tree_builder.write().unwrap().as_bytes())
        };

        let root_tree = Merge::from_removes_adds(
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
            secure_sig: None,
        };

        // When writing a tree-level conflict, the root tree on the git side has the
        // individual trees as subtrees.
        let read_commit_id = backend.write_commit(commit.clone(), None).unwrap().0;
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
        assert_eq!(
            entry.id().as_bytes(),
            root_tree.get_remove(0).unwrap().as_bytes()
        );
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-base-1"));
        assert_eq!(
            entry.id().as_bytes(),
            root_tree.get_remove(1).unwrap().as_bytes()
        );
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-side-0"));
        assert_eq!(
            entry.id().as_bytes(),
            root_tree.get_add(0).unwrap().as_bytes()
        );
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-side-1"));
        assert_eq!(
            entry.id().as_bytes(),
            root_tree.get_add(1).unwrap().as_bytes()
        );
        let entry = iter.next().unwrap();
        assert_eq!(entry.name(), Some(".jjconflict-side-2"));
        assert_eq!(
            entry.id().as_bytes(),
            root_tree.get_add(2).unwrap().as_bytes()
        );
        assert!(iter.next().is_none());

        // When writing a single tree using the new format, it's represented by a
        // regular git tree.
        commit.root_tree = MergedTreeId::resolved(create_tree(5));
        let read_commit_id = backend.write_commit(commit.clone(), None).unwrap().0;
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
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let backend = GitBackend::init_internal(&settings, temp_dir.path()).unwrap();
        let git_repo = backend.open_git_repo().unwrap();
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
            secure_sig: None,
        };
        let commit_id = backend.write_commit(commit, None).unwrap().0;
        let git_refs: Vec<_> = git_repo
            .references_glob("refs/jj/keep/*")
            .unwrap()
            .try_collect()
            .unwrap();
        assert!(git_refs
            .iter()
            .any(|git_ref| git_ref.target().unwrap() == git_id(&commit_id)));

        // Concurrently-running GC deletes the ref, leaving the extra metadata.
        for mut git_ref in git_refs {
            git_ref.delete().unwrap();
        }
        // Re-imported commit should have new ref.
        backend.import_head_commits([&commit_id]).unwrap();
        let git_refs: Vec<_> = git_repo
            .references_glob("refs/jj/keep/*")
            .unwrap()
            .try_collect()
            .unwrap();
        assert!(git_refs
            .iter()
            .any(|git_ref| git_ref.target().unwrap() == git_id(&commit_id)));
    }

    #[test]
    fn import_head_commits_duplicates() {
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let backend = GitBackend::init_internal(&settings, temp_dir.path()).unwrap();
        let git_repo = backend.open_git_repo().unwrap();

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
        let commit_id = CommitId::from_bytes(git_commit_id.as_bytes());

        // Ref creation shouldn't fail because of duplicated head ids.
        backend
            .import_head_commits([&commit_id, &commit_id])
            .unwrap();
        let git_refs: Vec<_> = git_repo
            .references_glob("refs/jj/keep/*")
            .unwrap()
            .try_collect()
            .unwrap();
        assert!(git_refs
            .iter()
            .any(|git_ref| git_ref.target().unwrap() == git_commit_id));
    }

    #[test]
    fn overlapping_git_commit_id() {
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let backend = GitBackend::init_internal(&settings, temp_dir.path()).unwrap();
        let mut commit1 = Commit {
            parents: vec![backend.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: MergedTreeId::Legacy(backend.empty_tree_id().clone()),
            change_id: ChangeId::new(vec![]),
            description: "initial".to_string(),
            author: create_signature(),
            committer: create_signature(),
            secure_sig: None,
        };
        // libgit2 doesn't seem to preserve negative timestamps, so set it to at least 1
        // second after the epoch, so the timestamp adjustment can remove 1
        // second and it will still be nonnegative
        commit1.committer.timestamp.timestamp = MillisSinceEpoch(1000);
        let (commit_id1, mut commit2) = backend.write_commit(commit1, None).unwrap();
        commit2.predecessors.push(commit_id1.clone());
        // `write_commit` should prevent the ids from being the same by changing the
        // committer timestamp of the commit it actually writes.
        let (commit_id2, mut actual_commit2) = backend.write_commit(commit2.clone(), None).unwrap();
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
        actual_commit2.committer.timestamp.timestamp = commit2.committer.timestamp.timestamp;
        assert_eq!(actual_commit2, commit2);
    }

    #[test]
    fn write_signed_commit() {
        let settings = user_settings();
        let temp_dir = testutils::new_temp_dir();
        let backend = GitBackend::init_internal(&settings, temp_dir.path()).unwrap();

        let commit = Commit {
            parents: vec![backend.root_commit_id().clone()],
            predecessors: vec![],
            root_tree: MergedTreeId::Legacy(backend.empty_tree_id().clone()),
            change_id: ChangeId::new(vec![]),
            description: "initial".to_string(),
            author: create_signature(),
            committer: create_signature(),
            secure_sig: None,
        };

        let mut signer = |data: &_| {
            let hash: String = blake2b_hash(data).encode_hex();
            Ok(format!("test sig\n\n\nhash={hash}").into_bytes())
        };

        let (id, commit) = backend
            .write_commit(commit, Some(&mut signer as &mut SigningFn))
            .unwrap();

        let git_repo = backend.git_repo();
        let obj = git_repo
            .find_object(gix::ObjectId::try_from(id.as_bytes()).unwrap())
            .unwrap();
        insta::assert_snapshot!(std::str::from_utf8(&obj.data).unwrap(), @r###"
        tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
        author Someone <someone@example.com> 0 +0000
        committer Someone <someone@example.com> 0 +0000
        gpgsig test sig
         
         
         hash=9ad9526c3b2103c41a229f2f3c82d107a0ecd902f476a855f0e1dd5f7bef1430663de12749b73e293a877113895a8a2a0f29da4bbc5a5f9a19c3523fb0e53518

        initial
        "###);

        let returned_sig = commit.secure_sig.expect("failed to return the signature");

        let commit = backend.read_commit(&id).block_on().unwrap();

        let sig = commit.secure_sig.expect("failed to read the signature");
        assert_eq!(&sig, &returned_sig);

        insta::assert_snapshot!(std::str::from_utf8(&sig.sig).unwrap(), @r###"
        test sig


        hash=9ad9526c3b2103c41a229f2f3c82d107a0ecd902f476a855f0e1dd5f7bef1430663de12749b73e293a877113895a8a2a0f29da4bbc5a5f9a19c3523fb0e53518
        "###);
        insta::assert_snapshot!(std::str::from_utf8(&sig.data).unwrap(), @r###"
        tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
        author Someone <someone@example.com> 0 +0000
        committer Someone <someone@example.com> 0 +0000

        initial
        "###);
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

    // Not using testutils::user_settings() because there is a dependency cycle
    // 'jj_lib (1) -> testutils -> jj_lib (2)' which creates another distinct
    // UserSettings type. testutils returns jj_lib (2)'s UserSettings, whereas
    // our UserSettings type comes from jj_lib (1).
    fn user_settings() -> UserSettings {
        let config = config::Config::default();
        UserSettings::from_config(config)
    }
}
