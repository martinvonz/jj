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
#![allow(clippy::let_unit_value)]

use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::fs::DirEntry;
use std::fs::File;
use std::fs::Metadata;
use std::fs::OpenOptions;
use std::io;
use std::io::Read;
use std::io::Write;
use std::iter;
use std::mem;
use std::ops::Range;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::slice;
use std::sync::mpsc::channel;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use either::Either;
use futures::StreamExt;
use itertools::EitherOrBoth;
use itertools::Itertools;
use once_cell::unsync::OnceCell;
use pollster::FutureExt;
use prost::Message;
use rayon::iter::IntoParallelIterator;
use rayon::prelude::IndexedParallelIterator;
use rayon::prelude::ParallelIterator;
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::instrument;
use tracing::trace_span;

use crate::backend::BackendError;
use crate::backend::BackendResult;
use crate::backend::FileId;
use crate::backend::MergedTreeId;
use crate::backend::MillisSinceEpoch;
use crate::backend::SymlinkId;
use crate::backend::TreeId;
use crate::backend::TreeValue;
use crate::commit::Commit;
use crate::conflicts;
use crate::conflicts::choose_materialized_conflict_marker_len;
use crate::conflicts::materialize_merge_result_to_bytes_with_marker_len;
use crate::conflicts::materialize_tree_value;
use crate::conflicts::ConflictMarkerStyle;
use crate::conflicts::MaterializedTreeValue;
use crate::conflicts::MIN_CONFLICT_MARKER_LEN;
use crate::file_util::check_symlink_support;
use crate::file_util::try_symlink;
#[cfg(feature = "watchman")]
use crate::fsmonitor::watchman;
use crate::fsmonitor::FsmonitorSettings;
#[cfg(feature = "watchman")]
use crate::fsmonitor::WatchmanConfig;
use crate::gitignore::GitIgnoreFile;
use crate::lock::FileLock;
use crate::matchers::DifferenceMatcher;
use crate::matchers::EverythingMatcher;
use crate::matchers::FilesMatcher;
use crate::matchers::IntersectionMatcher;
use crate::matchers::Matcher;
use crate::matchers::PrefixMatcher;
use crate::merge::Merge;
use crate::merge::MergeBuilder;
use crate::merge::MergedTreeValue;
use crate::merged_tree::MergedTree;
use crate::merged_tree::MergedTreeBuilder;
use crate::merged_tree::TreeDiffEntry;
use crate::object_id::ObjectId;
use crate::op_store::OperationId;
use crate::op_store::WorkspaceId;
use crate::repo_path::RepoPath;
use crate::repo_path::RepoPathBuf;
use crate::repo_path::RepoPathComponent;
use crate::store::Store;
use crate::tree::Tree;
use crate::working_copy::CheckoutError;
use crate::working_copy::CheckoutOptions;
use crate::working_copy::CheckoutStats;
use crate::working_copy::LockedWorkingCopy;
use crate::working_copy::ResetError;
use crate::working_copy::SnapshotError;
use crate::working_copy::SnapshotOptions;
use crate::working_copy::SnapshotProgress;
use crate::working_copy::SnapshotStats;
use crate::working_copy::UntrackedReason;
use crate::working_copy::WorkingCopy;
use crate::working_copy::WorkingCopyFactory;
use crate::working_copy::WorkingCopyStateError;

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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct MaterializedConflictData {
    pub conflict_marker_len: u32,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FileState {
    pub file_type: FileType,
    pub mtime: MillisSinceEpoch,
    pub size: u64,
    pub materialized_conflict_data: Option<MaterializedConflictData>,
    /* TODO: What else do we need here? Git stores a lot of fields.
     * TODO: Could possibly handle case-insensitive file systems keeping an
     *       Option<PathBuf> with the actual path here. */
}

impl FileState {
    /// Check whether a file state appears clean compared to a previous file
    /// state, ignoring materialized conflict data.
    pub fn is_clean(&self, old_file_state: &Self) -> bool {
        self.file_type == old_file_state.file_type
            && self.mtime == old_file_state.mtime
            && self.size == old_file_state.size
    }

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
            materialized_conflict_data: None,
        }
    }

    fn for_file(
        executable: bool,
        size: u64,
        metadata: &Metadata,
        materialized_conflict_data: Option<MaterializedConflictData>,
    ) -> Self {
        #[cfg(windows)]
        let executable = {
            // Windows doesn't support executable bit.
            let _ = executable;
        };
        FileState {
            file_type: FileType::Normal { executable },
            mtime: mtime_from_metadata(metadata),
            size,
            materialized_conflict_data,
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
            materialized_conflict_data: None,
        }
    }

    fn for_gitsubmodule() -> Self {
        FileState {
            file_type: FileType::GitSubmodule,
            mtime: MillisSinceEpoch(0),
            size: 0,
            materialized_conflict_data: None,
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

    fn from_proto(
        mut data: Vec<crate::protos::working_copy::FileStateEntry>,
        is_sorted: bool,
    ) -> Self {
        if !is_sorted {
            data.sort_unstable_by(|entry1, entry2| {
                let path1 = RepoPath::from_internal_string(&entry1.path);
                let path2 = RepoPath::from_internal_string(&entry2.path);
                path1.cmp(path2)
            });
        }
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

    /// Faster version of `prefixed("<dir>/<base>")`. Requires that all entries
    /// share the same prefix `dir`.
    fn prefixed_at(&self, dir: &RepoPath, base: &RepoPathComponent) -> Self {
        let range = self.prefixed_range_at(dir, base);
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

    /// Faster version of `get("<dir>/<name>")`. Requires that all entries share
    /// the same prefix `dir`.
    fn get_at(&self, dir: &RepoPath, name: &RepoPathComponent) -> Option<FileState> {
        let pos = self.exact_position_at(dir, name)?;
        let (_, state) = file_state_entry_from_proto(&self.data[pos]);
        Some(state)
    }

    fn exact_position(&self, path: &RepoPath) -> Option<usize> {
        self.data
            .binary_search_by(|entry| RepoPath::from_internal_string(&entry.path).cmp(path))
            .ok()
    }

    fn exact_position_at(&self, dir: &RepoPath, name: &RepoPathComponent) -> Option<usize> {
        debug_assert!(self.paths().all(|path| path.starts_with(dir)));
        let slash_len = !dir.is_root() as usize;
        let prefix_len = dir.as_internal_file_string().len() + slash_len;
        self.data
            .binary_search_by(|entry| {
                let tail = entry.path.get(prefix_len..).unwrap_or("");
                match tail.split_once('/') {
                    // "<name>/*" > "<name>"
                    Some((pre, _)) => pre.cmp(name.as_internal_str()).then(Ordering::Greater),
                    None => tail.cmp(name.as_internal_str()),
                }
            })
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

    fn prefixed_range_at(&self, dir: &RepoPath, base: &RepoPathComponent) -> Range<usize> {
        debug_assert!(self.paths().all(|path| path.starts_with(dir)));
        let slash_len = !dir.is_root() as usize;
        let prefix_len = dir.as_internal_file_string().len() + slash_len;
        let start = self.data.partition_point(|entry| {
            let tail = entry.path.get(prefix_len..).unwrap_or("");
            let entry_name = tail.split_once('/').map_or(tail, |(name, _)| name);
            entry_name < base.as_internal_str()
        });
        let len = self.data[start..].partition_point(|entry| {
            let tail = entry.path.get(prefix_len..).unwrap_or("");
            let entry_name = tail.split_once('/').map_or(tail, |(name, _)| name);
            entry_name == base.as_internal_str()
        });
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
        materialized_conflict_data: proto.materialized_conflict_data.as_ref().map(|data| {
            MaterializedConflictData {
                conflict_marker_len: data.conflict_marker_len,
            }
        }),
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
    proto.materialized_conflict_data = file_state.materialized_conflict_data.map(|data| {
        crate::protos::working_copy::MaterializedConflictData {
            conflict_marker_len: data.conflict_marker_len,
        }
    });
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
/// `repo_path` parent. Returns disk path for the `repo_path` file.
///
/// If an intermediate directory exists and if it is a file or symlink, this
/// function returns `Ok(None)` to signal that the path should be skipped.
/// The `working_copy_path` directory may be a symlink.
///
/// If an existing or newly-created sub directory points to ".git" or ".jj",
/// this function returns an error.
///
/// Note that this does not prevent TOCTOU bugs caused by concurrent checkouts.
/// Another process may remove the directory created by this function and put a
/// symlink there.
fn create_parent_dirs(
    working_copy_path: &Path,
    repo_path: &RepoPath,
) -> Result<Option<PathBuf>, CheckoutError> {
    let (parent_path, basename) = repo_path.split().expect("repo path shouldn't be root");
    let mut dir_path = working_copy_path.to_owned();
    for c in parent_path.components() {
        // Ensure that the name is a normal entry of the current dir_path.
        dir_path.push(c.to_fs_name().map_err(|err| err.with_path(repo_path))?);
        // A directory named ".git" or ".jj" can be temporarily created. It
        // might trick workspace path discovery, but is harmless so long as the
        // directory is empty.
        let new_dir_created = match fs::create_dir(&dir_path) {
            Ok(()) => true, // New directory
            Err(err) => match dir_path.symlink_metadata() {
                Ok(m) if m.is_dir() => false, // Existing directory
                Ok(_) => {
                    return Ok(None); // Skip existing file or symlink
                }
                Err(_) => {
                    return Err(CheckoutError::Other {
                        message: format!(
                            "Failed to create parent directories for {}",
                            repo_path.to_fs_path_unchecked(working_copy_path).display(),
                        ),
                        err: err.into(),
                    })
                }
            },
        };
        // Invalid component (e.g. "..") should have been rejected.
        // The current dir_path should be an entry of dir_path.parent().
        reject_reserved_existing_path(&dir_path).inspect_err(|_| {
            if new_dir_created {
                fs::remove_dir(&dir_path).ok();
            }
        })?;
    }

    let mut file_path = dir_path;
    file_path.push(
        basename
            .to_fs_name()
            .map_err(|err| err.with_path(repo_path))?,
    );
    Ok(Some(file_path))
}

/// Removes existing file named `disk_path` if any. Returns `Ok(true)` if the
/// file was there and got removed, meaning that new file can be safely created.
///
/// If the existing file points to ".git" or ".jj", this function returns an
/// error.
fn remove_old_file(disk_path: &Path) -> Result<bool, CheckoutError> {
    reject_reserved_existing_path(disk_path)?;
    match fs::remove_file(disk_path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        // TODO: Use io::ErrorKind::IsADirectory if it gets stabilized
        Err(_) if disk_path.symlink_metadata().is_ok_and(|m| m.is_dir()) => Ok(false),
        Err(err) => Err(CheckoutError::Other {
            message: format!("Failed to remove file {}", disk_path.display()),
            err: err.into(),
        }),
    }
}

/// Checks if new file or symlink named `disk_path` can be created.
///
/// If the file already exists, this function return `Ok(false)` to signal
/// that the path should be skipped.
///
/// If the path may point to ".git" or ".jj" entry, this function returns an
/// error.
///
/// This function can fail if `disk_path.parent()` isn't a directory.
fn can_create_new_file(disk_path: &Path) -> Result<bool, CheckoutError> {
    // New file or symlink will be created by caller. If it were pointed to by
    // name ".git" or ".jj", git/jj CLI could be tricked to load configuration
    // from an attacker-controlled location. So we first test the path by
    // creating an empty file.
    let new_file_created = match OpenOptions::new()
        .write(true)
        .create_new(true) // Don't overwrite, don't follow symlink
        .open(disk_path)
    {
        Ok(_) => true,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => false,
        // Workaround for "Access is denied. (os error 5)" error on Windows.
        Err(_) => match disk_path.symlink_metadata() {
            Ok(_) => false,
            Err(err) => {
                return Err(CheckoutError::Other {
                    message: format!("Failed to stat {}", disk_path.display()),
                    err: err.into(),
                })
            }
        },
    };
    reject_reserved_existing_path(disk_path).inspect_err(|_| {
        if new_file_created {
            fs::remove_file(disk_path).ok();
        }
    })?;
    if new_file_created {
        fs::remove_file(disk_path).map_err(|err| CheckoutError::Other {
            message: format!("Failed to remove temporary file {}", disk_path.display()),
            err: err.into(),
        })?;
    }
    Ok(new_file_created)
}

const RESERVED_DIR_NAMES: &[&str] = &[".git", ".jj"];

/// Suppose the `disk_path` exists, checks if the last component points to
/// ".git" or ".jj" in the same parent directory.
fn reject_reserved_existing_path(disk_path: &Path) -> Result<(), CheckoutError> {
    let parent_dir_path = disk_path.parent().expect("content path shouldn't be root");
    for name in RESERVED_DIR_NAMES {
        let reserved_path = parent_dir_path.join(name);
        match same_file::is_same_file(disk_path, &reserved_path) {
            Ok(true) => {
                return Err(CheckoutError::ReservedPathComponent {
                    path: disk_path.to_owned(),
                    name,
                });
            }
            Ok(false) => {}
            // If the existing disk_path pointed to the reserved path, the
            // reserved path would exist.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CheckoutError::Other {
                    message: format!("Failed to validate path {}", disk_path.display()),
                    err: err.into(),
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
            materialized_conflict_data: None,
        }
    })
}

struct FsmonitorMatcher {
    matcher: Option<Box<dyn Matcher>>,
    watchman_clock: Option<crate::protos::working_copy::WatchmanClock>,
}

#[derive(Debug, Error)]
pub enum TreeStateError {
    #[error("Reading tree state from {path}")]
    ReadTreeState { path: PathBuf, source: io::Error },
    #[error("Decoding tree state from {path}")]
    DecodeTreeState {
        path: PathBuf,
        source: prost::DecodeError,
    },
    #[error("Writing tree state to temporary file {path}")]
    WriteTreeState { path: PathBuf, source: io::Error },
    #[error("Persisting tree state to file {path}")]
    PersistTreeState { path: PathBuf, source: io::Error },
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
        TreeState {
            store,
            working_copy_path,
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
            Err(ref err) if err.kind() == io::ErrorKind::NotFound => {
                return TreeState::init(store, working_copy_path, state_path);
            }
            Err(err) => {
                return Err(TreeStateError::ReadTreeState {
                    path: tree_state_path,
                    source: err,
                });
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
        self.file_states =
            FileStatesMap::from_proto(proto.file_states, proto.is_file_states_sorted);
        self.sparse_patterns = sparse_patterns_from_proto(proto.sparse_patterns.as_ref());
        self.watchman_clock = proto.watchman_clock;
        Ok(())
    }

    #[allow(unknown_lints)] // XXX FIXME (aseipp): nightly bogons; re-test this occasionally
    #[allow(clippy::assigning_clones)]
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
        // `FileStatesMap` is guaranteed to be sorted.
        proto.is_file_states_sorted = true;
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

    fn current_tree(&self) -> BackendResult<MergedTree> {
        self.store.get_root_tree(&self.tree_id)
    }

    fn reset_watchman(&mut self) {
        self.watchman_clock.take();
    }

    #[cfg(feature = "watchman")]
    #[tokio::main(flavor = "current_thread")]
    #[instrument(skip(self))]
    pub async fn query_watchman(
        &self,
        config: &WatchmanConfig,
    ) -> Result<(watchman::Clock, Option<Vec<PathBuf>>), TreeStateError> {
        let fsmonitor = watchman::Fsmonitor::init(&self.working_copy_path, config)
            .await
            .map_err(|err| TreeStateError::Fsmonitor(Box::new(err)))?;
        let previous_clock = self.watchman_clock.clone().map(watchman::Clock::from);
        let changed_files = fsmonitor
            .query_changed_files(previous_clock)
            .await
            .map_err(|err| TreeStateError::Fsmonitor(Box::new(err)))?;
        Ok(changed_files)
    }

    #[cfg(feature = "watchman")]
    #[tokio::main(flavor = "current_thread")]
    #[instrument(skip(self))]
    pub async fn is_watchman_trigger_registered(
        &self,
        config: &WatchmanConfig,
    ) -> Result<bool, TreeStateError> {
        let fsmonitor = watchman::Fsmonitor::init(&self.working_copy_path, config)
            .await
            .map_err(|err| TreeStateError::Fsmonitor(Box::new(err)))?;
        fsmonitor
            .is_trigger_registered()
            .await
            .map_err(|err| TreeStateError::Fsmonitor(Box::new(err)))
    }
}

/// Functions to snapshot local-disk files to the store.
impl TreeState {
    /// Look for changes to the working copy. If there are any changes, create
    /// a new tree from it.
    #[instrument(skip_all)]
    pub fn snapshot(
        &mut self,
        options: &SnapshotOptions,
    ) -> Result<(bool, SnapshotStats), SnapshotError> {
        let &SnapshotOptions {
            ref base_ignores,
            ref fsmonitor_settings,
            progress,
            start_tracking_matcher,
            max_new_file_size,
            conflict_marker_style,
        } = options;

        let sparse_matcher = self.sparse_matcher();

        let fsmonitor_clock_needs_save = *fsmonitor_settings != FsmonitorSettings::None;
        let mut is_dirty = fsmonitor_clock_needs_save;
        let FsmonitorMatcher {
            matcher: fsmonitor_matcher,
            watchman_clock,
        } = self.make_fsmonitor_matcher(fsmonitor_settings)?;
        let fsmonitor_matcher = match fsmonitor_matcher.as_ref() {
            None => &EverythingMatcher,
            Some(fsmonitor_matcher) => fsmonitor_matcher.as_ref(),
        };

        let matcher = IntersectionMatcher::new(sparse_matcher.as_ref(), fsmonitor_matcher);
        if matcher.visit(RepoPath::root()).is_nothing() {
            // No need to load the current tree, set up channels, etc.
            self.watchman_clock = watchman_clock;
            return Ok((is_dirty, SnapshotStats::default()));
        }

        let (tree_entries_tx, tree_entries_rx) = channel();
        let (file_states_tx, file_states_rx) = channel();
        let (untracked_paths_tx, untracked_paths_rx) = channel();
        let (deleted_files_tx, deleted_files_rx) = channel();

        trace_span!("traverse filesystem").in_scope(|| -> Result<(), SnapshotError> {
            let snapshotter = FileSnapshotter {
                tree_state: self,
                current_tree: &self.current_tree()?,
                matcher: &matcher,
                start_tracking_matcher,
                // Move tx sides so they'll be dropped at the end of the scope.
                tree_entries_tx,
                file_states_tx,
                untracked_paths_tx,
                deleted_files_tx,
                error: OnceLock::new(),
                progress,
                max_new_file_size,
                conflict_marker_style,
            };
            let directory_to_visit = DirectoryToVisit {
                dir: RepoPathBuf::root(),
                disk_dir: self.working_copy_path.clone(),
                git_ignore: base_ignores.clone(),
                file_states: self.file_states.all(),
            };
            // Here we use scope as a queue of per-directory jobs.
            rayon::scope(|scope| {
                snapshotter.spawn_ok(scope, |scope| {
                    snapshotter.visit_directory(directory_to_visit, scope)
                });
            });
            snapshotter.into_result()
        })?;

        let stats = SnapshotStats {
            untracked_paths: untracked_paths_rx.into_iter().collect(),
        };
        let mut tree_builder = MergedTreeBuilder::new(self.tree_id.clone());
        trace_span!("process tree entries").in_scope(|| {
            for (path, tree_values) in &tree_entries_rx {
                tree_builder.set_or_remove(path, tree_values);
            }
        });
        let deleted_files = trace_span!("process deleted tree entries").in_scope(|| {
            let deleted_files = HashSet::from_iter(deleted_files_rx);
            is_dirty |= !deleted_files.is_empty();
            for file in &deleted_files {
                tree_builder.set_or_remove(file.clone(), Merge::absent());
            }
            deleted_files
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
                .filter_map(|(path, result)| result.is_ok().then_some(path))
                .collect();
            let file_states = self.file_states.all();
            let state_paths: HashSet<_> = file_states.paths().map(|path| path.to_owned()).collect();
            assert_eq!(state_paths, tree_paths);
        }
        self.watchman_clock = watchman_clock;
        Ok((is_dirty, stats))
    }

    #[instrument(skip_all)]
    fn make_fsmonitor_matcher(
        &self,
        fsmonitor_settings: &FsmonitorSettings,
    ) -> Result<FsmonitorMatcher, SnapshotError> {
        let (watchman_clock, changed_files) = match fsmonitor_settings {
            FsmonitorSettings::None => (None, None),
            FsmonitorSettings::Test { changed_files } => (None, Some(changed_files.clone())),
            #[cfg(feature = "watchman")]
            FsmonitorSettings::Watchman(config) => match self.query_watchman(config) {
                Ok((watchman_clock, changed_files)) => (Some(watchman_clock.into()), changed_files),
                Err(err) => {
                    tracing::warn!(?err, "Failed to query filesystem monitor");
                    (None, None)
                }
            },
            #[cfg(not(feature = "watchman"))]
            FsmonitorSettings::Watchman(_) => {
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
}

struct DirectoryToVisit<'a> {
    dir: RepoPathBuf,
    disk_dir: PathBuf,
    git_ignore: Arc<GitIgnoreFile>,
    file_states: FileStates<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresentDirEntryKind {
    Dir,
    File,
}

#[derive(Clone, Debug)]
struct PresentDirEntries {
    dirs: HashSet<String>,
    files: HashSet<String>,
}

/// Helper to scan local-disk directories and files in parallel.
struct FileSnapshotter<'a> {
    tree_state: &'a TreeState,
    current_tree: &'a MergedTree,
    matcher: &'a dyn Matcher,
    start_tracking_matcher: &'a dyn Matcher,
    tree_entries_tx: Sender<(RepoPathBuf, MergedTreeValue)>,
    file_states_tx: Sender<(RepoPathBuf, FileState)>,
    untracked_paths_tx: Sender<(RepoPathBuf, UntrackedReason)>,
    deleted_files_tx: Sender<RepoPathBuf>,
    error: OnceLock<SnapshotError>,
    progress: Option<&'a SnapshotProgress<'a>>,
    max_new_file_size: u64,
    conflict_marker_style: ConflictMarkerStyle,
}

impl FileSnapshotter<'_> {
    fn spawn_ok<'scope, F>(&'scope self, scope: &rayon::Scope<'scope>, body: F)
    where
        F: FnOnce(&rayon::Scope<'scope>) -> Result<(), SnapshotError> + Send + 'scope,
    {
        scope.spawn(|scope| {
            if self.error.get().is_some() {
                return;
            }
            match body(scope) {
                Ok(()) => {}
                Err(err) => self.error.set(err).unwrap_or(()),
            };
        });
    }

    /// Extracts the result of the snapshot.
    fn into_result(self) -> Result<(), SnapshotError> {
        match self.error.into_inner() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Visits the directory entries, spawns jobs to recurse into sub
    /// directories.
    fn visit_directory<'scope>(
        &'scope self,
        directory_to_visit: DirectoryToVisit<'scope>,
        scope: &rayon::Scope<'scope>,
    ) -> Result<(), SnapshotError> {
        let DirectoryToVisit {
            dir,
            disk_dir,
            git_ignore,
            file_states,
        } = directory_to_visit;

        let git_ignore = git_ignore
            .chain_with_file(&dir.to_internal_dir_string(), disk_dir.join(".gitignore"))?;
        let dir_entries: Vec<_> = disk_dir
            .read_dir()
            .and_then(|entries| entries.try_collect())
            .map_err(|err| SnapshotError::Other {
                message: format!("Failed to read directory {}", disk_dir.display()),
                err: err.into(),
            })?;
        let (dirs, files) = dir_entries
            .into_par_iter()
            // Don't split into too many small jobs. For a small directory,
            // sequential scan should be fast enough.
            .with_min_len(100)
            .filter_map(|entry| {
                self.process_dir_entry(&dir, &git_ignore, file_states, &entry, scope)
                    .transpose()
            })
            .map(|item| match item {
                Ok((PresentDirEntryKind::Dir, name)) => Ok(Either::Left(name)),
                Ok((PresentDirEntryKind::File, name)) => Ok(Either::Right(name)),
                Err(err) => Err(err),
            })
            .collect::<Result<_, _>>()?;
        let present_entries = PresentDirEntries { dirs, files };
        self.emit_deleted_files(&dir, file_states, &present_entries);
        Ok(())
    }

    fn process_dir_entry<'scope>(
        &'scope self,
        dir: &RepoPath,
        git_ignore: &Arc<GitIgnoreFile>,
        file_states: FileStates<'scope>,
        entry: &DirEntry,
        scope: &rayon::Scope<'scope>,
    ) -> Result<Option<(PresentDirEntryKind, String)>, SnapshotError> {
        let file_type = entry.file_type().unwrap();
        let file_name = entry.file_name();
        let name_string = file_name
            .into_string()
            .map_err(|path| SnapshotError::InvalidUtf8Path { path })?;

        if RESERVED_DIR_NAMES.contains(&name_string.as_str()) {
            return Ok(None);
        }
        let name = RepoPathComponent::new(&name_string);
        let path = dir.join(name);
        let maybe_current_file_state = file_states.get_at(dir, name);
        if let Some(file_state) = &maybe_current_file_state {
            if file_state.file_type == FileType::GitSubmodule {
                return Ok(None);
            }
        }

        if file_type.is_dir() {
            let file_states = file_states.prefixed_at(dir, name);
            if git_ignore.matches(&path.to_internal_dir_string())
                || self.start_tracking_matcher.visit(&path).is_nothing()
            {
                // TODO: Report this directory to the caller if there are unignored paths we
                // should not start tracking.

                // If the whole directory is ignored, visit only paths we're already
                // tracking.
                self.spawn_ok(scope, move |_| self.visit_tracked_files(file_states));
            } else if !self.matcher.visit(&path).is_nothing() {
                let directory_to_visit = DirectoryToVisit {
                    dir: path,
                    disk_dir: entry.path(),
                    git_ignore: git_ignore.clone(),
                    file_states,
                };
                self.spawn_ok(scope, |scope| {
                    self.visit_directory(directory_to_visit, scope)
                });
            }
            // Whether or not the directory path matches, any child file entries
            // shouldn't be touched within the current recursion step.
            Ok(Some((PresentDirEntryKind::Dir, name_string)))
        } else if self.matcher.matches(&path) {
            if let Some(progress) = self.progress {
                progress(&path);
            }
            if maybe_current_file_state.is_none()
                && git_ignore.matches(path.as_internal_file_string())
            {
                // If it wasn't already tracked and it matches
                // the ignored paths, then ignore it.
                Ok(None)
            } else if maybe_current_file_state.is_none()
                && !self.start_tracking_matcher.matches(&path)
            {
                // Leave the file untracked
                // TODO: Report this path to the caller
                Ok(None)
            } else {
                let metadata = entry.metadata().map_err(|err| SnapshotError::Other {
                    message: format!("Failed to stat file {}", entry.path().display()),
                    err: err.into(),
                })?;
                if maybe_current_file_state.is_none() && metadata.len() > self.max_new_file_size {
                    // Leave the large file untracked
                    let reason = UntrackedReason::FileTooLarge {
                        size: metadata.len(),
                        max_size: self.max_new_file_size,
                    };
                    self.untracked_paths_tx.send((path, reason)).ok();
                    Ok(None)
                } else if let Some(new_file_state) = file_state(&metadata) {
                    self.process_present_file(
                        path,
                        &entry.path(),
                        maybe_current_file_state.as_ref(),
                        new_file_state,
                    )?;
                    Ok(Some((PresentDirEntryKind::File, name_string)))
                } else {
                    // Special file is not considered present
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Visits only paths we're already tracking.
    fn visit_tracked_files(&self, file_states: FileStates<'_>) -> Result<(), SnapshotError> {
        for (tracked_path, current_file_state) in file_states {
            if !self.matcher.matches(tracked_path) {
                continue;
            }
            let disk_path = tracked_path.to_fs_path(&self.tree_state.working_copy_path)?;
            let metadata = match disk_path.symlink_metadata() {
                Ok(metadata) => Some(metadata),
                Err(err) if err.kind() == io::ErrorKind::NotFound => None,
                Err(err) => {
                    return Err(SnapshotError::Other {
                        message: format!("Failed to stat file {}", disk_path.display()),
                        err: err.into(),
                    });
                }
            };
            if let Some(new_file_state) = metadata.as_ref().and_then(file_state) {
                self.process_present_file(
                    tracked_path.to_owned(),
                    &disk_path,
                    Some(&current_file_state),
                    new_file_state,
                )?;
            } else {
                self.deleted_files_tx.send(tracked_path.to_owned()).ok();
            }
        }
        Ok(())
    }

    fn process_present_file(
        &self,
        path: RepoPathBuf,
        disk_path: &Path,
        maybe_current_file_state: Option<&FileState>,
        mut new_file_state: FileState,
    ) -> Result<(), SnapshotError> {
        let update = self.get_updated_tree_value(
            &path,
            disk_path,
            maybe_current_file_state,
            &new_file_state,
        )?;
        // Preserve materialized conflict data for normal, non-resolved files
        if matches!(new_file_state.file_type, FileType::Normal { .. })
            && !update.as_ref().is_some_and(|update| update.is_resolved())
        {
            new_file_state.materialized_conflict_data =
                maybe_current_file_state.and_then(|state| state.materialized_conflict_data);
        }
        if let Some(tree_value) = update {
            self.tree_entries_tx.send((path.clone(), tree_value)).ok();
        }
        if Some(&new_file_state) != maybe_current_file_state {
            self.file_states_tx.send((path, new_file_state)).ok();
        }
        Ok(())
    }

    /// Emits file paths that don't exist in the `present_entries`.
    fn emit_deleted_files(
        &self,
        dir: &RepoPath,
        file_states: FileStates<'_>,
        present_entries: &PresentDirEntries,
    ) {
        let file_state_chunks = file_states.iter().chunk_by(|(path, _state)| {
            // Extract <name> from <dir>, <dir>/<name>, or <dir>/<name>/**.
            // (file_states may contain <dir> file on file->dir transition.)
            debug_assert!(path.starts_with(dir));
            let slash = !dir.is_root() as usize;
            let len = dir.as_internal_file_string().len() + slash;
            let tail = path.as_internal_file_string().get(len..).unwrap_or("");
            match tail.split_once('/') {
                Some((name, _)) => (PresentDirEntryKind::Dir, name),
                None => (PresentDirEntryKind::File, tail),
            }
        });
        file_state_chunks
            .into_iter()
            .filter(|&((kind, name), _)| match kind {
                PresentDirEntryKind::Dir => !present_entries.dirs.contains(name),
                PresentDirEntryKind::File => !present_entries.files.contains(name),
            })
            .flat_map(|(_, chunk)| chunk)
            // Whether or not the entry exists, submodule should be ignored
            .filter(|(_, state)| state.file_type != FileType::GitSubmodule)
            .filter(|(path, _)| self.matcher.matches(path))
            .try_for_each(|(path, _)| self.deleted_files_tx.send(path.to_owned()))
            .ok();
    }

    fn get_updated_tree_value(
        &self,
        repo_path: &RepoPath,
        disk_path: &Path,
        maybe_current_file_state: Option<&FileState>,
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
                new_file_state.is_clean(current_file_state)
                    && current_file_state.mtime < self.tree_state.own_mtime
            }
        };
        if clean {
            Ok(None)
        } else {
            let current_tree_values = self.current_tree.path_value(repo_path)?;
            let new_file_type = if !self.tree_state.symlink_support {
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
                FileType::Normal { executable } => self
                    .write_path_to_store(
                        repo_path,
                        disk_path,
                        &current_tree_values,
                        executable,
                        maybe_current_file_state.and_then(|state| state.materialized_conflict_data),
                    )
                    .block_on()?,
                FileType::Symlink => {
                    let id = self
                        .write_symlink_to_store(repo_path, disk_path)
                        .block_on()?;
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

    fn store(&self) -> &Store {
        &self.tree_state.store
    }

    async fn write_path_to_store(
        &self,
        repo_path: &RepoPath,
        disk_path: &Path,
        current_tree_values: &MergedTreeValue,
        executable: FileExecutableFlag,
        materialized_conflict_data: Option<MaterializedConflictData>,
    ) -> Result<MergedTreeValue, SnapshotError> {
        if let Some(current_tree_value) = current_tree_values.as_resolved() {
            #[cfg(unix)]
            let _ = current_tree_value; // use the variable
            let id = self.write_file_to_store(repo_path, disk_path).await?;
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
            // If the file contained a conflict before and is a normal file on
            // disk, we try to parse any conflict markers in the file into a
            // conflict.
            let content = fs::read(disk_path).map_err(|err| SnapshotError::Other {
                message: format!("Failed to open file {}", disk_path.display()),
                err: err.into(),
            })?;
            let new_file_ids = conflicts::update_from_content(
                &old_file_ids,
                self.store(),
                repo_path,
                &content,
                self.conflict_marker_style,
                materialized_conflict_data.map_or(MIN_CONFLICT_MARKER_LEN, |data| {
                    data.conflict_marker_len as usize
                }),
            )
            .block_on()?;
            match new_file_ids.into_resolved() {
                Ok(file_id) => {
                    // On Windows, we preserve the executable bit from the merged trees.
                    #[cfg(windows)]
                    let executable = {
                        let () = executable; // use the variable
                        if let Some(merge) = current_tree_values.to_executable_merge() {
                            merge.resolve_trivial().copied().unwrap_or_default()
                        } else {
                            false
                        }
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

    async fn write_file_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<FileId, SnapshotError> {
        let mut file = File::open(disk_path).map_err(|err| SnapshotError::Other {
            message: format!("Failed to open file {}", disk_path.display()),
            err: err.into(),
        })?;
        Ok(self.store().write_file(path, &mut file).await?)
    }

    async fn write_symlink_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<SymlinkId, SnapshotError> {
        if self.tree_state.symlink_support {
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
            Ok(self.store().write_symlink(path, str_target).await?)
        } else {
            let target = fs::read(disk_path).map_err(|err| SnapshotError::Other {
                message: format!("Failed to read file {}", disk_path.display()),
                err: err.into(),
            })?;
            let string_target =
                String::from_utf8(target).map_err(|_| SnapshotError::InvalidUtf8SymlinkTarget {
                    path: disk_path.to_path_buf(),
                })?;
            Ok(self.store().write_symlink(path, &string_target).await?)
        }
    }
}

/// Functions to update local-disk files from the store.
impl TreeState {
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
        let size = io::copy(contents, &mut file).map_err(|err| CheckoutError::Other {
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
        Ok(FileState::for_file(executable, size, &metadata, None))
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
        executable: bool,
        materialized_conflict_data: Option<MaterializedConflictData>,
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
        self.set_executable(disk_path, executable)?;
        let metadata = file
            .metadata()
            .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        Ok(FileState::for_file(
            executable,
            size,
            &metadata,
            materialized_conflict_data,
        ))
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

    pub fn check_out(
        &mut self,
        new_tree: &MergedTree,
        options: &CheckoutOptions,
    ) -> Result<CheckoutStats, CheckoutError> {
        let old_tree = self.current_tree().map_err(|err| match err {
            err @ BackendError::ObjectNotFound { .. } => CheckoutError::SourceNotFound {
                source: Box::new(err),
            },
            other => CheckoutError::InternalBackendError(other),
        })?;
        let stats = self
            .update(
                &old_tree,
                new_tree,
                self.sparse_matcher().as_ref(),
                options.conflict_marker_style,
            )
            .block_on()?;
        self.tree_id = new_tree.id();
        Ok(stats)
    }

    pub fn set_sparse_patterns(
        &mut self,
        sparse_patterns: Vec<RepoPathBuf>,
        options: &CheckoutOptions,
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
        let empty_tree = MergedTree::resolved(Tree::empty(self.store.clone(), RepoPathBuf::root()));
        let added_stats = self
            .update(
                &empty_tree,
                &tree,
                &added_matcher,
                options.conflict_marker_style,
            )
            .block_on()?;
        let removed_stats = self
            .update(
                &tree,
                &empty_tree,
                &removed_matcher,
                options.conflict_marker_style,
            )
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
        conflict_marker_style: ConflictMarkerStyle,
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
        let mut diff_stream = old_tree
            .diff_stream(new_tree, matcher)
            .map(|TreeDiffEntry { path, values }| async {
                match values {
                    Ok((before, after)) => {
                        let result = materialize_tree_value(&self.store, &path, after).await;
                        (path, result.map(|value| (before, value)))
                    }
                    Err(err) => (path, Err(err)),
                }
            })
            .buffered(self.store.concurrency().max(1));
        while let Some((path, data)) = diff_stream.next().await {
            let (before, after) = data?;
            if after.is_absent() {
                stats.removed_files += 1;
            } else if before.is_absent() {
                stats.added_files += 1;
            } else {
                stats.updated_files += 1;
            }

            // Existing Git submodule can be a non-empty directory on disk. We
            // shouldn't attempt to manage it as a tracked path.
            //
            // TODO: It might be better to add general support for paths not
            // tracked by jj than processing submodules specially. For example,
            // paths excluded by .gitignore can be marked as such so that
            // newly-"unignored" paths won't be snapshotted automatically.
            if matches!(before.as_normal(), Some(TreeValue::GitSubmodule(_)))
                && matches!(after, MaterializedTreeValue::GitSubmodule(_))
            {
                eprintln!("ignoring git submodule at {path:?}");
                // Not updating the file state as if there were no diffs. Leave
                // the state type as FileType::GitSubmodule if it was before.
                continue;
            }

            // Create parent directories no matter if after.is_present(). This
            // ensures that the path never traverses symlinks.
            let Some(disk_path) = create_parent_dirs(&self.working_copy_path, &path)? else {
                changed_file_states.push((path, FileState::placeholder()));
                stats.skipped_files += 1;
                continue;
            };
            // If the path was present, check reserved path first and delete it.
            let present_file_deleted = before.is_present() && remove_old_file(&disk_path)?;
            // If not, create temporary file to test the path validity.
            if !present_file_deleted && !can_create_new_file(&disk_path)? {
                changed_file_states.push((path, FileState::placeholder()));
                stats.skipped_files += 1;
                continue;
            }

            // TODO: Check that the file has not changed before overwriting/removing it.
            let file_state = match after {
                MaterializedTreeValue::Absent | MaterializedTreeValue::AccessDenied(_) => {
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
                    eprintln!("ignoring git submodule at {path:?}");
                    FileState::for_gitsubmodule()
                }
                MaterializedTreeValue::Tree(_) => {
                    panic!("unexpected tree entry in diff at {path:?}");
                }
                MaterializedTreeValue::FileConflict {
                    id: _,
                    contents,
                    executable,
                } => {
                    let conflict_marker_len = choose_materialized_conflict_marker_len(&contents);
                    let data = materialize_merge_result_to_bytes_with_marker_len(
                        &contents,
                        conflict_marker_style,
                        conflict_marker_len,
                    )
                    .into();
                    let materialized_conflict_data = MaterializedConflictData {
                        conflict_marker_len: conflict_marker_len.try_into().unwrap_or(u32::MAX),
                    };
                    self.write_conflict(
                        &disk_path,
                        data,
                        executable,
                        Some(materialized_conflict_data),
                    )?
                }
                MaterializedTreeValue::OtherConflict { id } => {
                    // Unless all terms are regular files, we can't do much
                    // better than trying to describe the merge.
                    let data = id.describe().into_bytes();
                    let executable = false;
                    self.write_conflict(&disk_path, data, executable, None)?
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
        while let Some(TreeDiffEntry { path, values }) = diff_stream.next().await {
            let (_before, after) = values?;
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
                            eprintln!("ignoring git submodule at {path:?}");
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
                    materialized_conflict_data: None,
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

fn checkout_error_for_stat_error(err: io::Error, path: &Path) -> CheckoutError {
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
        let lock = FileLock::lock(lock_path).map_err(|err| WorkingCopyStateError {
            message: "Failed to lock working copy".to_owned(),
            err: err.into(),
        })?;

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
            new_workspace_id: None,
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
        config: &WatchmanConfig,
    ) -> Result<(watchman::Clock, Option<Vec<PathBuf>>), WorkingCopyStateError> {
        self.tree_state()?
            .query_watchman(config)
            .map_err(|err| WorkingCopyStateError {
                message: "Failed to query watchman".to_string(),
                err: err.into(),
            })
    }

    #[cfg(feature = "watchman")]
    pub fn is_watchman_trigger_registered(
        &self,
        config: &WatchmanConfig,
    ) -> Result<bool, WorkingCopyStateError> {
        self.tree_state()?
            .is_watchman_trigger_registered(config)
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
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError> {
        Ok(Box::new(LocalWorkingCopy::load(
            store,
            working_copy_path,
            state_path,
        )))
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
    new_workspace_id: Option<WorkspaceId>,
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

    fn snapshot(
        &mut self,
        options: &SnapshotOptions,
    ) -> Result<(MergedTreeId, SnapshotStats), SnapshotError> {
        let tree_state = self
            .wc
            .tree_state_mut()
            .map_err(|err| SnapshotError::Other {
                message: "Failed to read the working copy state".to_string(),
                err: err.into(),
            })?;
        let (is_dirty, stats) = tree_state.snapshot(options)?;
        self.tree_state_dirty |= is_dirty;
        Ok((tree_state.current_tree_id().clone(), stats))
    }

    fn check_out(
        &mut self,
        commit: &Commit,
        options: &CheckoutOptions,
    ) -> Result<CheckoutStats, CheckoutError> {
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
            .check_out(&new_tree, options)?;
        self.tree_state_dirty = true;
        Ok(stats)
    }

    fn rename_workspace(&mut self, new_workspace_id: WorkspaceId) {
        self.new_workspace_id = Some(new_workspace_id);
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
        options: &CheckoutOptions,
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
            .set_sparse_patterns(new_sparse_patterns, options)?;
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
        if self.old_operation_id != operation_id || self.new_workspace_id.is_some() {
            if let Some(new_workspace_id) = self.new_workspace_id {
                self.wc.checkout_state_mut().workspace_id = new_workspace_id;
            }
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
            materialized_conflict_data: None,
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
        let mut file_states = FileStatesMap::from_proto(data, false);

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
            materialized_conflict_data: None,
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

    #[test]
    fn test_file_states_lookup_at() {
        let new_state = |size| FileState {
            file_type: FileType::Normal {
                executable: FileExecutableFlag::default(),
            },
            mtime: MillisSinceEpoch(0),
            size,
            materialized_conflict_data: None,
        };
        let new_proto_entry = |path: &str, size| {
            file_state_entry_to_proto(repo_path(path).to_owned(), &new_state(size))
        };
        let data = vec![
            new_proto_entry("b/c", 0),
            new_proto_entry("b/d/e", 1),
            new_proto_entry("b/d#", 2), // '#' < '/'
            new_proto_entry("b/e", 3),
            new_proto_entry("b#", 4), // '#' < '/'
        ];
        let file_states = FileStates::from_sorted(&data);

        // At root
        assert_eq!(
            file_states.get_at(RepoPath::root(), RepoPathComponent::new("b")),
            None
        );
        assert_eq!(
            file_states.get_at(RepoPath::root(), RepoPathComponent::new("b#")),
            Some(new_state(4))
        );

        // At prefixed dir
        let prefixed_states =
            file_states.prefixed_at(RepoPath::root(), RepoPathComponent::new("b"));
        assert_eq!(
            prefixed_states.paths().collect_vec(),
            ["b/c", "b/d/e", "b/d#", "b/e"].map(repo_path)
        );
        assert_eq!(
            prefixed_states.get_at(repo_path("b"), RepoPathComponent::new("c")),
            Some(new_state(0))
        );
        assert_eq!(
            prefixed_states.get_at(repo_path("b"), RepoPathComponent::new("d")),
            None
        );
        assert_eq!(
            prefixed_states.get_at(repo_path("b"), RepoPathComponent::new("d#")),
            Some(new_state(2))
        );

        // At nested prefixed dir
        let prefixed_states =
            prefixed_states.prefixed_at(repo_path("b"), RepoPathComponent::new("d"));
        assert_eq!(
            prefixed_states.paths().collect_vec(),
            ["b/d/e"].map(repo_path)
        );
        assert_eq!(
            prefixed_states.get_at(repo_path("b/d"), RepoPathComponent::new("e")),
            Some(new_state(1))
        );
        assert_eq!(
            prefixed_states.get_at(repo_path("b/d"), RepoPathComponent::new("#")),
            None
        );

        // At prefixed file
        let prefixed_states =
            file_states.prefixed_at(RepoPath::root(), RepoPathComponent::new("b#"));
        assert_eq!(prefixed_states.paths().collect_vec(), ["b#"].map(repo_path));
        assert_eq!(
            prefixed_states.get_at(repo_path("b#"), RepoPathComponent::new("#")),
            None
        );
    }
}
