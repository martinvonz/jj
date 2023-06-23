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

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use itertools::Itertools;
use once_cell::unsync::OnceCell;
use prost::Message;
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::backend::{
    BackendError, ConflictId, FileId, MillisSinceEpoch, ObjectId, SymlinkId, TreeId, TreeValue,
};
#[cfg(feature = "watchman")]
use crate::fsmonitor::watchman;
use crate::fsmonitor::FsmonitorKind;
use crate::gitignore::GitIgnoreFile;
use crate::lock::FileLock;
use crate::matchers::{
    DifferenceMatcher, EverythingMatcher, IntersectionMatcher, Matcher, PrefixMatcher,
};
use crate::op_store::{OperationId, WorkspaceId};
use crate::repo_path::{FsPathParseError, RepoPath, RepoPathComponent, RepoPathJoin};
use crate::store::Store;
use crate::tree::{Diff, Tree};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FileType {
    Normal { executable: bool },
    Symlink,
    GitSubmodule,
    Conflict,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FileState {
    pub file_type: FileType,
    pub mtime: MillisSinceEpoch,
    pub size: u64,
    /* TODO: What else do we need here? Git stores a lot of fields.
     * TODO: Could possibly handle case-insensitive file systems keeping an
     *       Option<PathBuf> with the actual path here. */
}

impl FileState {
    fn for_file(executable: bool, size: u64, metadata: &Metadata) -> Self {
        FileState {
            file_type: FileType::Normal { executable },
            mtime: mtime_from_metadata(metadata),
            size,
        }
    }

    fn for_symlink(metadata: &Metadata) -> Self {
        // When using fscrypt, the reported size is not the content size. So if
        // we were to record the content size here (like we do for regular files), we
        // would end up thinking the file has changed every time we snapshot.
        FileState {
            file_type: FileType::Symlink,
            mtime: mtime_from_metadata(metadata),
            size: metadata.len(),
        }
    }

    fn for_conflict(size: u64, metadata: &Metadata) -> Self {
        FileState {
            file_type: FileType::Conflict,
            mtime: mtime_from_metadata(metadata),
            size,
        }
    }

    fn for_gitsubmodule() -> Self {
        FileState {
            file_type: FileType::GitSubmodule,
            mtime: MillisSinceEpoch(0),
            size: 0,
        }
    }

    #[cfg_attr(unix, allow(dead_code))]
    fn is_executable(&self) -> bool {
        if let FileType::Normal { executable } = &self.file_type {
            *executable
        } else {
            false
        }
    }

    fn mark_executable(&mut self, executable: bool) {
        if let FileType::Normal { .. } = &self.file_type {
            self.file_type = FileType::Normal { executable }
        }
    }
}

pub struct TreeState {
    store: Arc<Store>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    tree_id: TreeId,
    file_states: BTreeMap<RepoPath, FileState>,
    // Currently only path prefixes
    sparse_patterns: Vec<RepoPath>,
    own_mtime: MillisSinceEpoch,

    /// The most recent clock value returned by Watchman. Will only be set if
    /// the repo is configured to use the Watchman filesystem monitor and
    /// Watchman has been queried at least once.
    watchman_clock: Option<crate::protos::working_copy::WatchmanClock>,
}

fn file_state_from_proto(proto: crate::protos::working_copy::FileState) -> FileState {
    let file_type = match proto.file_type() {
        crate::protos::working_copy::FileType::Normal => FileType::Normal { executable: false },
        crate::protos::working_copy::FileType::Executable => FileType::Normal { executable: true },
        crate::protos::working_copy::FileType::Symlink => FileType::Symlink,
        crate::protos::working_copy::FileType::Conflict => FileType::Conflict,
        crate::protos::working_copy::FileType::GitSubmodule => FileType::GitSubmodule,
    };
    FileState {
        file_type,
        mtime: MillisSinceEpoch(proto.mtime_millis_since_epoch),
        size: proto.size,
    }
}

fn file_state_to_proto(file_state: &FileState) -> crate::protos::working_copy::FileState {
    let mut proto = crate::protos::working_copy::FileState::default();
    let file_type = match &file_state.file_type {
        FileType::Normal { executable: false } => crate::protos::working_copy::FileType::Normal,
        FileType::Normal { executable: true } => crate::protos::working_copy::FileType::Executable,
        FileType::Symlink => crate::protos::working_copy::FileType::Symlink,
        FileType::Conflict => crate::protos::working_copy::FileType::Conflict,
        FileType::GitSubmodule => crate::protos::working_copy::FileType::GitSubmodule,
    };
    proto.file_type = file_type as i32;
    proto.mtime_millis_since_epoch = file_state.mtime.0;
    proto.size = file_state.size;
    proto
}

fn file_states_from_proto(
    proto: &crate::protos::working_copy::TreeState,
) -> BTreeMap<RepoPath, FileState> {
    let mut file_states = BTreeMap::new();
    for (path_str, proto_file_state) in &proto.file_states {
        let path = RepoPath::from_internal_string(path_str.as_str());
        file_states.insert(path, file_state_from_proto(proto_file_state.clone()));
    }
    file_states
}

fn sparse_patterns_from_proto(proto: &crate::protos::working_copy::TreeState) -> Vec<RepoPath> {
    let mut sparse_patterns = vec![];
    if let Some(proto_sparse_patterns) = proto.sparse_patterns.as_ref() {
        for prefix in &proto_sparse_patterns.prefixes {
            sparse_patterns.push(RepoPath::from_internal_string(prefix.as_str()));
        }
    } else {
        // For compatibility with old working copies.
        // TODO: Delete this is late 2022 or so.
        sparse_patterns.push(RepoPath::root());
    }
    sparse_patterns
}

/// Creates intermediate directories from the `working_copy_path` to the
/// `repo_path` parent.
///
/// If an intermediate directory exists and if it is a symlink, this function
/// will return an error. The `working_copy_path` directory may be a symlink.
///
/// Note that this does not prevent TOCTOU bugs caused by concurrent checkouts.
/// Another process may remove the directory created by this function and put a
/// symlink there.
fn create_parent_dirs(working_copy_path: &Path, repo_path: &RepoPath) -> Result<(), CheckoutError> {
    let (_, dir_components) = repo_path
        .components()
        .split_last()
        .expect("repo path shouldn't be root");
    let mut dir_path = working_copy_path.to_owned();
    for c in dir_components {
        dir_path.push(c.as_str());
        match fs::create_dir(&dir_path) {
            Ok(()) => {}
            Err(_)
                if dir_path
                    .symlink_metadata()
                    .map(|m| m.is_dir())
                    .unwrap_or(false) => {}
            Err(err) => {
                return Err(CheckoutError::IoError {
                    message: format!(
                        "Failed to create parent directories for {}",
                        repo_path.to_fs_path(working_copy_path).display(),
                    ),
                    err,
                });
            }
        }
    }
    Ok(())
}

fn mtime_from_metadata(metadata: &Metadata) -> MillisSinceEpoch {
    let time = metadata
        .modified()
        .expect("File mtime not supported on this platform?");
    let since_epoch = time
        .duration_since(UNIX_EPOCH)
        .expect("mtime before unix epoch");

    MillisSinceEpoch(
        i64::try_from(since_epoch.as_millis())
            .expect("mtime billions of years into the future or past"),
    )
}

fn file_state(metadata: &Metadata) -> Option<FileState> {
    let metadata_file_type = metadata.file_type();
    let file_type = if metadata_file_type.is_dir() {
        None
    } else if metadata_file_type.is_symlink() {
        Some(FileType::Symlink)
    } else if metadata_file_type.is_file() {
        #[cfg(unix)]
        let mode = metadata.permissions().mode();
        #[cfg(windows)]
        let mode = 0;
        if mode & 0o111 != 0 {
            Some(FileType::Normal { executable: true })
        } else {
            Some(FileType::Normal { executable: false })
        }
    } else {
        None
    };
    file_type.map(|file_type| {
        let mtime = mtime_from_metadata(metadata);
        let size = metadata.len();
        FileState {
            file_type,
            mtime,
            size,
        }
    })
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CheckoutStats {
    pub updated_files: u32,
    pub added_files: u32,
    pub removed_files: u32,
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("Failed to open file {path}: {err:?}")]
    FileOpenError { path: PathBuf, err: std::io::Error },
    #[error("Failed to query the filesystem monitor: {0}")]
    FsmonitorError(String),
    #[error("{message}: {err}")]
    IoError {
        message: String,
        #[source]
        err: std::io::Error,
    },
    #[error("Working copy path {} is not valid UTF-8", path.to_string_lossy())]
    InvalidUtf8Path { path: OsString },
    #[error("Symlink {path} target is not valid UTF-8")]
    InvalidUtf8SymlinkTarget { path: PathBuf, target: PathBuf },
    #[error("Internal backend error: {0}")]
    InternalBackendError(#[from] BackendError),
}

#[derive(Debug, Error)]
pub enum CheckoutError {
    // The current working-copy commit was deleted, maybe by an overly aggressive GC that happened
    // while the current process was running.
    #[error("Current working-copy commit not found: {source}")]
    SourceNotFound {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    // Another process checked out a commit while the current process was running (after the
    // working copy was read by the current process).
    #[error("Concurrent checkout")]
    ConcurrentCheckout,
    #[error("{message}: {err:?}")]
    IoError {
        message: String,
        #[source]
        err: std::io::Error,
    },
    #[error("Internal error: {0}")]
    InternalBackendError(#[from] BackendError),
}

impl CheckoutError {
    fn for_stat_error(err: std::io::Error, path: &Path) -> Self {
        CheckoutError::IoError {
            message: format!("Failed to stat file {}", path.display()),
            err,
        }
    }
}

fn suppress_file_exists_error(orig_err: CheckoutError) -> Result<(), CheckoutError> {
    match orig_err {
        CheckoutError::IoError { err, .. } if err.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(())
        }
        _ => Err(orig_err),
    }
}

pub struct SnapshotOptions<'a> {
    pub base_ignores: Arc<GitIgnoreFile>,
    pub fsmonitor_kind: Option<FsmonitorKind>,
    pub progress: Option<&'a SnapshotProgress<'a>>,
}

impl SnapshotOptions<'_> {
    pub fn empty_for_test() -> Self {
        SnapshotOptions {
            base_ignores: GitIgnoreFile::empty(),
            fsmonitor_kind: None,
            progress: None,
        }
    }
}

struct FsmonitorMatcher {
    matcher: Option<Box<dyn Matcher>>,
    watchman_clock: Option<crate::protos::working_copy::WatchmanClock>,
}

#[derive(Debug, Error)]
pub enum ResetError {
    // The current working-copy commit was deleted, maybe by an overly aggressive GC that happened
    // while the current process was running.
    #[error("Current working-copy commit not found: {source}")]
    SourceNotFound {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Internal error: {0}")]
    InternalBackendError(#[from] BackendError),
}

impl TreeState {
    pub fn current_tree_id(&self) -> &TreeId {
        &self.tree_id
    }

    pub fn file_states(&self) -> &BTreeMap<RepoPath, FileState> {
        &self.file_states
    }

    pub fn sparse_patterns(&self) -> &Vec<RepoPath> {
        &self.sparse_patterns
    }

    fn sparse_matcher(&self) -> Box<dyn Matcher> {
        Box::new(PrefixMatcher::new(&self.sparse_patterns))
    }

    pub fn init(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> TreeState {
        let mut wc = TreeState::empty(store, working_copy_path, state_path);
        wc.save();
        wc
    }

    fn empty(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> TreeState {
        let tree_id = store.empty_tree_id().clone();
        // Canonicalize the working copy path because "repo/." makes libgit2 think that
        // everything should be ignored
        TreeState {
            store,
            working_copy_path: working_copy_path.canonicalize().unwrap(),
            state_path,
            tree_id,
            file_states: BTreeMap::new(),
            sparse_patterns: vec![RepoPath::root()],
            own_mtime: MillisSinceEpoch(0),
            watchman_clock: None,
        }
    }

    pub fn load(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> TreeState {
        let maybe_file = File::open(state_path.join("tree_state"));
        let file = match maybe_file {
            Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
                return TreeState::init(store, working_copy_path, state_path);
            }
            result => result.unwrap(),
        };

        let mut wc = TreeState::empty(store, working_copy_path, state_path);
        wc.read(file);
        wc
    }

    fn update_own_mtime(&mut self) {
        if let Ok(metadata) = self.state_path.join("tree_state").symlink_metadata() {
            self.own_mtime = mtime_from_metadata(&metadata);
        } else {
            self.own_mtime = MillisSinceEpoch(0);
        }
    }

    fn read(&mut self, mut file: File) {
        self.update_own_mtime();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        let proto = crate::protos::working_copy::TreeState::decode(&*buf).unwrap();
        self.tree_id = TreeId::new(proto.tree_id.clone());
        self.file_states = file_states_from_proto(&proto);
        self.sparse_patterns = sparse_patterns_from_proto(&proto);
        self.watchman_clock = proto.watchman_clock;
    }

    fn save(&mut self) {
        let mut proto = crate::protos::working_copy::TreeState {
            tree_id: self.tree_id.to_bytes(),
            ..Default::default()
        };
        for (file, file_state) in &self.file_states {
            proto.file_states.insert(
                file.to_internal_file_string(),
                file_state_to_proto(file_state),
            );
        }
        let mut sparse_patterns = crate::protos::working_copy::SparsePatterns::default();
        for path in &self.sparse_patterns {
            sparse_patterns
                .prefixes
                .push(path.to_internal_file_string());
        }
        proto.sparse_patterns = Some(sparse_patterns);
        proto.watchman_clock = self.watchman_clock.clone();

        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        temp_file
            .as_file_mut()
            .write_all(&proto.encode_to_vec())
            .unwrap();
        // update own write time while we before we rename it, so we know
        // there is no unknown data in it
        self.update_own_mtime();
        // TODO: Retry if persisting fails (it will on Windows if the file happened to
        // be open for read).
        temp_file
            .persist(self.state_path.join("tree_state"))
            .unwrap();
    }

    fn write_file_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<FileId, SnapshotError> {
        let file = File::open(disk_path).map_err(|err| SnapshotError::IoError {
            message: format!("Failed to open file {}", disk_path.display()),
            err,
        })?;
        Ok(self.store.write_file(path, &mut Box::new(file))?)
    }

    fn write_symlink_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<SymlinkId, SnapshotError> {
        let target = disk_path
            .read_link()
            .map_err(|err| SnapshotError::IoError {
                message: format!("Failed to read symlink {}", disk_path.display()),
                err,
            })?;
        let str_target =
            target
                .to_str()
                .ok_or_else(|| SnapshotError::InvalidUtf8SymlinkTarget {
                    path: disk_path.to_path_buf(),
                    target: target.clone(),
                })?;
        Ok(self.store.write_symlink(path, str_target)?)
    }

    fn reset_watchman(&mut self) {
        self.watchman_clock.take();
    }

    #[cfg(feature = "watchman")]
    #[tokio::main]
    pub async fn query_watchman(
        &self,
    ) -> Result<(watchman::Clock, Option<Vec<PathBuf>>), watchman::Error> {
        let fsmonitor = watchman::Fsmonitor::init(&self.working_copy_path).await?;
        let previous_clock = self.watchman_clock.clone().map(watchman::Clock::from);
        fsmonitor.query_changed_files(previous_clock).await
    }

    /// Look for changes to the working copy. If there are any changes, create
    /// a new tree from it and return it, and also update the dirstate on disk.
    pub fn snapshot(&mut self, options: SnapshotOptions) -> Result<bool, SnapshotError> {
        let SnapshotOptions {
            base_ignores,
            fsmonitor_kind,
            progress,
        } = options;

        let sparse_matcher = self.sparse_matcher();
        let current_tree = self.store.get_tree(&RepoPath::root(), &self.tree_id)?;
        let mut tree_builder = self.store.tree_builder(self.tree_id.clone());
        let mut deleted_files: HashSet<_> = self
            .file_states
            .iter()
            .filter_map(|(path, state)| {
                (state.file_type != FileType::GitSubmodule).then(|| path.clone())
            })
            .collect();

        let fsmonitor_clock_needs_save = fsmonitor_kind.is_some();
        let FsmonitorMatcher {
            matcher: fsmonitor_matcher,
            watchman_clock,
        } = self.make_fsmonitor_matcher(fsmonitor_kind, &mut deleted_files)?;

        let matcher = IntersectionMatcher::new(
            sparse_matcher.as_ref(),
            match fsmonitor_matcher.as_ref() {
                None => &EverythingMatcher,
                Some(fsmonitor_matcher) => fsmonitor_matcher.as_ref(),
            },
        );
        struct WorkItem {
            dir: RepoPath,
            disk_dir: PathBuf,
            git_ignore: Arc<GitIgnoreFile>,
        }
        let mut work = vec![WorkItem {
            dir: RepoPath::root(),
            disk_dir: self.working_copy_path.clone(),
            git_ignore: base_ignores,
        }];
        while let Some(WorkItem {
            dir,
            disk_dir,
            git_ignore,
        }) = work.pop()
        {
            if matcher.visit(&dir).is_nothing() {
                continue;
            }
            let git_ignore = git_ignore
                .chain_with_file(&dir.to_internal_dir_string(), disk_dir.join(".gitignore"));
            for maybe_entry in disk_dir.read_dir().unwrap() {
                let entry = maybe_entry.unwrap();
                let file_type = entry.file_type().unwrap();
                let file_name = entry.file_name();
                let name = file_name
                    .to_str()
                    .ok_or_else(|| SnapshotError::InvalidUtf8Path {
                        path: file_name.clone(),
                    })?;
                if name == ".jj" || name == ".git" {
                    continue;
                }
                let sub_path = dir.join(&RepoPathComponent::from(name));
                if let Some(file_state) = self.file_states.get(&sub_path) {
                    if file_state.file_type == FileType::GitSubmodule {
                        continue;
                    }
                }

                if file_type.is_dir() {
                    // If the whole directory is ignored, skip it unless we're already tracking
                    // some file in it.
                    if git_ignore.matches_all_files_in(&sub_path.to_internal_dir_string())
                        && current_tree.path_value(&sub_path).is_none()
                    {
                        continue;
                    }

                    work.push(WorkItem {
                        dir: sub_path,
                        disk_dir: entry.path(),
                        git_ignore: git_ignore.clone(),
                    });
                } else {
                    deleted_files.remove(&sub_path);
                    if matcher.matches(&sub_path) {
                        if let Some(progress) = progress {
                            progress(&sub_path);
                        }
                        let update = self.update_file_state(
                            sub_path.clone(),
                            &entry.path(),
                            git_ignore.as_ref(),
                            &current_tree,
                        )?;
                        match update {
                            Some((new_tree_value, new_file_state)) => {
                                self.file_states.insert(sub_path.clone(), new_file_state);
                                tree_builder.set(sub_path, new_tree_value);
                            }
                            None => {
                                self.file_states.remove(&sub_path);
                                tree_builder.remove(sub_path);
                            }
                        }
                    }
                }
            }
        }

        for file in &deleted_files {
            self.file_states.remove(file);
            tree_builder.remove(file.clone());
        }
        let has_changes = tree_builder.has_overrides();
        self.tree_id = tree_builder.write_tree();
        self.watchman_clock = watchman_clock;
        Ok(has_changes || fsmonitor_clock_needs_save)
    }

    fn make_fsmonitor_matcher(
        &mut self,
        fsmonitor_kind: Option<FsmonitorKind>,
        deleted_files: &mut HashSet<RepoPath>,
    ) -> Result<FsmonitorMatcher, SnapshotError> {
        let (watchman_clock, changed_files) = match fsmonitor_kind {
            None => (None, None),
            Some(FsmonitorKind::Test { changed_files }) => (None, Some(changed_files)),
            #[cfg(feature = "watchman")]
            Some(FsmonitorKind::Watchman) => match self.query_watchman() {
                Ok((watchman_clock, changed_files)) => (Some(watchman_clock.into()), changed_files),
                Err(err) => {
                    tracing::warn!(?err, "Failed to query filesystem monitor");
                    (None, None)
                }
            },
            #[cfg(not(feature = "watchman"))]
            Some(FsmonitorKind::Watchman) => {
                return Err(SnapshotError::FsmonitorError(
                    "Cannot query Watchman because jj was not compiled with the `watchman` \
                     feature (consider disabling `core.fsmonitor`)"
                        .to_string(),
                ));
            }
        };
        let matcher: Option<Box<dyn Matcher>> = match changed_files {
            None => None,
            Some(changed_files) => {
                let repo_paths = changed_files
                    .into_iter()
                    .filter_map(|path| {
                        match RepoPath::parse_fs_path(
                            &self.working_copy_path,
                            &self.working_copy_path,
                            path,
                        ) {
                            Ok(repo_path) => Some(repo_path),
                            Err(FsPathParseError::InputNotInRepo(_)) => None,
                        }
                    })
                    .collect_vec();

                let repo_path_set: HashSet<_> = repo_paths.iter().collect();
                deleted_files.retain(|path| repo_path_set.contains(path));

                Some(Box::new(PrefixMatcher::new(&repo_paths)))
            }
        };
        Ok(FsmonitorMatcher {
            matcher,
            watchman_clock,
        })
    }

    fn update_file_state(
        &self,
        repo_path: RepoPath,
        disk_path: &Path,
        git_ignore: &GitIgnoreFile,
        current_tree: &Tree,
    ) -> Result<Option<(TreeValue, FileState)>, SnapshotError> {
        let current_tree_value = current_tree.path_value(&repo_path);
        let maybe_current_file_state = self.file_states.get(&repo_path).cloned();
        if maybe_current_file_state.is_none()
            && git_ignore.matches_file(&repo_path.to_internal_file_string())
        {
            // If it wasn't already tracked and it matches the ignored paths, then
            // ignore it.
            return Ok(None);
        }

        let maybe_new_file_state = match std::fs::symlink_metadata(disk_path) {
            Ok(metadata) => file_state(&metadata),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(err) => {
                return Err(SnapshotError::IoError {
                    message: format!("Failed to stat file {}", disk_path.display()),
                    err,
                });
            }
        };
        let (mut current_file_state, mut new_file_state) =
            match (maybe_current_file_state, maybe_new_file_state) {
                (_, None) => {
                    return Ok(None);
                }
                (None, Some(new_file_state)) => {
                    // untracked
                    let file_type = new_file_state.file_type.clone();
                    let file_value = self.write_path_to_store(&repo_path, disk_path, file_type)?;
                    return Ok(Some((file_value, new_file_state)));
                }
                (Some(current_file_state), Some(new_file_state)) => {
                    (current_file_state, new_file_state)
                }
            };

        #[cfg(windows)]
        {
            // On Windows, we preserve the state we had recorded
            // when we wrote the file.
            new_file_state.mark_executable(current_file_state.is_executable());
        }
        // If the file's mtime was set at the same time as this state file's own mtime,
        // then we don't know if the file was modified before or after this state file.
        // We set the file's mtime to 0 to simplify later code.
        if current_file_state.mtime >= self.own_mtime {
            current_file_state.mtime = MillisSinceEpoch(0);
        }
        let mut clean = current_file_state == new_file_state;
        // Because the file system doesn't have a built-in way of indicating a conflict,
        // we look at the current state instead. If that indicates that the path has a
        // conflict and the contents are now a file, then we take interpret that as if
        // it is still a conflict.
        if !clean
            && current_file_state.file_type == FileType::Conflict
            && matches!(new_file_state.file_type, FileType::Normal { .. })
        {
            // If the only change is that the type changed from conflict to regular file,
            // then we consider it clean (the same as a regular file being clean, it's
            // just that the file system doesn't have a conflict type).
            if new_file_state.mtime == current_file_state.mtime
                && new_file_state.size == current_file_state.size
            {
                clean = true;
            } else {
                // If the file contained a conflict before and is now a normal file on disk
                // (new_file_state cannot be a Conflict at this point), we try to parse
                // any conflict markers in the file into a conflict.
                if let (
                    Some(TreeValue::Conflict(conflict_id)),
                    FileType::Normal { executable: _ },
                ) = (&current_tree_value, &new_file_state.file_type)
                {
                    let mut file = File::open(disk_path).unwrap();
                    let mut content = vec![];
                    file.read_to_end(&mut content).unwrap();
                    let conflict = self.store.read_conflict(&repo_path, conflict_id)?;
                    if let Some(new_conflict) = conflict
                        .update_from_content(self.store.as_ref(), &repo_path, &content)
                        .unwrap()
                    {
                        new_file_state.file_type = FileType::Conflict;
                        let new_conflict_id = if new_conflict == conflict {
                            conflict_id.clone()
                        } else {
                            self.store.write_conflict(&repo_path, &new_conflict)?
                        };
                        return Ok(Some((TreeValue::Conflict(new_conflict_id), new_file_state)));
                    }
                }
            }
        }
        if !clean {
            let file_type = new_file_state.file_type.clone();
            let file_value = self.write_path_to_store(&repo_path, disk_path, file_type)?;
            return Ok(Some((file_value, new_file_state)));
        }
        Ok(current_tree_value.map(|current_tree_value| (current_tree_value, new_file_state)))
    }

    fn write_path_to_store(
        &self,
        repo_path: &RepoPath,
        disk_path: &Path,
        file_type: FileType,
    ) -> Result<TreeValue, SnapshotError> {
        match file_type {
            FileType::Normal { executable } => {
                let id = self.write_file_to_store(repo_path, disk_path)?;
                Ok(TreeValue::File { id, executable })
            }
            FileType::Symlink => {
                let id = self.write_symlink_to_store(repo_path, disk_path)?;
                Ok(TreeValue::Symlink(id))
            }
            FileType::Conflict { .. } => panic!("conflicts should be handled by the caller"),
            FileType::GitSubmodule => panic!("git submodule cannot be written to store"),
        }
    }

    fn write_file(
        &self,
        disk_path: &Path,
        path: &RepoPath,
        id: &FileId,
        executable: bool,
    ) -> Result<FileState, CheckoutError> {
        create_parent_dirs(&self.working_copy_path, path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true) // Don't overwrite un-ignored file. Don't follow symlink.
            .open(disk_path)
            .map_err(|err| CheckoutError::IoError {
                message: format!("Failed to open file {} for writing", disk_path.display()),
                err,
            })?;
        let mut contents = self.store.read_file(path, id)?;
        let size =
            std::io::copy(&mut contents, &mut file).map_err(|err| CheckoutError::IoError {
                message: format!("Failed to write file {}", disk_path.display()),
                err,
            })?;
        self.set_executable(disk_path, executable)?;
        // Read the file state from the file descriptor. That way, know that the file
        // exists and is of the expected type, and the stat information is most likely
        // accurate, except for other processes modifying the file concurrently (The
        // mtime is set at write time and won't change when we close the file.)
        let metadata = file
            .metadata()
            .map_err(|err| CheckoutError::for_stat_error(err, disk_path))?;
        Ok(FileState::for_file(executable, size, &metadata))
    }

    #[cfg_attr(windows, allow(unused_variables))]
    fn write_symlink(
        &self,
        disk_path: &Path,
        path: &RepoPath,
        id: &SymlinkId,
    ) -> Result<FileState, CheckoutError> {
        create_parent_dirs(&self.working_copy_path, path)?;
        let target = self.store.read_symlink(path, id)?;
        #[cfg(windows)]
        {
            println!("ignoring symlink at {:?}", path);
        }
        #[cfg(unix)]
        {
            let target = PathBuf::from(&target);
            symlink(&target, disk_path).map_err(|err| CheckoutError::IoError {
                message: format!(
                    "Failed to create symlink from {} to {}",
                    disk_path.display(),
                    target.display()
                ),
                err,
            })?;
        }
        let metadata = disk_path
            .symlink_metadata()
            .map_err(|err| CheckoutError::for_stat_error(err, disk_path))?;
        Ok(FileState::for_symlink(&metadata))
    }

    fn write_conflict(
        &self,
        disk_path: &Path,
        path: &RepoPath,
        id: &ConflictId,
    ) -> Result<FileState, CheckoutError> {
        create_parent_dirs(&self.working_copy_path, path)?;
        let conflict = self.store.read_conflict(path, id)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true) // Don't overwrite un-ignored file. Don't follow symlink.
            .open(disk_path)
            .map_err(|err| CheckoutError::IoError {
                message: format!("Failed to open file {} for writing", disk_path.display()),
                err,
            })?;
        let mut conflict_data = vec![];
        conflict
            .materialize(self.store.as_ref(), path, &mut conflict_data)
            .expect("Failed to materialize conflict to in-memory buffer");
        file.write_all(&conflict_data)
            .map_err(|err| CheckoutError::IoError {
                message: format!("Failed to write conflict to file {}", disk_path.display()),
                err,
            })?;
        let size = conflict_data.len() as u64;
        // TODO: Set the executable bit correctly (when possible) and preserve that on
        // Windows like we do with the executable bit for regular files.
        let metadata = file
            .metadata()
            .map_err(|err| CheckoutError::for_stat_error(err, disk_path))?;
        Ok(FileState::for_conflict(size, &metadata))
    }

    #[cfg_attr(windows, allow(unused_variables))]
    fn set_executable(&self, disk_path: &Path, executable: bool) -> Result<(), CheckoutError> {
        #[cfg(unix)]
        {
            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(disk_path, fs::Permissions::from_mode(mode))
                .map_err(|err| CheckoutError::for_stat_error(err, disk_path))?;
        }
        Ok(())
    }

    pub fn check_out(&mut self, new_tree: &Tree) -> Result<CheckoutStats, CheckoutError> {
        let old_tree = self
            .store
            .get_tree(&RepoPath::root(), &self.tree_id)
            .map_err(|err| match err {
                err @ BackendError::ObjectNotFound { .. } => CheckoutError::SourceNotFound {
                    source: Box::new(err),
                },
                other => CheckoutError::InternalBackendError(other),
            })?;
        let stats = self.update(&old_tree, new_tree, self.sparse_matcher().as_ref(), Err)?;
        self.tree_id = new_tree.id().clone();
        Ok(stats)
    }

    pub fn set_sparse_patterns(
        &mut self,
        sparse_patterns: Vec<RepoPath>,
    ) -> Result<CheckoutStats, CheckoutError> {
        let tree = self
            .store
            .get_tree(&RepoPath::root(), &self.tree_id)
            .map_err(|err| match err {
                err @ BackendError::ObjectNotFound { .. } => CheckoutError::SourceNotFound {
                    source: Box::new(err),
                },
                other => CheckoutError::InternalBackendError(other),
            })?;
        let old_matcher = PrefixMatcher::new(&self.sparse_patterns);
        let new_matcher = PrefixMatcher::new(&sparse_patterns);
        let added_matcher = DifferenceMatcher::new(&new_matcher, &old_matcher);
        let removed_matcher = DifferenceMatcher::new(&old_matcher, &new_matcher);
        let empty_tree = Tree::null(self.store.clone(), RepoPath::root());
        let added_stats = self.update(
            &empty_tree,
            &tree,
            &added_matcher,
            suppress_file_exists_error, // Keep un-ignored file and mark it as modified
        )?;
        let removed_stats = self.update(&tree, &empty_tree, &removed_matcher, Err)?;
        self.sparse_patterns = sparse_patterns;
        assert_eq!(added_stats.updated_files, 0);
        assert_eq!(added_stats.removed_files, 0);
        assert_eq!(removed_stats.updated_files, 0);
        assert_eq!(removed_stats.added_files, 0);
        Ok(CheckoutStats {
            updated_files: 0,
            added_files: added_stats.added_files,
            removed_files: removed_stats.removed_files,
        })
    }

    fn update(
        &mut self,
        old_tree: &Tree,
        new_tree: &Tree,
        matcher: &dyn Matcher,
        mut handle_error: impl FnMut(CheckoutError) -> Result<(), CheckoutError>,
    ) -> Result<CheckoutStats, CheckoutError> {
        let mut stats = CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 0,
        };
        let mut apply_diff = |path: RepoPath, diff: Diff<TreeValue>| -> Result<(), CheckoutError> {
            let disk_path = path.to_fs_path(&self.working_copy_path);

            // TODO: Check that the file has not changed before overwriting/removing it.
            match diff {
                Diff::Removed(_before) => {
                    fs::remove_file(&disk_path).ok();
                    let mut parent_dir = disk_path.parent().unwrap();
                    loop {
                        if fs::remove_dir(parent_dir).is_err() {
                            break;
                        }
                        parent_dir = parent_dir.parent().unwrap();
                    }
                    self.file_states.remove(&path);
                    stats.removed_files += 1;
                }
                Diff::Added(after) => {
                    let file_state = match after {
                        TreeValue::File { id, executable } => {
                            self.write_file(&disk_path, &path, &id, executable)?
                        }
                        TreeValue::Symlink(id) => self.write_symlink(&disk_path, &path, &id)?,
                        TreeValue::Conflict(id) => self.write_conflict(&disk_path, &path, &id)?,
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {path:?}");
                            FileState::for_gitsubmodule()
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {path:?}");
                        }
                    };
                    self.file_states.insert(path, file_state);
                    stats.added_files += 1;
                }
                Diff::Modified(
                    TreeValue::File {
                        id: old_id,
                        executable: old_executable,
                    },
                    TreeValue::File { id, executable },
                ) if id == old_id => {
                    // Optimization for when only the executable bit changed
                    assert_ne!(executable, old_executable);
                    self.set_executable(&disk_path, executable)?;
                    let file_state = self.file_states.get_mut(&path).unwrap();
                    file_state.mark_executable(executable);
                    stats.updated_files += 1;
                }
                Diff::Modified(_before, after) => {
                    fs::remove_file(&disk_path).ok();
                    let file_state = match after {
                        TreeValue::File { id, executable } => {
                            self.write_file(&disk_path, &path, &id, executable)?
                        }
                        TreeValue::Symlink(id) => self.write_symlink(&disk_path, &path, &id)?,
                        TreeValue::Conflict(id) => self.write_conflict(&disk_path, &path, &id)?,
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {path:?}");
                            FileState::for_gitsubmodule()
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {path:?}");
                        }
                    };

                    self.file_states.insert(path, file_state);
                    stats.updated_files += 1;
                }
            }
            Ok(())
        };

        for (path, diff) in old_tree.diff(new_tree, matcher) {
            apply_diff(path, diff).or_else(&mut handle_error)?;
        }
        Ok(stats)
    }

    pub fn reset(&mut self, new_tree: &Tree) -> Result<(), ResetError> {
        let old_tree = self
            .store
            .get_tree(&RepoPath::root(), &self.tree_id)
            .map_err(|err| match err {
                err @ BackendError::ObjectNotFound { .. } => ResetError::SourceNotFound {
                    source: Box::new(err),
                },
                other => ResetError::InternalBackendError(other),
            })?;

        for (path, diff) in old_tree.diff(new_tree, self.sparse_matcher().as_ref()) {
            match diff {
                Diff::Removed(_before) => {
                    self.file_states.remove(&path);
                }
                Diff::Added(after) | Diff::Modified(_, after) => {
                    let file_type = match after {
                        TreeValue::File { id: _, executable } => FileType::Normal { executable },
                        TreeValue::Symlink(_id) => FileType::Symlink,
                        TreeValue::Conflict(_id) => FileType::Conflict,
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {path:?}");
                            FileType::GitSubmodule
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {path:?}");
                        }
                    };
                    let file_state = FileState {
                        file_type,
                        mtime: MillisSinceEpoch(0),
                        size: 0,
                    };
                    self.file_states.insert(path.clone(), file_state);
                }
            }
        }
        self.tree_id = new_tree.id().clone();
        Ok(())
    }
}

