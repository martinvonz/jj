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
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::fs::Metadata;
use std::fs::OpenOptions;
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
use std::time::UNIX_EPOCH;

use futures::StreamExt;
use itertools::EitherOrBoth;
use itertools::Itertools;
use once_cell::unsync::OnceCell;
use pollster::FutureExt;
use prost::Message;
use rayon::iter::IntoParallelIterator;
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
use crate::conflicts::materialize_merge_result;
use crate::conflicts::materialize_tree_value;
use crate::conflicts::MaterializedTreeValue;
#[cfg(unix)]
use crate::file_util::check_executable_bit_support;
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
use crate::settings::ignore_executable_bit;
use crate::settings::HumanByteSize;
use crate::settings::UserSettings;
use crate::store::Store;
use crate::tree::Tree;
use crate::working_copy::CheckoutError;
use crate::working_copy::CheckoutStats;
use crate::working_copy::LockedWorkingCopy;
use crate::working_copy::ResetError;
use crate::working_copy::SnapshotError;
use crate::working_copy::SnapshotOptions;
use crate::working_copy::SnapshotProgress;
use crate::working_copy::WorkingCopy;
use crate::working_copy::WorkingCopyFactory;
use crate::working_copy::WorkingCopyStateError;

/// The executable bit for a filetype, potentially ignored.
///
/// On Windows there is no executable bit, so this will always be Ignore. On
/// Unix it will usually be Exec(true|false), but may be ignored by a config
/// value or by a check we run when we load the config if not specified.
#[derive(Debug, Clone, Copy)]
pub enum ExecFlag {
    Exec(bool),
    Ignore,
}
// Note: cannot derive Eq or PartialEq since a == b == c does not imply a == c.
// e.g. `Exec(true) == Ignore == Exec(false)` but `Exec(true) != Exec(false)`

impl ExecFlag {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (ExecFlag::Exec(a), ExecFlag::Exec(b)) => a == b,
            // Always treat as equal if either is Ignore.
            _ => true,
        }
    }

    /// Create a bool in an environment where we can't check a IgnoreExec value.
    fn from_bool_unchecked(executable: bool) -> Self {
        if cfg!(unix) {
            ExecFlag::Exec(executable)
        } else {
            ExecFlag::Ignore
        }
    }
}

/// Whether to ignore the executable bit when comparing files. The executable
/// state is always ignored on Windows, but is respected by default on Unix and
/// is ignored if we find that the filesystem doesn't support it or by user
/// configuration.
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct IgnoreExec(bool);
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct IgnoreExec;

impl IgnoreExec {
    /// Load from user settings. If the setting is not given on Unix, then we
    /// check whether executable bits are supported in the working copy's
    /// filesystem and return true or false accordingly.
    fn load_config(exec_config: Option<bool>, wc_path: &PathBuf) -> Self {
        #[cfg(unix)] // check for executable support on Unix.
        let ignore_exec =
            IgnoreExec(exec_config.unwrap_or_else(|| !check_executable_bit_support(wc_path)));
        #[cfg(windows)]
        let (ignore_exec, _, _) = (IgnoreExec, exec_config, wc_path); // use the variables
        ignore_exec
    }

    /// Push into an Option<bool> config value for roundtripping.
    fn as_config(self) -> Option<bool> {
        #[cfg(unix)]
        let exec_config = Some(self.0);
        #[cfg(windows)]
        let (exec_config, _) = (None, self); // use the variable
        exec_config
    }

    /// Resolve an executable bit into a flag for the FileType, potentially
    /// ignoring it.
    fn into_flag<F: Fn() -> bool>(self, is_executable: F) -> ExecFlag {
        #[cfg(unix)]
        let exec_flag = if self.0 {
            ExecFlag::Ignore
        } else {
            ExecFlag::Exec(is_executable())
        };
        #[cfg(windows)]
        let (exec_flag, _, _) = (ExecFlag::Ignore, self, is_executable); // use the variables
        exec_flag
    }

    /// Convert a flag into the executable bit to write with a closure for a
    /// default value.
    fn exec_bit_to_write<F: Fn() -> bool>(self, exec_flag: ExecFlag, default: F) -> bool {
        #[cfg(unix)]
        let executable = match (self.0, exec_flag) {
            (false, ExecFlag::Exec(executable)) => executable,
            (true | false, _) => default(),
        };
        #[cfg(windows)]
        let (executable, _, _) = (default(), self, exec_flag); // use the variables
        executable
    }
}

#[derive(Debug, Clone)]
pub enum FileType {
    Normal { exec_flag: ExecFlag },
    Symlink,
    GitSubmodule,
}

impl Default for FileType {
    fn default() -> Self {
        FileType::Normal {
            exec_flag: ExecFlag::Exec(false),
        }
    }
}

