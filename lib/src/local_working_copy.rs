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
use std::error::Error;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::ops::Range;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use std::{fs, iter, mem, slice};

use futures::StreamExt;
use itertools::{EitherOrBoth, Itertools};
use once_cell::unsync::OnceCell;
use pollster::FutureExt;
use prost::Message;
use rayon::iter::IntoParallelIterator;
use rayon::prelude::ParallelIterator;
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::{instrument, trace_span};

use crate::backend::{
    BackendError, FileId, MergedTreeId, MillisSinceEpoch, SymlinkId, TreeId, TreeValue,
};
use crate::commit::Commit;
use crate::conflicts::{self, materialize_tree_value, MaterializedTreeValue};
use crate::file_util::{check_symlink_support, try_symlink};
#[cfg(feature = "watchman")]
use crate::fsmonitor::watchman;
use crate::fsmonitor::FsmonitorKind;
use crate::gitignore::GitIgnoreFile;
use crate::lock::FileLock;
use crate::matchers::{
    DifferenceMatcher, EverythingMatcher, FilesMatcher, IntersectionMatcher, Matcher, PrefixMatcher,
};
use crate::merge::{Merge, MergeBuilder, MergedTreeValue};
use crate::merged_tree::{MergedTree, MergedTreeBuilder};
use crate::object_id::ObjectId;
use crate::op_store::{OperationId, WorkspaceId};
use crate::repo_path::{RepoPath, RepoPathBuf, RepoPathComponent};
use crate::settings::HumanByteSize;
use crate::store::Store;
use crate::tree::Tree;
use crate::working_copy::{
    CheckoutError, CheckoutStats, LockedWorkingCopy, ResetError, SnapshotError, SnapshotOptions,
    SnapshotProgress, WorkingCopy, WorkingCopyFactory, WorkingCopyStateError,
};

#[cfg(unix)]
type FileExecutableFlag = bool;
#[cfg(windows)]
type FileExecutableFlag = ();

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FileType {
    Normal { executable: FileExecutableFlag },
    Symlink,
    GitSubmodule,
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
    /// Indicates that a file exists in the tree but that it needs to be
    /// re-stat'ed on the next snapshot.
    fn placeholder() -> Self {
        #[cfg(unix)]
        let executable = false;
        #[cfg(windows)]
        let executable = ();
        FileState {
            file_type: FileType::Normal { executable },
            mtime: MillisSinceEpoch(0),
            size: 0,
        }
    }

    fn for_file(executable: bool, size: u64, metadata: &Metadata) -> Self {
        #[cfg(windows)]
        let executable = {
            // Windows doesn't support executable bit.
            let _ = executable;
            ()
        };
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

    fn for_gitsubmodule() -> Self {
        FileState {
            file_type: FileType::GitSubmodule,
            mtime: MillisSinceEpoch(0),
            size: 0,
        }
    }
}

/// Owned map of path to file states, backed by proto data.
#[derive(Clone, Debug)]
struct FileStatesMap {
    data: Vec<crate::protos::working_copy::FileStateEntry>,
}

impl FileStatesMap {
    fn new() -> Self {
        FileStatesMap { data: Vec::new() }
    }

    // TODO: skip sorting if the data is known to be sorted?
    fn from_proto_unsorted(mut data: Vec<crate::protos::working_copy::FileStateEntry>) -> Self {
        data.sort_unstable_by(|entry1, entry2| {
            let path1 = RepoPath::from_internal_string(&entry1.path);
            let path2 = RepoPath::from_internal_string(&entry2.path);
            path1.cmp(path2)
        });
        debug_assert!(is_file_state_entries_proto_unique_and_sorted(&data));
        FileStatesMap { data }
    }

    /// Merges changed and deleted entries into this map. The changed entries
    /// must be sorted by path.
    fn merge_in(
        &mut self,
        changed_file_states: Vec<(RepoPathBuf, FileState)>,
        deleted_files: &HashSet<RepoPathBuf>,
    ) {
        if changed_file_states.is_empty() && deleted_files.is_empty() {
            return;
        }
        debug_assert!(
            changed_file_states
                .iter()
                .tuple_windows()
                .all(|((path1, _), (path2, _))| path1 < path2),
            "changed_file_states must be sorted and have no duplicates"
        );
        self.data = itertools::merge_join_by(
            mem::take(&mut self.data),
            changed_file_states,
            |old_entry, (changed_path, _)| {
                RepoPath::from_internal_string(&old_entry.path).cmp(changed_path)
            },
        )
        .filter_map(|diff| match diff {
            EitherOrBoth::Both(_, (path, state)) | EitherOrBoth::Right((path, state)) => {
                debug_assert!(!deleted_files.contains(&path));
                Some(file_state_entry_to_proto(path, &state))
            }
            EitherOrBoth::Left(entry) => {
                let present = !deleted_files.contains(RepoPath::from_internal_string(&entry.path));
                present.then_some(entry)
            }
        })
        .collect();
    }

    fn clear(&mut self) {
        self.data.clear();
    }

    /// Returns read-only map containing all file states.
    fn all(&self) -> FileStates<'_> {
        FileStates::from_sorted(&self.data)
    }
}

/// Read-only map of path to file states, possibly filtered by path prefix.
#[derive(Clone, Copy, Debug)]
pub struct FileStates<'a> {
    data: &'a [crate::protos::working_copy::FileStateEntry],
}

impl<'a> FileStates<'a> {
    fn from_sorted(data: &'a [crate::protos::working_copy::FileStateEntry]) -> Self {
        debug_assert!(is_file_state_entries_proto_unique_and_sorted(data));
        FileStates { data }
    }

    /// Returns file states under the given directory path.
    pub fn prefixed(&self, base: &RepoPath) -> Self {
        let range = self.prefixed_range(base);
        Self::from_sorted(&self.data[range])
    }

    /// Returns true if this contains no entries.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns true if the given `path` exists.
    pub fn contains_path(&self, path: &RepoPath) -> bool {
        self.exact_position(path).is_some()
    }

    /// Returns file state for the given `path`.
    pub fn get(&self, path: &RepoPath) -> Option<FileState> {
        let pos = self.exact_position(path)?;
        let (_, state) = file_state_entry_from_proto(&self.data[pos]);
        Some(state)
    }

    fn exact_position(&self, path: &RepoPath) -> Option<usize> {
        self.data
            .binary_search_by(|entry| RepoPath::from_internal_string(&entry.path).cmp(path))
            .ok()
    }

    fn prefixed_range(&self, base: &RepoPath) -> Range<usize> {
        let start = self
            .data
            .partition_point(|entry| RepoPath::from_internal_string(&entry.path) < base);
        let len = self.data[start..]
            .partition_point(|entry| RepoPath::from_internal_string(&entry.path).starts_with(base));
        start..(start + len)
    }

    /// Iterates file state entries sorted by path.
    pub fn iter(&self) -> FileStatesIter<'a> {
        self.data.iter().map(file_state_entry_from_proto)
    }

    /// Iterates sorted file paths.
    pub fn paths(&self) -> impl ExactSizeIterator<Item = &'a RepoPath> {
        self.data
            .iter()
            .map(|entry| RepoPath::from_internal_string(&entry.path))
    }
}

type FileStatesIter<'a> = iter::Map<
    slice::Iter<'a, crate::protos::working_copy::FileStateEntry>,
    fn(&crate::protos::working_copy::FileStateEntry) -> (&RepoPath, FileState),