/// Working copy state stored in "checkout" file.
#[derive(Clone, Debug)]
struct CheckoutState {
    operation_id: OperationId,
    workspace_id: WorkspaceId,
}

pub struct WorkingCopy {
    store: Arc<Store>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    checkout_state: OnceCell<CheckoutState>,
    tree_state: OnceCell<TreeState>,
}

impl WorkingCopy {
    /// Initializes a new working copy at `working_copy_path`. The working
    /// copy's state will be stored in the `state_path` directory. The working
    /// copy will have the empty tree checked out.
    pub fn init(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        operation_id: OperationId,
        workspace_id: WorkspaceId,
    ) -> WorkingCopy {
        let proto = crate::protos::working_copy::Checkout {
            operation_id: operation_id.to_bytes(),
            workspace_id: workspace_id.as_str().to_string(),
            ..Default::default()
        };
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state_path.join("checkout"))
            .unwrap();
        file.write_all(&proto.encode_to_vec()).unwrap();
        let tree_state =
            TreeState::init(store.clone(), working_copy_path.clone(), state_path.clone());
        WorkingCopy {
            store,
            working_copy_path,
            state_path,
            checkout_state: OnceCell::new(),
            tree_state: OnceCell::with_value(tree_state),
        }
    }

    pub fn load(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> WorkingCopy {
        WorkingCopy {
            store,
            working_copy_path,
            state_path,
            checkout_state: OnceCell::new(),
            tree_state: OnceCell::new(),
        }
    }

    pub fn working_copy_path(&self) -> &Path {
        &self.working_copy_path
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    fn write_proto(&self, proto: crate::protos::working_copy::Checkout) {
        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        temp_file
            .as_file_mut()
            .write_all(&proto.encode_to_vec())
            .unwrap();
        // TODO: Retry if persisting fails (it will on Windows if the file happened to
        // be open for read).
        temp_file.persist(self.state_path.join("checkout")).unwrap();
    }

    fn checkout_state(&self) -> &CheckoutState {
        self.checkout_state.get_or_init(|| {
            let buf = fs::read(self.state_path.join("checkout")).unwrap();
            let proto = crate::protos::working_copy::Checkout::decode(&*buf).unwrap();
            CheckoutState {
                operation_id: OperationId::new(proto.operation_id),
                workspace_id: if proto.workspace_id.is_empty() {
                    // For compatibility with old working copies.
                    // TODO: Delete in mid 2022 or so
                    WorkspaceId::default()
                } else {
                    WorkspaceId::new(proto.workspace_id)
                },
            }
        })
    }

    fn checkout_state_mut(&mut self) -> &mut CheckoutState {
        self.checkout_state(); // ensure loaded
        self.checkout_state.get_mut().unwrap()
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.checkout_state().operation_id
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.checkout_state().workspace_id
    }

    fn tree_state(&self) -> &TreeState {
        self.tree_state.get_or_init(|| {
            TreeState::load(
                self.store.clone(),
                self.working_copy_path.clone(),
                self.state_path.clone(),
            )
        })
    }

    fn tree_state_mut(&mut self) -> &mut TreeState {
        self.tree_state(); // ensure loaded
        self.tree_state.get_mut().unwrap()
    }

    pub fn current_tree_id(&self) -> &TreeId {
        self.tree_state().current_tree_id()
    }

    pub fn file_states(&self) -> &BTreeMap<RepoPath, FileState> {
        self.tree_state().file_states()
    }

    pub fn sparse_patterns(&self) -> &[RepoPath] {
        self.tree_state().sparse_patterns()
    }

    fn save(&mut self) {
        self.write_proto(crate::protos::working_copy::Checkout {
            operation_id: self.operation_id().to_bytes(),
            workspace_id: self.workspace_id().as_str().to_string(),
            ..Default::default()
        });
    }

    pub fn start_mutation(&mut self) -> LockedWorkingCopy {
        let lock_path = self.state_path.join("working_copy.lock");
        let lock = FileLock::lock(lock_path);

        // Re-read from disk after taking the lock
        self.checkout_state.take();
        // TODO: It's expensive to reload the whole tree. We should first check if it
        // has changed.
        self.tree_state.take();
        let old_operation_id = self.operation_id().clone();
        let old_tree_id = self.current_tree_id().clone();

        LockedWorkingCopy {
            wc: self,
            lock,
            old_operation_id,
            old_tree_id,
            tree_state_dirty: false,
            closed: false,
        }
    }

    pub fn check_out(
        &mut self,
        operation_id: OperationId,
        old_tree_id: Option<&TreeId>,
        new_tree: &Tree,
    ) -> Result<CheckoutStats, CheckoutError> {
        let mut locked_wc = self.start_mutation();
        // Check if the current working-copy commit has changed on disk compared to what
        // the caller expected. It's safe to check out another commit
        // regardless, but it's probably not what  the caller wanted, so we let
        // them know.
        if let Some(old_tree_id) = old_tree_id {
            if *old_tree_id != locked_wc.old_tree_id {
                locked_wc.discard();
                return Err(CheckoutError::ConcurrentCheckout);
            }
        }
        let stats = locked_wc.check_out(new_tree)?;
        locked_wc.finish(operation_id);
        Ok(stats)
    }

    #[cfg(feature = "watchman")]
    pub fn query_watchman(
        &self,
    ) -> Result<(watchman::Clock, Option<Vec<PathBuf>>), watchman::Error> {
        self.tree_state().query_watchman()
    }
}

/// A working copy that's locked on disk. The lock is held until you call
/// `finish()` or `discard()`.
pub struct LockedWorkingCopy<'a> {
    wc: &'a mut WorkingCopy,
    #[allow(dead_code)]
    lock: FileLock,
    old_operation_id: OperationId,
    old_tree_id: TreeId,
    tree_state_dirty: bool,
    closed: bool,
}