#[derive(Debug, Clone)]
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
        FileState {
            file_type: FileType::default(),
            mtime: MillisSinceEpoch(0),
            size: 0,
        }
    }

    /// Filestate for a normal file..
    fn for_file(size: u64, metadata: &Metadata, exec_flag: ExecFlag) -> Self {
        FileState {
            file_type: FileType::Normal { exec_flag },
            mtime: mtime_from_metadata(metadata),
            size,
        }
    }

    /// Whether this file state is compatible with another file state. The extra
    /// complexity here comes from executable flags which always match on
    /// Windows and might always match on Unix if ignore_exec is true.
    fn matches(&self, other: &Self) -> bool {
        use FileType::*;
        let file_types_match = match (&self.file_type, &other.file_type) {
            (GitSubmodule, GitSubmodule) | (Symlink, Symlink) => true,
            (Normal { exec_flag: lhs }, Normal { exec_flag: rhs }) => lhs.matches(rhs),
            _ => false,
        };
        file_types_match && self.mtime == other.mtime && self.size == other.size
    }

    /// Inverse of `self.matches(other)`.
    fn differs(&self, other: &Self) -> bool {
        !self.matches(other)
    }

    /// Filestate for a symlink.
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

    /// Filestate for a git submodule.
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
    ignore_exec: IgnoreExec,

    /// The most recent clock value returned by Watchman. Will only be set if
    /// the repo is configured to use the Watchman filesystem monitor and
    /// Watchman has been queried at least once.
    watchman_clock: Option<crate::protos::working_copy::WatchmanClock>,
}