>;

impl<'a> IntoIterator for FileStates<'a> {
    type Item = (&'a RepoPath, FileState);
    type IntoIter = FileStatesIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct TreeState {
    store: Arc<Store>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    tree_id: MergedTreeId,
    file_states: FileStatesMap,
    // Currently only path prefixes
    sparse_patterns: Vec<RepoPathBuf>,
    own_mtime: MillisSinceEpoch,
    symlink_support: bool,

    /// The most recent clock value returned by Watchman. Will only be set if
    /// the repo is configured to use the Watchman filesystem monitor and
    /// Watchman has been queried at least once.
    watchman_clock: Option<crate::protos::working_copy::WatchmanClock>,
}

fn file_state_from_proto(proto: &crate::protos::working_copy::FileState) -> FileState {
    let file_type = match proto.file_type() {
        crate::protos::working_copy::FileType::Normal => FileType::Normal {
            executable: FileExecutableFlag::default(),
        },
        #[cfg(unix)]
        crate::protos::working_copy::FileType::Executable => FileType::Normal { executable: true },
        // can exist in files written by older versions of jj
        #[cfg(windows)]
        crate::protos::working_copy::FileType::Executable => FileType::Normal { executable: () },
        crate::protos::working_copy::FileType::Symlink => FileType::Symlink,
        crate::protos::working_copy::FileType::Conflict => FileType::Normal {
            executable: FileExecutableFlag::default(),
        },
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
        #[cfg(unix)]
        FileType::Normal { executable: false } => crate::protos::working_copy::FileType::Normal,
        #[cfg(unix)]
        FileType::Normal { executable: true } => crate::protos::working_copy::FileType::Executable,
        #[cfg(windows)]
        FileType::Normal { executable: () } => crate::protos::working_copy::FileType::Normal,
        FileType::Symlink => crate::protos::working_copy::FileType::Symlink,
        FileType::GitSubmodule => crate::protos::working_copy::FileType::GitSubmodule,
    };
    proto.file_type = file_type as i32;
    proto.mtime_millis_since_epoch = file_state.mtime.0;
    proto.size = file_state.size;
    proto
}

fn file_state_entry_from_proto(
    proto: &crate::protos::working_copy::FileStateEntry,
) -> (&RepoPath, FileState) {
    let path = RepoPath::from_internal_string(&proto.path);
    (path, file_state_from_proto(proto.state.as_ref().unwrap()))
}

fn file_state_entry_to_proto(
    path: RepoPathBuf,
    state: &FileState,
) -> crate::protos::working_copy::FileStateEntry {
    crate::protos::working_copy::FileStateEntry {
        path: path.into_internal_string(),
        state: Some(file_state_to_proto(state)),
    }
}

fn is_file_state_entries_proto_unique_and_sorted(
    data: &[crate::protos::working_copy::FileStateEntry],
) -> bool {
    data.iter()
        .map(|entry| RepoPath::from_internal_string(&entry.path))
        .tuple_windows()
        .all(|(path1, path2)| path1 < path2)
}

fn sparse_patterns_from_proto(
    proto: Option<&crate::protos::working_copy::SparsePatterns>,
) -> Vec<RepoPathBuf> {
    let mut sparse_patterns = vec![];
    if let Some(proto_sparse_patterns) = proto {
        for prefix in &proto_sparse_patterns.prefixes {
            sparse_patterns.push(RepoPathBuf::from_internal_string(prefix));
        }
    } else {
        // For compatibility with old working copies.
        // TODO: Delete this is late 2022 or so.
        sparse_patterns.push(RepoPathBuf::root());
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
fn create_parent_dirs(
    working_copy_path: &Path,
    repo_path: &RepoPath,
) -> Result<bool, CheckoutError> {
    let parent_path = repo_path.parent().expect("repo path shouldn't be root");
    let mut dir_path = working_copy_path.to_owned();
    for c in parent_path.components() {
        dir_path.push(c.as_str());
        match fs::create_dir(&dir_path) {
            Ok(()) => {}
            Err(_)
                if dir_path
                    .symlink_metadata()
                    .map(|m| m.is_dir())
                    .unwrap_or(false) => {}
            Err(err) => {
                if dir_path.is_file() {
                    return Ok(true);
                }
                return Err(CheckoutError::Other {
                    message: format!(
                        "Failed to create parent directories for {}",
                        repo_path.to_fs_path(working_copy_path).display(),
                    ),
                    err: err.into(),
                });
            }
        }
    }
    Ok(false)
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
        if metadata.permissions().mode() & 0o111 != 0 {
            Some(FileType::Normal { executable: true })
        } else {
            Some(FileType::Normal { executable: false })
        }
        #[cfg(windows)]
        Some(FileType::Normal { executable: () })
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

struct FsmonitorMatcher {
    matcher: Option<Box<dyn Matcher>>,
    watchman_clock: Option<crate::protos::working_copy::WatchmanClock>,
}

struct DirectoryToVisit<'a> {
    dir: RepoPathBuf,
    disk_dir: PathBuf,
    git_ignore: Arc<GitIgnoreFile>,
    file_states: FileStates<'a>,
}

#[derive(Debug, Error)]
pub enum TreeStateError {
    #[error("Reading tree state from {path}")]
    ReadTreeState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Decoding tree state from {path}")]
    DecodeTreeState {
        path: PathBuf,
        source: prost::DecodeError,
    },
    #[error("Writing tree state to temporary file {path}")]
    WriteTreeState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Persisting tree state to file {path}")]
    PersistTreeState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Filesystem monitor error")]
    Fsmonitor(#[source] Box<dyn Error + Send + Sync>),
}

impl TreeState {
    pub fn working_copy_path(&self) -> &Path {
        &self.working_copy_path
    }

    pub fn current_tree_id(&self) -> &MergedTreeId {
        &self.tree_id
    }

    pub fn file_states(&self) -> FileStates<'_> {
        self.file_states.all()
    }

    pub fn sparse_patterns(&self) -> &Vec<RepoPathBuf> {
        &self.sparse_patterns
    }

    fn sparse_matcher(&self) -> Box<dyn Matcher> {
        Box::new(PrefixMatcher::new(&self.sparse_patterns))
    }

    pub fn init(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> Result<TreeState, TreeStateError> {
        let mut wc = TreeState::empty(store, working_copy_path, state_path);
        wc.save()?;
        Ok(wc)
    }

    fn empty(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> TreeState {
        let tree_id = store.empty_merged_tree_id();
        // Canonicalize the working copy path because "repo/." makes libgit2 think that
        // everything should be ignored
        TreeState {
            store,
            working_copy_path: working_copy_path.canonicalize().unwrap(),
            state_path,
            tree_id,
            file_states: FileStatesMap::new(),
            sparse_patterns: vec![RepoPathBuf::root()],
            own_mtime: MillisSinceEpoch(0),
            symlink_support: check_symlink_support().unwrap_or(false),
            watchman_clock: None,
        }
    }

    pub fn load(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> Result<TreeState, TreeStateError> {
        let tree_state_path = state_path.join("tree_state");
        let file = match File::open(&tree_state_path) {
            Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
                return TreeState::init(store, working_copy_path, state_path);
            }
            Err(err) => {
                return Err(TreeStateError::ReadTreeState {
                    path: tree_state_path,
                    source: err,
                })
            }
            Ok(file) => file,
        };

        let mut wc = TreeState::empty(store, working_copy_path, state_path);
        wc.read(&tree_state_path, file)?;
        Ok(wc)
    }

    fn update_own_mtime(&mut self) {
        if let Ok(metadata) = self.state_path.join("tree_state").symlink_metadata() {
            self.own_mtime = mtime_from_metadata(&metadata);
        } else {
            self.own_mtime = MillisSinceEpoch(0);
        }
    }

    fn read(&mut self, tree_state_path: &Path, mut file: File) -> Result<(), TreeStateError> {
        self.update_own_mtime();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|err| TreeStateError::ReadTreeState {
                path: tree_state_path.to_owned(),
                source: err,
            })?;
        let proto = crate::protos::working_copy::TreeState::decode(&*buf).map_err(|err| {
            TreeStateError::DecodeTreeState {
                path: tree_state_path.to_owned(),
                source: err,
            }
        })?;
        if proto.tree_ids.is_empty() {
            self.tree_id = MergedTreeId::Legacy(TreeId::new(proto.legacy_tree_id.clone()));
        } else {
            let tree_ids_builder: MergeBuilder<TreeId> = proto
                .tree_ids
                .iter()
                .map(|id| TreeId::new(id.clone()))
                .collect();
            self.tree_id = MergedTreeId::Merge(tree_ids_builder.build());
        }
        self.file_states = FileStatesMap::from_proto_unsorted(proto.file_states);
        self.sparse_patterns = sparse_patterns_from_proto(proto.sparse_patterns.as_ref());
        self.watchman_clock = proto.watchman_clock;
        Ok(())
    }

    fn save(&mut self) -> Result<(), TreeStateError> {
        let mut proto: crate::protos::working_copy::TreeState = Default::default();
        match &self.tree_id {
            MergedTreeId::Legacy(tree_id) => {
                proto.legacy_tree_id = tree_id.to_bytes();
            }
            MergedTreeId::Merge(tree_ids) => {
                proto.tree_ids = tree_ids.iter().map(|id| id.to_bytes()).collect();
            }
        }

        proto.file_states = self.file_states.data.clone();
        let mut sparse_patterns = crate::protos::working_copy::SparsePatterns::default();
        for path in &self.sparse_patterns {
            sparse_patterns
                .prefixes
                .push(path.as_internal_file_string().to_owned());
        }
        proto.sparse_patterns = Some(sparse_patterns);
        proto.watchman_clock = self.watchman_clock.clone();

        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        temp_file
            .as_file_mut()
            .write_all(&proto.encode_to_vec())
            .map_err(|err| TreeStateError::WriteTreeState {
                path: self.state_path.clone(),
                source: err,
            })?;
        // update own write time while we before we rename it, so we know
        // there is no unknown data in it
        self.update_own_mtime();
        // TODO: Retry if persisting fails (it will on Windows if the file happened to
        // be open for read).
        let target_path = self.state_path.join("tree_state");
        temp_file
            .persist(&target_path)
            .map_err(|tempfile::PersistError { error, file: _ }| {
                TreeStateError::PersistTreeState {
                    path: target_path.clone(),
                    source: error,
                }
            })?;
        Ok(())
    }

    fn current_tree(&self) -> Result<MergedTree, BackendError> {
        self.store.get_root_tree(&self.tree_id)
    }

    fn write_file_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<FileId, SnapshotError> {
        let mut file = File::open(disk_path).map_err(|err| SnapshotError::Other {
            message: format!("Failed to open file {}", disk_path.display()),
            err: err.into(),
        })?;
        Ok(self.store.write_file(path, &mut file)?)
    }

    fn write_symlink_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<SymlinkId, SnapshotError> {
        if self.symlink_support {
            let target = disk_path.read_link().map_err(|err| SnapshotError::Other {
                message: format!("Failed to read symlink {}", disk_path.display()),
                err: err.into(),
            })?;
            let str_target =
                target
                    .to_str()
                    .ok_or_else(|| SnapshotError::InvalidUtf8SymlinkTarget {
                        path: disk_path.to_path_buf(),
                    })?;
            Ok(self.store.write_symlink(path, str_target)?)
        } else {
            let target = fs::read(disk_path).map_err(|err| SnapshotError::Other {
                message: format!("Failed to read file {}", disk_path.display()),
                err: err.into(),
            })?;
            let string_target =
                String::from_utf8(target).map_err(|_| SnapshotError::InvalidUtf8SymlinkTarget {
                    path: disk_path.to_path_buf(),
                })?;
            Ok(self.store.write_symlink(path, &string_target)?)
        }
    }

    fn reset_watchman(&mut self) {
        self.watchman_clock.take();
    }

    #[cfg(feature = "watchman")]
    #[tokio::main(flavor = "current_thread")]
    #[instrument(skip(self))]
    pub async fn query_watchman(
        &self,
    ) -> Result<(watchman::Clock, Option<Vec<PathBuf>>), TreeStateError> {
        let fsmonitor = watchman::Fsmonitor::init(&self.working_copy_path)
            .await
            .map_err(|err| TreeStateError::Fsmonitor(Box::new(err)))?;
        let previous_clock = self.watchman_clock.clone().map(watchman::Clock::from);
        let changed_files = fsmonitor
            .query_changed_files(previous_clock)
            .await
            .map_err(|err| TreeStateError::Fsmonitor(Box::new(err)))?;
        Ok(changed_files)
    }

    /// Look for changes to the working copy. If there are any changes, create
    /// a new tree from it and return it, and also update the dirstate on disk.
    #[instrument(skip_all)]
    pub fn snapshot(&mut self, options: SnapshotOptions) -> Result<bool, SnapshotError> {
        let SnapshotOptions {
            base_ignores,
            fsmonitor_kind,
            progress,
            max_new_file_size,
        } = options;

        let sparse_matcher = self.sparse_matcher();

        let fsmonitor_clock_needs_save = fsmonitor_kind != FsmonitorKind::None;
        let mut is_dirty = fsmonitor_clock_needs_save;
        let FsmonitorMatcher {
            matcher: fsmonitor_matcher,
            watchman_clock,
        } = self.make_fsmonitor_matcher(fsmonitor_kind)?;
        let fsmonitor_matcher = match fsmonitor_matcher.as_ref() {
            None => &EverythingMatcher,
            Some(fsmonitor_matcher) => fsmonitor_matcher.as_ref(),
        };

        let matcher = IntersectionMatcher::new(sparse_matcher.as_ref(), fsmonitor_matcher);
        if matcher.visit(RepoPath::root()).is_nothing() {
            // No need to iterate file states to build empty deleted_files.
            self.watchman_clock = watchman_clock;
            return Ok(is_dirty);
        }

        let (tree_entries_tx, tree_entries_rx) = channel();
        let (file_states_tx, file_states_rx) = channel();
        let (present_files_tx, present_files_rx) = channel();

        trace_span!("traverse filesystem").in_scope(|| -> Result<(), SnapshotError> {
            let current_tree = self.current_tree()?;
            let directory_to_visit = DirectoryToVisit {
                dir: RepoPathBuf::root(),
                disk_dir: self.working_copy_path.clone(),
                git_ignore: base_ignores,
                file_states: self.file_states.all(),
            };
            self.visit_directory(
                &matcher,
                &current_tree,
                tree_entries_tx,
                file_states_tx,
                present_files_tx,
                directory_to_visit,
                progress,
                max_new_file_size,
            )
        })?;

        let mut tree_builder = MergedTreeBuilder::new(self.tree_id.clone());
        let mut deleted_files: HashSet<_> =
            trace_span!("collecting existing files").in_scope(|| {
                // Since file_states shouldn't contain files excluded by the sparse patterns,
                // fsmonitor_matcher here is identical to the intersected matcher.
                let file_states = self.file_states.all();
                file_states
                    .iter()
                    .filter(|(path, state)| {
                        fsmonitor_matcher.matches(path) && state.file_type != FileType::GitSubmodule
                    })
                    .map(|(path, _state)| path.to_owned())
                    .collect()
            });
        trace_span!("process tree entries").in_scope(|| -> Result<(), SnapshotError> {
            while let Ok((path, tree_values)) = tree_entries_rx.recv() {
                tree_builder.set_or_remove(path, tree_values);
            }
            Ok(())
        })?;
        trace_span!("process present files").in_scope(|| {
            while let Ok(path) = present_files_rx.recv() {
                deleted_files.remove(&path);
            }
        });
        trace_span!("process deleted tree entries").in_scope(|| {
            is_dirty |= !deleted_files.is_empty();
            for file in &deleted_files {
                tree_builder.set_or_remove(file.clone(), Merge::absent());
            }
        });
        trace_span!("process file states").in_scope(|| {
            let changed_file_states = file_states_rx
                .iter()
                .sorted_unstable_by(|(path1, _), (path2, _)| path1.cmp(path2))
                .collect_vec();
            is_dirty |= !changed_file_states.is_empty();
            self.file_states
                .merge_in(changed_file_states, &deleted_files);
        });
        trace_span!("write tree").in_scope(|| {
            let new_tree_id = tree_builder.write_tree(&self.store).unwrap();
            is_dirty |= new_tree_id != self.tree_id;
            self.tree_id = new_tree_id;
        });
        if cfg!(debug_assertions) {
            let tree = self.current_tree().unwrap();
            let tree_paths: HashSet<_> = tree
                .entries_matching(sparse_matcher.as_ref())
                .map(|(path, _)| path)
                .collect();
            let file_states = self.file_states.all();
            let state_paths: HashSet<_> = file_states.paths().map(|path| path.to_owned()).collect();
            assert_eq!(state_paths, tree_paths);
        }
        self.watchman_clock = watchman_clock;
        Ok(is_dirty)
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_directory(
        &self,
        matcher: &dyn Matcher,
        current_tree: &MergedTree,
        tree_entries_tx: Sender<(RepoPathBuf, MergedTreeValue)>,
        file_states_tx: Sender<(RepoPathBuf, FileState)>,
        present_files_tx: Sender<RepoPathBuf>,
        directory_to_visit: DirectoryToVisit,
        progress: Option<&SnapshotProgress>,
        max_new_file_size: u64,
    ) -> Result<(), SnapshotError> {
        let DirectoryToVisit {
            dir,
            disk_dir,
            git_ignore,
            file_states,
        } = directory_to_visit;

        if matcher.visit(&dir).is_nothing() {
            return Ok(());
        }

        let git_ignore = git_ignore
            .chain_with_file(&dir.to_internal_dir_string(), disk_dir.join(".gitignore"))?;
        let dir_entries = disk_dir
            .read_dir()
            .unwrap()
            .map(|maybe_entry| maybe_entry.unwrap())
            .collect_vec();
        dir_entries.into_par_iter().try_for_each_with(
            (
                tree_entries_tx.clone(),
                file_states_tx.clone(),
                present_files_tx.clone(),
            ),
            |(tree_entries_tx, file_states_tx, present_files_tx),
             entry|
             -> Result<(), SnapshotError> {
                let file_type = entry.file_type().unwrap();
                let file_name = entry.file_name();
                let name = file_name
                    .to_str()
                    .ok_or_else(|| SnapshotError::InvalidUtf8Path {
                        path: file_name.clone(),
                    })?;

                if name == ".jj" || name == ".git" {
                    return Ok(());
                }
                let path = dir.join(RepoPathComponent::new(name));
                let maybe_current_file_state = file_states.get(&path);
                if let Some(file_state) = &maybe_current_file_state {
                    if file_state.file_type == FileType::GitSubmodule {
                        return Ok(());
                    }
                }

                if file_type.is_dir() {
                    let file_states = file_states.prefixed(&path);
                    if git_ignore.matches(&path.to_internal_dir_string()) {
                        // If the whole directory is ignored, visit only paths we're already
                        // tracking.
                        for (tracked_path, current_file_state) in file_states {
                            if !matcher.matches(tracked_path) {
                                continue;
                            }
                            let disk_path = tracked_path.to_fs_path(&self.working_copy_path);
                            let metadata = match disk_path.symlink_metadata() {
                                Ok(metadata) => metadata,
                                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                                    continue;
                                }
                                Err(err) => {
                                    return Err(SnapshotError::Other {
                                        message: format!(
                                            "Failed to stat file {}",
                                            disk_path.display()
                                        ),
                                        err: err.into(),
                                    });
                                }
                            };
                            if let Some(new_file_state) = file_state(&metadata) {
                                present_files_tx.send(tracked_path.to_owned()).ok();
                                let update = self.get_updated_tree_value(
                                    tracked_path,
                                    disk_path,
                                    Some(&current_file_state),
                                    current_tree,
                                    &new_file_state,
                                )?;
                                if let Some(tree_value) = update {
                                    tree_entries_tx
                                        .send((tracked_path.to_owned(), tree_value))
                                        .ok();
                                }
                                if new_file_state != current_file_state {
                                    file_states_tx
                                        .send((tracked_path.to_owned(), new_file_state))
                                        .ok();
                                }
                            }
                        }
                    } else {
                        let directory_to_visit = DirectoryToVisit {
                            dir: path,
                            disk_dir: entry.path(),
                            git_ignore: git_ignore.clone(),
                            file_states,
                        };
                        self.visit_directory(
                            matcher,
                            current_tree,
                            tree_entries_tx.clone(),
                            file_states_tx.clone(),
                            present_files_tx.clone(),
                            directory_to_visit,
                            progress,
                            max_new_file_size,
                        )?;
                    }
                } else if matcher.matches(&path) {
                    if let Some(progress) = progress {
                        progress(&path);
                    }
                    if maybe_current_file_state.is_none()
                        && git_ignore.matches(path.as_internal_file_string())
                    {
                        // If it wasn't already tracked and it matches
                        // the ignored paths, then
                        // ignore it.
                    } else {
                        let metadata = entry.metadata().map_err(|err| SnapshotError::Other {
                            message: format!("Failed to stat file {}", entry.path().display()),
                            err: err.into(),
                        })?;
                        if maybe_current_file_state.is_none() && metadata.len() > max_new_file_size
                        {
                            return Err(SnapshotError::NewFileTooLarge {
                                path: entry.path().clone(),
                                size: HumanByteSize(metadata.len()),
                                max_size: HumanByteSize(max_new_file_size),
                            });
                        }
                        if let Some(new_file_state) = file_state(&metadata) {
                            present_files_tx.send(path.clone()).ok();
                            let update = self.get_updated_tree_value(
                                &path,
                                entry.path(),
                                maybe_current_file_state.as_ref(),
                                current_tree,
                                &new_file_state,
                            )?;
                            if let Some(tree_value) = update {
                                tree_entries_tx.send((path.clone(), tree_value)).ok();
                            }
                            if Some(&new_file_state) != maybe_current_file_state.as_ref() {
                                file_states_tx.send((path, new_file_state)).ok();
                            }
                        }
                    }
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    #[instrument(skip_all)]
    fn make_fsmonitor_matcher(
        &self,
        fsmonitor_kind: FsmonitorKind,
    ) -> Result<FsmonitorMatcher, SnapshotError> {
        let (watchman_clock, changed_files) = match fsmonitor_kind {
            FsmonitorKind::None => (None, None),
            FsmonitorKind::Test { changed_files } => (None, Some(changed_files)),
            #[cfg(feature = "watchman")]
            FsmonitorKind::Watchman => match self.query_watchman() {
                Ok((watchman_clock, changed_files)) => (Some(watchman_clock.into()), changed_files),
                Err(err) => {
                    tracing::warn!(?err, "Failed to query filesystem monitor");
                    (None, None)
                }
            },
            #[cfg(not(feature = "watchman"))]
            FsmonitorKind::Watchman => {
                return Err(SnapshotError::Other {
                    message: "Failed to query the filesystem monitor".to_string(),
                    err: "Cannot query Watchman because jj was not compiled with the `watchman` \
                          feature (consider disabling `core.fsmonitor`)"
                        .into(),
                });
            }
        };
        let matcher: Option<Box<dyn Matcher>> = match changed_files {
            None => None,
            Some(changed_files) => {
                let repo_paths = trace_span!("processing fsmonitor paths").in_scope(|| {
                    changed_files
                        .into_iter()
                        .filter_map(|path| RepoPathBuf::from_relative_path(path).ok())
                        .collect_vec()
                });

                Some(Box::new(FilesMatcher::new(repo_paths)))
            }
        };
        Ok(FsmonitorMatcher {
            matcher,
            watchman_clock,
        })
    }

    fn get_updated_tree_value(
        &self,
        repo_path: &RepoPath,
        disk_path: PathBuf,
        maybe_current_file_state: Option<&FileState>,
        current_tree: &MergedTree,
        new_file_state: &FileState,
    ) -> Result<Option<MergedTreeValue>, SnapshotError> {
        let clean = match maybe_current_file_state {
            None => {
                // untracked
                false
            }
            Some(current_file_state) => {
                // If the file's mtime was set at the same time as this state file's own mtime,
                // then we don't know if the file was modified before or after this state file.
                current_file_state == new_file_state && current_file_state.mtime < self.own_mtime
            }
        };
        if clean {
            Ok(None)
        } else {
            let current_tree_values = current_tree.path_value(repo_path);
            let new_file_type = if !self.symlink_support {
                let mut new_file_type = new_file_state.file_type.clone();
                if matches!(new_file_type, FileType::Normal { .. })
                    && matches!(current_tree_values.as_normal(), Some(TreeValue::Symlink(_)))
                {
                    new_file_type = FileType::Symlink;
                }
                new_file_type
            } else {
                new_file_state.file_type.clone()
            };
            let new_tree_values = match new_file_type {
                FileType::Normal { executable } => self.write_path_to_store(
                    repo_path,
                    &disk_path,
                    &current_tree_values,
                    executable,
                )?,
                FileType::Symlink => {
                    let id = self.write_symlink_to_store(repo_path, &disk_path)?;
                    Merge::normal(TreeValue::Symlink(id))
                }
                FileType::GitSubmodule => panic!("git submodule cannot be written to store"),
            };
            if new_tree_values != current_tree_values {
                Ok(Some(new_tree_values))
            } else {
                Ok(None)
            }
        }
    }

    fn write_path_to_store(
        &self,
        repo_path: &RepoPath,
        disk_path: &Path,
        current_tree_values: &MergedTreeValue,
        executable: FileExecutableFlag,
    ) -> Result<MergedTreeValue, SnapshotError> {
        // If the file contained a conflict before and is now a normal file on disk, we
        // try to parse any conflict markers in the file into a conflict.
        if let Some(current_tree_value) = current_tree_values.as_resolved() {
            #[cfg(unix)]
            let _ = current_tree_value; // use the variable
            let id = self.write_file_to_store(repo_path, disk_path)?;
            // On Windows, we preserve the executable bit from the current tree.
            #[cfg(windows)]
            let executable = {
                let () = executable; // use the variable
                if let Some(TreeValue::File { id: _, executable }) = current_tree_value {
                    *executable
                } else {
                    false
                }
            };
            Ok(Merge::normal(TreeValue::File { id, executable }))
        } else if let Some(old_file_ids) = current_tree_values.to_file_merge() {
            let content = fs::read(disk_path).map_err(|err| SnapshotError::Other {
                message: format!("Failed to open file {}", disk_path.display()),
                err: err.into(),
            })?;
            let new_file_ids = conflicts::update_from_content(
                &old_file_ids,
                self.store.as_ref(),
                repo_path,
                &content,
            )
            .block_on()?;
            match new_file_ids.into_resolved() {
                Ok(file_id) => {
                    #[cfg(windows)]
                    let executable = {
                        let () = executable; // use the variable
                        false
                    };
                    Ok(Merge::normal(TreeValue::File {
                        id: file_id.unwrap(),
                        executable,
                    }))
                }
                Err(new_file_ids) => {
                    if new_file_ids != old_file_ids {
                        Ok(current_tree_values.with_new_file_ids(&new_file_ids))
                    } else {
                        Ok(current_tree_values.clone())
                    }
                }
            }
        } else {
            Ok(current_tree_values.clone())
        }
    }

    fn write_file(
        &self,
        disk_path: &Path,
        contents: &mut dyn Read,
        executable: bool,
    ) -> Result<FileState, CheckoutError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true) // Don't overwrite un-ignored file. Don't follow symlink.
            .open(disk_path)
            .map_err(|err| CheckoutError::Other {
                message: format!("Failed to open file {} for writing", disk_path.display()),
                err: err.into(),
            })?;
        let size = std::io::copy(contents, &mut file).map_err(|err| CheckoutError::Other {
            message: format!("Failed to write file {}", disk_path.display()),
            err: err.into(),
        })?;
        self.set_executable(disk_path, executable)?;
        // Read the file state from the file descriptor. That way, know that the file
        // exists and is of the expected type, and the stat information is most likely
        // accurate, except for other processes modifying the file concurrently (The
        // mtime is set at write time and won't change when we close the file.)
        let metadata = file
            .metadata()
            .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        Ok(FileState::for_file(executable, size, &metadata))
    }

    fn write_symlink(&self, disk_path: &Path, target: String) -> Result<FileState, CheckoutError> {
        let target = PathBuf::from(&target);
        try_symlink(&target, disk_path).map_err(|err| CheckoutError::Other {
            message: format!(
                "Failed to create symlink from {} to {}",
                disk_path.display(),
                target.display()
            ),
            err: err.into(),
        })?;
        let metadata = disk_path
            .symlink_metadata()
            .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        Ok(FileState::for_symlink(&metadata))
    }

    fn write_conflict(
        &self,
        disk_path: &Path,
        conflict_data: Vec<u8>,
    ) -> Result<FileState, CheckoutError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true) // Don't overwrite un-ignored file. Don't follow symlink.
            .open(disk_path)
            .map_err(|err| CheckoutError::Other {
                message: format!("Failed to open file {} for writing", disk_path.display()),
                err: err.into(),
            })?;
        file.write_all(&conflict_data)
            .map_err(|err| CheckoutError::Other {
                message: format!("Failed to write conflict to file {}", disk_path.display()),
                err: err.into(),
            })?;
        let size = conflict_data.len() as u64;
        // TODO: Set the executable bit correctly (when possible) and preserve that on
        // Windows like we do with the executable bit for regular files.
        let metadata = file
            .metadata()
            .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        Ok(FileState::for_file(false, size, &metadata))
    }

    #[cfg_attr(windows, allow(unused_variables))]
    fn set_executable(&self, disk_path: &Path, executable: bool) -> Result<(), CheckoutError> {
        #[cfg(unix)]
        {
            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(disk_path, fs::Permissions::from_mode(mode))
                .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        }
        Ok(())
    }

    pub fn check_out(&mut self, new_tree: &MergedTree) -> Result<CheckoutStats, CheckoutError> {
        let old_tree = self.current_tree().map_err(|err| match err {
            err @ BackendError::ObjectNotFound { .. } => CheckoutError::SourceNotFound {
                source: Box::new(err),
            },
            other => CheckoutError::InternalBackendError(other),
        })?;
        let stats = self
            .update(&old_tree, new_tree, self.sparse_matcher().as_ref())
            .block_on()?;
        self.tree_id = new_tree.id();
        Ok(stats)
    }

    pub fn set_sparse_patterns(
        &mut self,
        sparse_patterns: Vec<RepoPathBuf>,
    ) -> Result<CheckoutStats, CheckoutError> {
        let tree = self.current_tree().map_err(|err| match err {
            err @ BackendError::ObjectNotFound { .. } => CheckoutError::SourceNotFound {
                source: Box::new(err),
            },
            other => CheckoutError::InternalBackendError(other),
        })?;
        let old_matcher = PrefixMatcher::new(&self.sparse_patterns);
        let new_matcher = PrefixMatcher::new(&sparse_patterns);
        let added_matcher = DifferenceMatcher::new(&new_matcher, &old_matcher);
        let removed_matcher = DifferenceMatcher::new(&old_matcher, &new_matcher);
        let empty_tree = MergedTree::resolved(Tree::null(self.store.clone(), RepoPathBuf::root()));
        let added_stats = self.update(&empty_tree, &tree, &added_matcher).block_on()?;
        let removed_stats = self
            .update(&tree, &empty_tree, &removed_matcher)
            .block_on()?;
        self.sparse_patterns = sparse_patterns;
        assert_eq!(added_stats.updated_files, 0);
        assert_eq!(added_stats.removed_files, 0);
        assert_eq!(removed_stats.updated_files, 0);
        assert_eq!(removed_stats.added_files, 0);
        assert_eq!(removed_stats.skipped_files, 0);
        Ok(CheckoutStats {
            updated_files: 0,
            added_files: added_stats.added_files,
            removed_files: removed_stats.removed_files,
            skipped_files: added_stats.skipped_files,
        })
    }

    async fn update(
        &mut self,
        old_tree: &MergedTree,
        new_tree: &MergedTree,
        matcher: &dyn Matcher,
    ) -> Result<CheckoutStats, CheckoutError> {
        // TODO: maybe it's better not include the skipped counts in the "intended"
        // counts
        let mut stats = CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 0,
            skipped_files: 0,
        };
        let mut changed_file_states = Vec::new();
        let mut deleted_files = HashSet::new();
        let mut diff_stream = Box::pin(
            old_tree
                .diff_stream(new_tree, matcher)
                .map(|(path, diff)| async {
                    match diff {
                        Ok((before, after)) => {
                            let result = materialize_tree_value(&self.store, &path, after).await;
                            (path, result.map(|value| (before.is_present(), value)))
                        }
                        Err(err) => (path, Err(err)),
                    }
                })
                .buffered(self.store.concurrency().max(1)),
        );
        while let Some((path, data)) = diff_stream.next().await {
            let (present_before, after) = data?;
            if after.is_absent() {
                stats.removed_files += 1;
            } else if !present_before {
                stats.added_files += 1;
            } else {
                stats.updated_files += 1;
            }
            let disk_path = path.to_fs_path(&self.working_copy_path);

            if present_before {
                fs::remove_file(&disk_path).ok();
            } else if disk_path.exists() {
                changed_file_states.push((path, FileState::placeholder()));
                stats.skipped_files += 1;
                continue;
            }
            if after.is_present() {
                let skip = create_parent_dirs(&self.working_copy_path, &path)?;
                if skip {
                    changed_file_states.push((path, FileState::placeholder()));
                    stats.skipped_files += 1;
                    continue;
                }
            }
            // TODO: Check that the file has not changed before overwriting/removing it.
            let file_state = match after {
                MaterializedTreeValue::Absent => {
                    let mut parent_dir = disk_path.parent().unwrap();
                    loop {
                        if fs::remove_dir(parent_dir).is_err() {
                            break;
                        }
                        parent_dir = parent_dir.parent().unwrap();
                    }
                    deleted_files.insert(path);
                    continue;
                }
                MaterializedTreeValue::File {
                    executable,
                    mut reader,
                    ..
                } => self.write_file(&disk_path, &mut reader, executable)?,
                MaterializedTreeValue::Symlink { id: _, target } => {
                    if self.symlink_support {
                        self.write_symlink(&disk_path, target)?
                    } else {
                        self.write_file(&disk_path, &mut target.as_bytes(), false)?
                    }
                }
                MaterializedTreeValue::GitSubmodule(_) => {
                    println!("ignoring git submodule at {path:?}");
                    FileState::for_gitsubmodule()
                }
                MaterializedTreeValue::Tree(_) => {
                    panic!("unexpected tree entry in diff at {path:?}");
                }
                MaterializedTreeValue::Conflict { id: _, contents } => {
                    self.write_conflict(&disk_path, contents)?
                }
            };
            changed_file_states.push((path, file_state));
        }
        self.file_states
            .merge_in(changed_file_states, &deleted_files);
        Ok(stats)
    }

    pub async fn reset(&mut self, new_tree: &MergedTree) -> Result<(), ResetError> {
        let old_tree = self.current_tree().map_err(|err| match err {
            err @ BackendError::ObjectNotFound { .. } => ResetError::SourceNotFound {
                source: Box::new(err),
            },
            other => ResetError::InternalBackendError(other),
        })?;

        let matcher = self.sparse_matcher();
        let mut changed_file_states = Vec::new();
        let mut deleted_files = HashSet::new();
        let mut diff_stream = old_tree.diff_stream(new_tree, matcher.as_ref());
        while let Some((path, diff)) = diff_stream.next().await {
            let (_before, after) = diff?;
            if after.is_absent() {
                deleted_files.insert(path);
            } else {
                let file_type = match after.into_resolved() {
                    Ok(value) => match value.unwrap() {
                        #[cfg(unix)]
                        TreeValue::File { id: _, executable } => FileType::Normal { executable },
                        #[cfg(windows)]
                        TreeValue::File { .. } => FileType::Normal { executable: () },
                        TreeValue::Symlink(_id) => FileType::Symlink,
                        TreeValue::Conflict(_id) => {
                            panic!("unexpected conflict entry in diff at {path:?}");
                        }
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {path:?}");
                            FileType::GitSubmodule
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {path:?}");
                        }
                    },
                    Err(_values) => {
                        // TODO: Try to set the executable bit based on the conflict
                        FileType::Normal {
                            executable: FileExecutableFlag::default(),
                        }
                    }
                };
                let file_state = FileState {
                    file_type,
                    mtime: MillisSinceEpoch(0),
                    size: 0,
                };
                changed_file_states.push((path, file_state));
            }
        }
        self.file_states
            .merge_in(changed_file_states, &deleted_files);
        self.tree_id = new_tree.id();
        Ok(())
    }

    pub async fn recover(&mut self, new_tree: &MergedTree) -> Result<(), ResetError> {
        self.file_states.clear();
        self.tree_id = self.store.empty_merged_tree_id();
        self.reset(new_tree).await
    }
}

fn checkout_error_for_stat_error(err: std::io::Error, path: &Path) -> CheckoutError {
    CheckoutError::Other {
        message: format!("Failed to stat file {}", path.display()),
        err: err.into(),
    }
}

/// Working copy state stored in "checkout" file.
#[derive(Clone, Debug)]
struct CheckoutState {
    operation_id: OperationId,
    workspace_id: WorkspaceId,
}

pub struct LocalWorkingCopy {
    store: Arc<Store>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    checkout_state: OnceCell<CheckoutState>,
    tree_state: OnceCell<TreeState>,
}

impl WorkingCopy for LocalWorkingCopy {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        Self::name()
    }

    fn path(&self) -> &Path {
        &self.working_copy_path
    }

    fn workspace_id(&self) -> &WorkspaceId {
        &self.checkout_state().workspace_id
    }

    fn operation_id(&self) -> &OperationId {
        &self.checkout_state().operation_id
    }

    fn tree_id(&self) -> Result<&MergedTreeId, WorkingCopyStateError> {
        Ok(self.tree_state()?.current_tree_id())
    }

    fn sparse_patterns(&self) -> Result<&[RepoPathBuf], WorkingCopyStateError> {
        Ok(self.tree_state()?.sparse_patterns())
    }

    fn start_mutation(&self) -> Result<Box<dyn LockedWorkingCopy>, WorkingCopyStateError> {
        let lock_path = self.state_path.join("working_copy.lock");
        let lock = FileLock::lock(lock_path);

        let wc = LocalWorkingCopy {
            store: self.store.clone(),
            working_copy_path: self.working_copy_path.clone(),
            state_path: self.state_path.clone(),
            // Empty so we re-read the state after taking the lock
            checkout_state: OnceCell::new(),
            // TODO: It's expensive to reload the whole tree. We should copy it from `self` if it
            // hasn't changed.
            tree_state: OnceCell::new(),
        };
        let old_operation_id = wc.operation_id().clone();
        let old_tree_id = wc.tree_id()?.clone();
        Ok(Box::new(LockedLocalWorkingCopy {
            wc,
            lock,
            old_operation_id,
            old_tree_id,
            tree_state_dirty: false,
        }))
    }
}

impl LocalWorkingCopy {
    pub fn name() -> &'static str {
        "local"
    }

    /// Initializes a new working copy at `working_copy_path`. The working
    /// copy's state will be stored in the `state_path` directory. The working
    /// copy will have the empty tree checked out.
    pub fn init(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        operation_id: OperationId,
        workspace_id: WorkspaceId,
    ) -> Result<LocalWorkingCopy, WorkingCopyStateError> {
        let proto = crate::protos::working_copy::Checkout {
            operation_id: operation_id.to_bytes(),
            workspace_id: workspace_id.as_str().to_string(),
        };
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state_path.join("checkout"))
            .unwrap();
        file.write_all(&proto.encode_to_vec()).unwrap();
        let tree_state =
            TreeState::init(store.clone(), working_copy_path.clone(), state_path.clone()).map_err(
                |err| WorkingCopyStateError {
                    message: "Failed to initialize working copy state".to_string(),
                    err: err.into(),
                },
            )?;
        Ok(LocalWorkingCopy {
            store,
            working_copy_path,
            state_path,
            checkout_state: OnceCell::new(),
            tree_state: OnceCell::with_value(tree_state),
        })
    }