impl LockedWorkingCopy<'_> {
    /// The operation at the time the lock was taken
    pub fn old_operation_id(&self) -> &OperationId {
        &self.old_operation_id
    }

    /// The tree at the time the lock was taken
    pub fn old_tree_id(&self) -> &TreeId {
        &self.old_tree_id
    }

    pub fn reset_watchman(&mut self) -> Result<(), SnapshotError> {
        self.wc.tree_state_mut().reset_watchman();
        self.tree_state_dirty = true;
        Ok(())
    }

    // The base_ignores are passed in here rather than being set on the TreeState
    // because the TreeState may be long-lived if the library is used in a
    // long-lived process.
    pub fn snapshot(&mut self, options: SnapshotOptions) -> Result<TreeId, SnapshotError> {
        let tree_state = self.wc.tree_state_mut();
        self.tree_state_dirty |= tree_state.snapshot(options)?;
        Ok(tree_state.current_tree_id().clone())
    }

    pub fn check_out(&mut self, new_tree: &Tree) -> Result<CheckoutStats, CheckoutError> {
        // TODO: Write a "pending_checkout" file with the new TreeId so we can
        // continue an interrupted update if we find such a file.
        let stats = self.wc.tree_state_mut().check_out(new_tree)?;
        self.tree_state_dirty = true;
        Ok(stats)
    }

    pub fn reset(&mut self, new_tree: &Tree) -> Result<(), ResetError> {
        self.wc.tree_state_mut().reset(new_tree)?;
        self.tree_state_dirty = true;
        Ok(())
    }

    pub fn sparse_patterns(&self) -> &[RepoPath] {
        self.wc.sparse_patterns()
    }

    pub fn set_sparse_patterns(
        &mut self,
        new_sparse_patterns: Vec<RepoPath>,
    ) -> Result<CheckoutStats, CheckoutError> {
        // TODO: Write a "pending_checkout" file with new sparse patterns so we can
        // continue an interrupted update if we find such a file.
        let stats = self
            .wc
            .tree_state_mut()
            .set_sparse_patterns(new_sparse_patterns)?;
        self.tree_state_dirty = true;
        Ok(stats)
    }

    pub fn finish(mut self, operation_id: OperationId) {
        assert!(self.tree_state_dirty || &self.old_tree_id == self.wc.current_tree_id());
        if self.tree_state_dirty {
            self.wc.tree_state_mut().save();
        }
        if self.old_operation_id != operation_id {
            self.wc.checkout_state_mut().operation_id = operation_id;
            self.wc.save();
        }
        // TODO: Clear the "pending_checkout" file here.
        self.tree_state_dirty = false;
        self.closed = true;
    }

    pub fn discard(mut self) {
        // Undo the changes in memory
        self.wc.tree_state.take();
        self.tree_state_dirty = false;
        self.closed = true;
    }
}

impl Drop for LockedWorkingCopy<'_> {
    fn drop(&mut self) {
        if !self.closed && !std::thread::panicking() {
            eprintln!("BUG: Working copy lock was dropped without being closed.");
        }
    }
}

pub type SnapshotProgress<'a> = dyn Fn(&RepoPath) + 'a;