fn file_state_from_proto(proto: &crate::protos::working_copy::FileState) -> FileState {
    let file_type = match proto.file_type() {
        crate::protos::working_copy::FileType::Normal => FileType::Normal {
            exec_flag: ExecFlag::from_bool_unchecked(false),
        },
        // can exist for Windows in files written by older versions of jj
        crate::protos::working_copy::FileType::Executable => FileType::Normal {
            exec_flag: ExecFlag::from_bool_unchecked(true),
        },
        crate::protos::working_copy::FileType::Symlink => FileType::Symlink,
        crate::protos::working_copy::FileType::Conflict => FileType::default(),
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
        FileType::Normal {
            exec_flag: ExecFlag::Exec(true),
        } => crate::protos::working_copy::FileType::Executable,
        FileType::Normal { exec_flag: _ } => crate::protos::working_copy::FileType::Normal,
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

fn file_state(metadata: &Metadata, ignore_exec: IgnoreExec) -> Option<FileState> {
    let metadata_file_type = metadata.file_type();
    let file_type = if metadata_file_type.is_dir() {
        None
    } else if metadata_file_type.is_symlink() {
        Some(FileType::Symlink)
    } else if metadata_file_type.is_file() {
        #[cfg(unix)]
        let exec_flag = ignore_exec.into_flag(|| metadata.permissions().mode() & 0o111 != 0);
        #[cfg(windows)]
        let exec_flag = ExecFlag::Ignore;
        Some(FileType::Normal { exec_flag })
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

    /// Initialize an empty tree state and save it to the filesystem.
    pub fn init(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        exec_config: Option<bool>,
    ) -> Result<TreeState, TreeStateError> {
        let mut wc = TreeState::empty(store, working_copy_path, state_path, exec_config);
        wc.save()?;
        Ok(wc)
    }

    /// Create a new empty tree state for this working copy path.
    fn empty(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        exec_config: Option<bool>,
    ) -> TreeState {
        let tree_id = store.empty_merged_tree_id();
        let ignore_exec = IgnoreExec::load_config(exec_config, &working_copy_path);
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
            ignore_exec,
        }
    }

    /// Load an existing tree state if present, or initialize an empty one.
    pub fn load(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        exec_config: Option<bool>,
    ) -> Result<TreeState, TreeStateError> {
        let tree_state_path = state_path.join("tree_state");
        let file = match File::open(&tree_state_path) {
            Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
                return TreeState::init(store, working_copy_path, state_path, exec_config);
            }
            Err(err) => {
                return Err(TreeStateError::ReadTreeState {
                    path: tree_state_path,
                    source: err,
                });
            }
            Ok(file) => file,
        };

        let mut wc = TreeState::empty(store, working_copy_path, state_path, exec_config);
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

    /// Load the tree's data from the filesystem.
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

    /// Save the tree's data to the filesystem.
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

    async fn write_file_to_store(
        &self,
        path: &RepoPath,
        disk_path: &Path,
    ) -> Result<FileId, SnapshotError> {
        let mut file = File::open(disk_path).map_err(|err| SnapshotError::Other {
            message: format!("Failed to open file {}", disk_path.display()),
            err: err.into(),
        })?;
        Ok(self.store.write_file(path, &mut file).await?)
    }

    async fn write_symlink_to_store(
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
            Ok(self.store.write_symlink(path, str_target).await?)
        } else {
            let target = fs::read(disk_path).map_err(|err| SnapshotError::Other {
                message: format!("Failed to read file {}", disk_path.display()),
                err: err.into(),
            })?;
            let string_target =
                String::from_utf8(target).map_err(|_| SnapshotError::InvalidUtf8SymlinkTarget {
                    path: disk_path.to_path_buf(),
                })?;
            Ok(self.store.write_symlink(path, &string_target).await?)
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

    /// Look for changes to the working copy. If there are any changes, create
    /// a new tree from it and return it, and also update the dirstate on disk.
    #[instrument(skip_all)]
    pub fn snapshot(&mut self, options: &SnapshotOptions) -> Result<bool, SnapshotError> {
        let SnapshotOptions {
            base_ignores,
            fsmonitor_settings,
            progress,
            start_tracking_matcher,
            max_new_file_size,
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
                git_ignore: base_ignores.clone(),
                file_states: self.file_states.all(),
            };
            self.visit_directory(
                &matcher,
                start_tracking_matcher,
                &current_tree,
                tree_entries_tx,
                file_states_tx,
                present_files_tx,
                directory_to_visit,
                *progress,
                *max_new_file_size,
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
                        fsmonitor_matcher.matches(path)
                            && !matches!(state.file_type, FileType::GitSubmodule)
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
                .filter_map(|(path, result)| result.is_ok().then_some(path))
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
        start_tracking_matcher: &dyn Matcher,
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
                    if matches!(file_state.file_type, FileType::GitSubmodule) {
                        return Ok(());
                    }
                }

                if file_type.is_dir() {
                    let file_states = file_states.prefixed(&path);
                    if git_ignore.matches(&path.to_internal_dir_string())
                        || start_tracking_matcher.visit(&path).is_nothing()
                    {
                        // TODO: Report this directory to the caller if there are unignored paths we
                        // should not start tracking.

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
                            if let Some(new_file_state) = file_state(&metadata, self.ignore_exec) {
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
                                if new_file_state.differs(&current_file_state) {
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
                            start_tracking_matcher,
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
                        // the ignored paths, then ignore it.
                    } else if maybe_current_file_state.is_none()
                        && !start_tracking_matcher.matches(&path)
                    {
                        // Leave the file untracked
                        // TODO: Report this path to the caller
                    } else {
                        let metadata = entry.metadata().map_err(|err| SnapshotError::Other {
                            message: format!("Failed to stat file {}", entry.path().display()),
                            err: err.into(),
                        })?;
                        if maybe_current_file_state.is_none() && metadata.len() > max_new_file_size
                        {
                            // TODO: Maybe leave the file untracked instead
                            return Err(SnapshotError::NewFileTooLarge {
                                path: entry.path().clone(),
                                size: HumanByteSize(metadata.len()),
                                max_size: HumanByteSize(max_new_file_size),
                            });
                        }
                        if let Some(new_file_state) = file_state(&metadata, self.ignore_exec) {
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
                            if maybe_current_file_state
                                .map(|fs| new_file_state.differs(&fs))
                                .unwrap_or(true)
                            {
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
                current_file_state.matches(new_file_state)
                    && current_file_state.mtime < self.own_mtime
            }
        };
        if clean {
            Ok(None)
        } else {
            let current_tree_values = current_tree.path_value(repo_path)?;
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
                FileType::Normal { exec_flag } => self
                    .write_path_to_store(repo_path, &disk_path, &current_tree_values, exec_flag)
                    .block_on()?,
                FileType::Symlink => {
                    let id = self
                        .write_symlink_to_store(repo_path, &disk_path)
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

    async fn write_path_to_store(
        &self,
        repo_path: &RepoPath,
        disk_path: &Path,
        current_tree_values: &MergedTreeValue,
        exec_flag: ExecFlag,
    ) -> Result<MergedTreeValue, SnapshotError> {
        // If the file contained a conflict before and is now a normal file on disk, we
        // try to parse any conflict markers in the file into a conflict.
        if let Some(current_tree_value) = current_tree_values.as_resolved() {
            let id = self.write_file_to_store(repo_path, disk_path).await?;
            // Use the given executable bit or return the current bit.
            let executable =
                self.ignore_exec
                    .exec_bit_to_write(exec_flag, || match current_tree_value {
                        Some(TreeValue::File { id: _, executable }) => *executable,
                        _ => false,
                    });
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
                    // Use the given executable bit or preserve the executable
                    // bit from the merged trees.
                    let executable = self.ignore_exec.exec_bit_to_write(exec_flag, || {
                        if let Some(merge) = current_tree_values.to_executable_merge() {
                            merge.resolve_trivial().copied().unwrap_or(false)
                        } else {
                            false
                        }
                    });
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
        let exec_flag = self.set_executable_get_flag(disk_path, executable)?;
        // Read the file state from the file descriptor. That way, know that the file
        // exists and is of the expected type, and the stat information is most likely
        // accurate, except for other processes modifying the file concurrently (The
        // mtime is set at write time and won't change when we close the file.)
        let metadata = file
            .metadata()
            .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        Ok(FileState::for_file(size, &metadata, exec_flag))
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
        let exec_flag = self.set_executable_get_flag(disk_path, executable)?;
        let metadata = file
            .metadata()
            .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        Ok(FileState::for_file(size, &metadata, exec_flag))
    }

    /// Maybe set the executable bit and return the flag or an error. This is a
    /// no-op on Windows.
    fn set_executable_get_flag(
        &self,
        disk_path: &Path,
        executable: bool,
    ) -> Result<ExecFlag, CheckoutError> {
        let exec_flag = self.ignore_exec.into_flag(|| executable);
        #[cfg(unix)]
        if let ExecFlag::Exec(executable) = exec_flag {
            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(disk_path, fs::Permissions::from_mode(mode))
                .map_err(|err| checkout_error_for_stat_error(err, disk_path))?;
        };
        Ok(exec_flag)
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
        let empty_tree = MergedTree::resolved(Tree::empty(self.store.clone(), RepoPathBuf::root()));
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
                .map(|TreeDiffEntry { path, values }| async {
                    match values {
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
                    let mut data = vec![];
                    materialize_merge_result(&contents, &mut data)
                        .expect("Failed to materialize conflict to in-memory buffer");
                    self.write_conflict(&disk_path, data, executable)?
                }
                MaterializedTreeValue::OtherConflict { id } => {
                    // Unless all terms are regular files, we can't do much
                    // better than trying to describe the merge.
                    let data = id.describe().into_bytes();
                    let executable = false;
                    self.write_conflict(&disk_path, data, executable)?
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
                        TreeValue::File { id: _, executable } => FileType::Normal {
                            exec_flag: self.ignore_exec.into_flag(|| executable),
                        },
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
                    // TODO: Try to set the executable bit based on the conflict
                    Err(values) => {
                        let mut file_type = FileType::default();
                        for value in values.adds().flatten() {
                            // Use the *last* added filetype from the merge
                            if let TreeValue::File { id: _, executable } = value {
                                let exec_flag = self.ignore_exec.into_flag(|| *executable);
                                file_type = FileType::Normal { exec_flag };
                            }
                        }
                        file_type
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
    ignore_exec: IgnoreExec,
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
            ignore_exec: self.ignore_exec,
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
        settings: &UserSettings,
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
        let tree_state = TreeState::init(
            store.clone(),
            working_copy_path.clone(),
            state_path.clone(),
            ignore_executable_bit(settings.config()),
        )
        .map_err(|err| WorkingCopyStateError {
            message: "Failed to initialize working copy state".to_string(),
            err: err.into(),
        })?;
        let ignore_exec = tree_state.ignore_exec;
        Ok(LocalWorkingCopy {
            store,
            working_copy_path,
            state_path,
            checkout_state: OnceCell::new(),
            tree_state: OnceCell::with_value(tree_state),
            ignore_exec,
        })
    }

    pub fn load(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        settings: &UserSettings,
    ) -> LocalWorkingCopy {
        let exec_config = ignore_executable_bit(settings.config());
        let ignore_exec = IgnoreExec::load_config(exec_config, &working_copy_path);
        LocalWorkingCopy {
            store,
            working_copy_path,
            state_path,
            checkout_state: OnceCell::new(),
            tree_state: OnceCell::new(),
            ignore_exec,
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
                    self.ignore_exec.as_config(),
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
        settings: &UserSettings,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError> {
        Ok(Box::new(LocalWorkingCopy::init(
            store,
            working_copy_path,
            state_path,
            operation_id,
            workspace_id,
            settings,
        )?))
    }

    fn load_working_copy(
        &self,
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        settings: &UserSettings,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError> {
        Ok(Box::new(LocalWorkingCopy::load(
            store,
            working_copy_path,
            state_path,
            settings,
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

    fn snapshot(&mut self, options: &SnapshotOptions) -> Result<MergedTreeId, SnapshotError> {
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

    // Only for convenience in these tests. File states are *not* transitively
    // equal (due to ExecFlag), so we should not implement PartialEq generally.
    impl PartialEq for FileState {
        fn eq(&self, other: &Self) -> bool {
            self.matches(other)
        }
    }

    #[test]
    fn test_file_states_merge() {
        let new_state = |size| FileState {
            file_type: FileType::default(),
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
            file_type: FileType::default(),
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