    pub fn load(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> LocalWorkingCopy {
        LocalWorkingCopy {
            store,
            working_copy_path,
            state_path,
            checkout_state: OnceCell::new(),
            tree_state: OnceCell::new(),
        }
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

    #[instrument(skip_all)]
    fn tree_state(&self) -> Result<&TreeState, WorkingCopyStateError> {
        self.tree_state
            .get_or_try_init(|| {
                TreeState::load(
                    self.store.clone(),
                    self.working_copy_path.clone(),
                    self.state_path.clone(),
                )
            })
            .map_err(|err| WorkingCopyStateError {
                message: "Failed to read working copy state".to_string(),
                err: err.into(),
            })
    }

    fn tree_state_mut(&mut self) -> Result<&mut TreeState, WorkingCopyStateError> {
        self.tree_state()?; // ensure loaded
        Ok(self.tree_state.get_mut().unwrap())
    }

    pub fn file_states(&self) -> Result<FileStates<'_>, WorkingCopyStateError> {
        Ok(self.tree_state()?.file_states())
    }

    #[instrument(skip_all)]
    fn save(&mut self) {
        self.write_proto(crate::protos::working_copy::Checkout {
            operation_id: self.operation_id().to_bytes(),
            workspace_id: self.workspace_id().as_str().to_string(),
        });
    }

    #[cfg(feature = "watchman")]
    pub fn query_watchman(
        &self,
    ) -> Result<(watchman::Clock, Option<Vec<PathBuf>>), WorkingCopyStateError> {
        self.tree_state()?
            .query_watchman()
            .map_err(|err| WorkingCopyStateError {
                message: "Failed to query watchman".to_string(),
                err: err.into(),
            })
    }
}

pub struct LocalWorkingCopyFactory {}

impl WorkingCopyFactory for LocalWorkingCopyFactory {
    fn init_working_copy(
        &self,
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        operation_id: OperationId,
        workspace_id: WorkspaceId,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError> {
        Ok(Box::new(LocalWorkingCopy::init(
            store,
            working_copy_path,
            state_path,
            operation_id,
            workspace_id,
        )?))
    }

    fn load_working_copy(
        &self,
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> Box<dyn WorkingCopy> {
        Box::new(LocalWorkingCopy::load(store, working_copy_path, state_path))
    }
}

/// A working copy that's locked on disk. The lock is held until you call
/// `finish()` or `discard()`.
pub struct LockedLocalWorkingCopy {
    wc: LocalWorkingCopy,
    #[allow(dead_code)]
    lock: FileLock,
    old_operation_id: OperationId,
    old_tree_id: MergedTreeId,
    tree_state_dirty: bool,
}

impl LockedWorkingCopy for LockedLocalWorkingCopy {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn old_operation_id(&self) -> &OperationId {
        &self.old_operation_id
    }

    fn old_tree_id(&self) -> &MergedTreeId {
        &self.old_tree_id
    }

    fn snapshot(&mut self, options: SnapshotOptions) -> Result<MergedTreeId, SnapshotError> {
        let tree_state = self
            .wc
            .tree_state_mut()
            .map_err(|err| SnapshotError::Other {
                message: "Failed to read the working copy state".to_string(),
                err: err.into(),
            })?;
        self.tree_state_dirty |= tree_state.snapshot(options)?;
        Ok(tree_state.current_tree_id().clone())
    }

    fn check_out(&mut self, commit: &Commit) -> Result<CheckoutStats, CheckoutError> {
        // TODO: Write a "pending_checkout" file with the new TreeId so we can
        // continue an interrupted update if we find such a file.
        let new_tree = commit.tree()?;
        let stats = self
            .wc
            .tree_state_mut()
            .map_err(|err| CheckoutError::Other {
                message: "Failed to load the working copy state".to_string(),
                err: err.into(),
            })?
            .check_out(&new_tree)?;
        self.tree_state_dirty = true;
        Ok(stats)
    }

    fn reset(&mut self, commit: &Commit) -> Result<(), ResetError> {
        let new_tree = commit.tree()?;
        self.wc
            .tree_state_mut()
            .map_err(|err| ResetError::Other {
                message: "Failed to read the working copy state".to_string(),
                err: err.into(),
            })?
            .reset(&new_tree)
            .block_on()?;
        self.tree_state_dirty = true;
        Ok(())
    }

    fn recover(&mut self, commit: &Commit) -> Result<(), ResetError> {
        let new_tree = commit.tree()?;
        self.wc
            .tree_state_mut()
            .map_err(|err| ResetError::Other {
                message: "Failed to read the working copy state".to_string(),
                err: err.into(),
            })?
            .recover(&new_tree)
            .block_on()?;
        self.tree_state_dirty = true;
        Ok(())
    }

    fn sparse_patterns(&self) -> Result<&[RepoPathBuf], WorkingCopyStateError> {
        self.wc.sparse_patterns()
    }

    fn set_sparse_patterns(
        &mut self,
        new_sparse_patterns: Vec<RepoPathBuf>,
    ) -> Result<CheckoutStats, CheckoutError> {
        // TODO: Write a "pending_checkout" file with new sparse patterns so we can
        // continue an interrupted update if we find such a file.
        let stats = self
            .wc
            .tree_state_mut()
            .map_err(|err| CheckoutError::Other {
                message: "Failed to load the working copy state".to_string(),
                err: err.into(),
            })?
            .set_sparse_patterns(new_sparse_patterns)?;
        self.tree_state_dirty = true;
        Ok(stats)
    }

    #[instrument(skip_all)]
    fn finish(
        mut self: Box<Self>,
        operation_id: OperationId,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError> {
        assert!(self.tree_state_dirty || &self.old_tree_id == self.wc.tree_id()?);
        if self.tree_state_dirty {
            self.wc
                .tree_state_mut()?
                .save()
                .map_err(|err| WorkingCopyStateError {
                    message: "Failed to write working copy state".to_string(),
                    err: Box::new(err),
                })?;
        }
        if self.old_operation_id != operation_id {
            self.wc.checkout_state_mut().operation_id = operation_id;
            self.wc.save();
        }
        // TODO: Clear the "pending_checkout" file here.
        Ok(Box::new(self.wc))
    }
}

impl LockedLocalWorkingCopy {
    pub fn reset_watchman(&mut self) -> Result<(), SnapshotError> {
        self.wc
            .tree_state_mut()
            .map_err(|err| SnapshotError::Other {
                message: "Failed to read the working copy state".to_string(),
                err: err.into(),
            })?
            .reset_watchman();
        self.tree_state_dirty = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use maplit::hashset;

    use super::*;

    fn repo_path(value: &str) -> &RepoPath {
        RepoPath::from_internal_string(value)
    }

    #[test]
    fn test_file_states_merge() {
        let new_state = |size| FileState {
            file_type: FileType::Normal {
                executable: FileExecutableFlag::default(),
            },
            mtime: MillisSinceEpoch(0),
            size,
        };
        let new_static_entry = |path: &'static str, size| (repo_path(path), new_state(size));
        let new_owned_entry = |path: &str, size| (repo_path(path).to_owned(), new_state(size));
        let new_proto_entry = |path: &str, size| {
            file_state_entry_to_proto(repo_path(path).to_owned(), &new_state(size))
        };
        let data = vec![
            new_proto_entry("aa", 0),
            new_proto_entry("b#", 4), // '#' < '/'
            new_proto_entry("b/c", 1),
            new_proto_entry("b/d/e", 2),
            new_proto_entry("b/e", 3),
            new_proto_entry("bc", 5),
        ];
        let mut file_states = FileStatesMap::from_proto_unsorted(data);

        let changed_file_states = vec![
            new_owned_entry("aa", 10),    // change
            new_owned_entry("b/d/f", 11), // add
            new_owned_entry("b/e", 12),   // change
            new_owned_entry("c", 13),     // add
        ];
        let deleted_files = hashset! {
            repo_path("b/c").to_owned(),
            repo_path("b#").to_owned(),
        };
        file_states.merge_in(changed_file_states, &deleted_files);
        assert_eq!(
            file_states.all().iter().collect_vec(),
            vec![
                new_static_entry("aa", 10),
                new_static_entry("b/d/e", 2),
                new_static_entry("b/d/f", 11),
                new_static_entry("b/e", 12),
                new_static_entry("bc", 5),
                new_static_entry("c", 13),
            ],
        );
    }

    #[test]
    fn test_file_states_lookup() {
        let new_state = |size| FileState {
            file_type: FileType::Normal {
                executable: FileExecutableFlag::default(),
            },
            mtime: MillisSinceEpoch(0),
            size,
        };
        let new_proto_entry = |path: &str, size| {
            file_state_entry_to_proto(repo_path(path).to_owned(), &new_state(size))
        };
        let data = vec![
            new_proto_entry("aa", 0),
            new_proto_entry("b/c", 1),
            new_proto_entry("b/d/e", 2),
            new_proto_entry("b/e", 3),
            new_proto_entry("b#", 4), // '#' < '/'
            new_proto_entry("bc", 5),
        ];
        let file_states = FileStates::from_sorted(&data);

        assert_eq!(
            file_states.prefixed(repo_path("")).paths().collect_vec(),
            ["aa", "b/c", "b/d/e", "b/e", "b#", "bc"].map(repo_path)
        );
        assert!(file_states.prefixed(repo_path("a")).is_empty());
        assert_eq!(
            file_states.prefixed(repo_path("aa")).paths().collect_vec(),
            ["aa"].map(repo_path)
        );
        assert_eq!(
            file_states.prefixed(repo_path("b")).paths().collect_vec(),
            ["b/c", "b/d/e", "b/e"].map(repo_path)
        );
        assert_eq!(
            file_states.prefixed(repo_path("b/d")).paths().collect_vec(),
            ["b/d/e"].map(repo_path)
        );
        assert_eq!(
            file_states.prefixed(repo_path("b#")).paths().collect_vec(),
            ["b#"].map(repo_path)
        );
        assert_eq!(
            file_states.prefixed(repo_path("bc")).paths().collect_vec(),
            ["bc"].map(repo_path)
        );
        assert!(file_states.prefixed(repo_path("z")).is_empty());

        assert!(!file_states.contains_path(repo_path("a")));
        assert!(file_states.contains_path(repo_path("aa")));
        assert!(file_states.contains_path(repo_path("b/d/e")));
        assert!(!file_states.contains_path(repo_path("b/d")));
        assert!(file_states.contains_path(repo_path("b#")));
        assert!(file_states.contains_path(repo_path("bc")));
        assert!(!file_states.contains_path(repo_path("z")));

        assert_eq!(file_states.get(repo_path("a")), None);
        assert_eq!(file_states.get(repo_path("aa")), Some(new_state(0)));
        assert_eq!(file_states.get(repo_path("b/d/e")), Some(new_state(2)));
        assert_eq!(file_states.get(repo_path("bc")), Some(new_state(5)));
        assert_eq!(file_states.get(repo_path("z")), None);
    }
}
