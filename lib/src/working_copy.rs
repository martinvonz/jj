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

use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, HashSet};
use std::convert::TryInto;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::ops::Bound;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use protobuf::Message;
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::backend::{
    BackendError, ConflictId, FileId, MillisSinceEpoch, SymlinkId, TreeId, TreeValue,
};
use crate::conflicts::{materialize_conflict, update_conflict_from_content};
use crate::gitignore::GitIgnoreFile;
use crate::lock::FileLock;
use crate::matchers::{EverythingMatcher, Matcher};
use crate::op_store::{OperationId, WorkspaceId};
use crate::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use crate::store::Store;
use crate::tree::{Diff, Tree};
use crate::tree_builder::TreeBuilder;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FileType {
    Normal { executable: bool },
    Symlink,
    Conflict { id: ConflictId },
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
    own_mtime: MillisSinceEpoch,
}

fn file_state_from_proto(proto: &crate::protos::working_copy::FileState) -> FileState {
    let file_type = match proto.file_type {
        crate::protos::working_copy::FileType::Normal => FileType::Normal { executable: false },
        crate::protos::working_copy::FileType::Executable => FileType::Normal { executable: true },
        crate::protos::working_copy::FileType::Symlink => FileType::Symlink,
        crate::protos::working_copy::FileType::Conflict => {
            let id = ConflictId::new(proto.conflict_id.to_vec());
            FileType::Conflict { id }
        }
    };
    FileState {
        file_type,
        mtime: MillisSinceEpoch(proto.mtime_millis_since_epoch),
        size: proto.size,
    }
}

fn file_state_to_proto(file_state: &FileState) -> crate::protos::working_copy::FileState {
    let mut proto = crate::protos::working_copy::FileState::new();
    let file_type = match &file_state.file_type {
        FileType::Normal { executable: false } => crate::protos::working_copy::FileType::Normal,
        FileType::Normal { executable: true } => crate::protos::working_copy::FileType::Executable,
        FileType::Symlink => crate::protos::working_copy::FileType::Symlink,
        FileType::Conflict { id } => {
            proto.conflict_id = id.to_bytes();
            crate::protos::working_copy::FileType::Conflict
        }
    };
    proto.file_type = file_type;
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
        file_states.insert(path, file_state_from_proto(proto_file_state));
    }
    file_states
}

fn create_parent_dirs(disk_path: &Path) {
    fs::create_dir_all(disk_path.parent().unwrap())
        .unwrap_or_else(|_| panic!("failed to create parent directories for {:?}", &disk_path));
}

fn file_state(path: &Path) -> Option<FileState> {
    let metadata = path.symlink_metadata().ok()?;
    let time = metadata.modified().unwrap();
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap();
    let mtime = MillisSinceEpoch(since_epoch.as_millis().try_into().unwrap());
    let size = metadata.len();
    let metadata_file_type = metadata.file_type();
    let file_type = if metadata_file_type.is_dir() {
        panic!("expected file, not directory: {:?}", path);
    } else if metadata_file_type.is_symlink() {
        FileType::Symlink
    } else {
        #[cfg(unix)]
        let mode = metadata.permissions().mode();
        #[cfg(windows)]
        let mode = 0;
        if mode & 0o111 != 0 {
            FileType::Normal { executable: true }
        } else {
            FileType::Normal { executable: false }
        }
    };
    Some(FileState {
        file_type,
        mtime,
        size,
    })
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CheckoutStats {
    pub updated_files: u32,
    pub added_files: u32,
    pub removed_files: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CheckoutError {
    // The current checkout was deleted, maybe by an overly aggressive GC that happened while
    // the current process was running.
    #[error("Current checkout not found")]
    SourceNotFound,
    // Another process checked out a commit while the current process was running (after the
    // working copy was read by the current process).
    #[error("Concurrent checkout")]
    ConcurrentCheckout,
    #[error("Internal error: {0:?}")]
    InternalBackendError(BackendError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResetError {
    // The current checkout was deleted, maybe by an overly aggressive GC that happened while
    // the current process was running.
    #[error("Current checkout not found")]
    SourceNotFound,
    #[error("Internal error: {0:?}")]
    InternalBackendError(BackendError),
}

impl TreeState {
    pub fn current_tree_id(&self) -> &TreeId {
        &self.tree_id
    }

    pub fn file_states(&self) -> &BTreeMap<RepoPath, FileState> {
        &self.file_states
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
            own_mtime: MillisSinceEpoch(0),
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
            let time = metadata.modified().unwrap();
            let since_epoch = time.duration_since(UNIX_EPOCH).unwrap();
            self.own_mtime = MillisSinceEpoch(since_epoch.as_millis().try_into().unwrap());
        } else {
            self.own_mtime = MillisSinceEpoch(0);
        }
    }

    fn read(&mut self, mut file: File) {
        self.update_own_mtime();
        let proto: crate::protos::working_copy::TreeState =
            Message::parse_from_reader(&mut file).unwrap();
        self.tree_id = TreeId::new(proto.tree_id.clone());
        self.file_states = file_states_from_proto(&proto);
    }

    fn save(&mut self) {
        let mut proto = crate::protos::working_copy::TreeState::new();
        proto.tree_id = self.tree_id.to_bytes();
        for (file, file_state) in &self.file_states {
            proto.file_states.insert(
                file.to_internal_file_string(),
                file_state_to_proto(file_state),
            );
        }

        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        proto.write_to_writer(temp_file.as_file_mut()).unwrap();
        // update own write time while we before we rename it, so we know
        // there is no unknown data in it
        self.update_own_mtime();
        // TODO: Retry if persisting fails (it will on Windows if the file happened to
        // be open for read).
        temp_file
            .persist(self.state_path.join("tree_state"))
            .unwrap();
    }

    fn write_file_to_store(&self, path: &RepoPath, disk_path: &Path) -> FileId {
        let file = File::open(disk_path).unwrap();
        self.store.write_file(path, &mut Box::new(file)).unwrap()
    }

    fn write_symlink_to_store(&self, path: &RepoPath, disk_path: &Path) -> SymlinkId {
        let target = disk_path.read_link().unwrap();
        let str_target = target.to_str().unwrap();
        self.store.write_symlink(path, str_target).unwrap()
    }

    // Look for changes to the working copy. If there are any changes, create
    // a new tree from it and return it, and also update the dirstate on disk.
    pub fn write_tree(&mut self, base_ignores: Arc<GitIgnoreFile>) -> TreeId {
        let mut work = vec![(
            RepoPath::root(),
            self.working_copy_path.clone(),
            base_ignores,
        )];
        let mut tree_builder = self.store.tree_builder(self.tree_id.clone());
        let mut deleted_files: HashSet<_> = self.file_states.keys().cloned().collect();
        while !work.is_empty() {
            let (dir, disk_dir, git_ignore) = work.pop().unwrap();
            let git_ignore = git_ignore
                .chain_with_file(&dir.to_internal_dir_string(), disk_dir.join(".gitignore"));
            for maybe_entry in disk_dir.read_dir().unwrap() {
                let entry = maybe_entry.unwrap();
                let file_type = entry.file_type().unwrap();
                let file_name = entry.file_name();
                let name = file_name.to_str().unwrap();
                if name == ".jj" || name == ".git" {
                    continue;
                }
                let sub_path = dir.join(&RepoPathComponent::from(name));
                if file_type.is_dir() {
                    if git_ignore.matches_all_files_in(&sub_path.to_internal_dir_string()) {
                        // If the whole directory is ignored, skip it unless we're already tracking
                        // some file in it. TODO: This is pretty ugly... Also, we should
                        // optimize it to check exactly the already-tracked files (we know that
                        // we won't have to consider new files in the directory).
                        let first_file_in_dir = dir.join(&RepoPathComponent::from("\0"));
                        if let Some((maybe_subdir_file, _)) = self
                            .file_states
                            .range((Bound::Included(&first_file_in_dir), Bound::Unbounded))
                            .next()
                        {
                            if !dir.contains(&maybe_subdir_file.parent().unwrap()) {
                                continue;
                            }
                        }
                    }
                    work.push((sub_path, entry.path(), git_ignore.clone()));
                } else {
                    deleted_files.remove(&sub_path);
                    self.update_file_state(
                        sub_path,
                        entry.path(),
                        git_ignore.as_ref(),
                        &mut tree_builder,
                    );
                }
            }
        }

        for file in &deleted_files {
            self.file_states.remove(file);
            tree_builder.remove(file.clone());
        }
        self.tree_id = tree_builder.write_tree();
        self.tree_id.clone()
    }

    fn update_file_state(
        &mut self,
        repo_path: RepoPath,
        disk_path: PathBuf,
        git_ignore: &GitIgnoreFile,
        tree_builder: &mut TreeBuilder,
    ) {
        let maybe_current_file_state = self.file_states.get_mut(&repo_path);
        if maybe_current_file_state.is_none()
            && git_ignore.matches_file(&repo_path.to_internal_file_string())
        {
            // If it wasn't already tracked and it matches the ignored paths, then
            // ignore it.
            return;
        }
        #[cfg_attr(unix, allow(unused_mut))]
        let mut new_file_state = file_state(&disk_path).unwrap();
        match maybe_current_file_state {
            None => {
                // untracked
                let file_type = new_file_state.file_type.clone();
                self.file_states.insert(repo_path.clone(), new_file_state);
                let file_value = self.write_path_to_store(&repo_path, &disk_path, file_type);
                tree_builder.set(repo_path, file_value);
            }
            Some(current_file_state) => {
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
                let mut clean = current_file_state == &new_file_state;
                // Because the file system doesn't have a built-in way of indicating a conflict,
                // we look at the current state instead. If that indicates that the path has a
                // conflict and the contents are now a file, then we take interpret that as if
                // it is still a conflict.
                if !clean
                    && matches!(current_file_state.file_type, FileType::Conflict { .. })
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
                        if let (FileType::Conflict { id }, FileType::Normal { executable: _ }) =
                            (&current_file_state.file_type, &new_file_state.file_type)
                        {
                            let mut file = File::open(&disk_path).unwrap();
                            let mut content = vec![];
                            file.read_to_end(&mut content).unwrap();
                            if let Some(new_conflict_id) = update_conflict_from_content(
                                self.store.as_ref(),
                                &repo_path,
                                id,
                                &content,
                            )
                            .unwrap()
                            {
                                new_file_state.file_type = FileType::Conflict {
                                    id: new_conflict_id.clone(),
                                };
                                *current_file_state = new_file_state;
                                tree_builder.set(repo_path, TreeValue::Conflict(new_conflict_id));
                                return;
                            }
                        }
                    }
                }
                if !clean {
                    let file_type = new_file_state.file_type.clone();
                    *current_file_state = new_file_state;
                    let file_value = self.write_path_to_store(&repo_path, &disk_path, file_type);
                    tree_builder.set(repo_path, file_value);
                }
            }
        };
    }

    fn write_path_to_store(
        &self,
        repo_path: &RepoPath,
        disk_path: &Path,
        file_type: FileType,
    ) -> TreeValue {
        match file_type {
            FileType::Normal { executable } => {
                let id = self.write_file_to_store(repo_path, disk_path);
                TreeValue::Normal { id, executable }
            }
            FileType::Symlink => {
                let id = self.write_symlink_to_store(repo_path, disk_path);
                TreeValue::Symlink(id)
            }
            FileType::Conflict { .. } => panic!("conflicts should be handled by the caller"),
        }
    }

    fn write_file(
        &self,
        disk_path: &Path,
        path: &RepoPath,
        id: &FileId,
        executable: bool,
    ) -> FileState {
        create_parent_dirs(disk_path);
        // TODO: Check that we're not overwriting an un-ignored file here (which might
        // be created by a concurrent process).
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(disk_path)
            .unwrap_or_else(|err| panic!("failed to open {:?} for write: {:?}", &disk_path, err));
        let mut contents = self.store.read_file(path, id).unwrap();
        std::io::copy(&mut contents, &mut file).unwrap();
        self.set_executable(disk_path, executable);
        // Read the file state while we still have the file open. That way, know that
        // the file exists, and the stat information is most likely accurate,
        // except for other processes modifying the file concurrently (The mtime is set
        // at write time and won't change when we close the file.)
        let mut file_state = file_state(disk_path).unwrap();
        // Make sure the state we record is what we tried to set above. This is mostly
        // for Windows, since the executable bit is not reflected in the file system
        // there.
        file_state.mark_executable(executable);
        file_state
    }

    #[cfg_attr(windows, allow(unused_variables))]
    fn write_symlink(&self, disk_path: &Path, path: &RepoPath, id: &SymlinkId) -> FileState {
        create_parent_dirs(disk_path);
        #[cfg(windows)]
        {
            println!("ignoring symlink at {:?}", path);
        }
        #[cfg(unix)]
        {
            let target = self.store.read_symlink(path, id).unwrap();
            let target = PathBuf::from(&target);
            symlink(target, disk_path).unwrap();
        }
        file_state(disk_path).unwrap()
    }

    fn write_conflict(&self, disk_path: &Path, path: &RepoPath, id: &ConflictId) -> FileState {
        create_parent_dirs(disk_path);
        let conflict = self.store.read_conflict(path, id).unwrap();
        // TODO: Check that we're not overwriting an un-ignored file here (which might
        // be created by a concurrent process).
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(disk_path)
            .unwrap_or_else(|err| panic!("failed to open {:?} for write: {:?}", &disk_path, err));
        materialize_conflict(self.store.as_ref(), path, &conflict, &mut file).unwrap();
        // TODO: Set the executable bit correctly (when possible) and preserve that on
        // Windows like we do with the executable bit for regular files.
        let mut result = file_state(disk_path).unwrap();
        result.file_type = FileType::Conflict { id: id.clone() };
        result
    }

    #[cfg_attr(windows, allow(unused_variables))]
    fn set_executable(&self, disk_path: &Path, executable: bool) {
        #[cfg(windows)]
        {
            return;
        }
        #[cfg(unix)]
        {
            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(disk_path, fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    pub fn check_out(&mut self, new_tree: &Tree) -> Result<CheckoutStats, CheckoutError> {
        let old_tree = self
            .store
            .get_tree(&RepoPath::root(), &self.tree_id)
            .map_err(|err| match err {
                BackendError::NotFound => CheckoutError::SourceNotFound,
                other => CheckoutError::InternalBackendError(other),
            })?;
        let stats = self.update(&old_tree, new_tree, &EverythingMatcher)?;
        self.tree_id = new_tree.id().clone();
        Ok(stats)
    }

    fn update(
        &mut self,
        old_tree: &Tree,
        new_tree: &Tree,
        matcher: &dyn Matcher,
    ) -> Result<CheckoutStats, CheckoutError> {
        let mut stats = CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 0,
        };

        for (path, diff) in old_tree.diff(new_tree, matcher) {
            let disk_path = path.to_fs_path(&self.working_copy_path);

            // TODO: Check that the file has not changed before overwriting/removing it.
            match diff {
                Diff::Removed(_before) => {
                    fs::remove_file(&disk_path).ok();
                    let mut parent_dir = disk_path.parent().unwrap();
                    loop {
                        if fs::remove_dir(&parent_dir).is_err() {
                            break;
                        }
                        parent_dir = parent_dir.parent().unwrap();
                    }
                    self.file_states.remove(&path);
                    stats.removed_files += 1;
                }
                Diff::Added(after) => {
                    let file_state = match after {
                        TreeValue::Normal { id, executable } => {
                            self.write_file(&disk_path, &path, &id, executable)
                        }
                        TreeValue::Symlink(id) => self.write_symlink(&disk_path, &path, &id),
                        TreeValue::Conflict(id) => self.write_conflict(&disk_path, &path, &id),
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {:?}", path);
                            continue;
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {:?}", path);
                        }
                    };
                    self.file_states.insert(path.clone(), file_state);
                    stats.added_files += 1;
                }
                Diff::Modified(
                    TreeValue::Normal {
                        id: old_id,
                        executable: old_executable,
                    },
                    TreeValue::Normal { id, executable },
                ) if id == old_id => {
                    // Optimization for when only the executable bit changed
                    assert_ne!(executable, old_executable);
                    self.set_executable(&disk_path, executable);
                    let file_state = self.file_states.get_mut(&path).unwrap();
                    file_state.mark_executable(executable);
                    stats.updated_files += 1;
                }
                Diff::Modified(before, after) => {
                    fs::remove_file(&disk_path).ok();
                    let file_state = match (before, after) {
                        (_, TreeValue::Normal { id, executable }) => {
                            self.write_file(&disk_path, &path, &id, executable)
                        }
                        (_, TreeValue::Symlink(id)) => self.write_symlink(&disk_path, &path, &id),
                        (_, TreeValue::Conflict(id)) => self.write_conflict(&disk_path, &path, &id),
                        (_, TreeValue::GitSubmodule(_id)) => {
                            println!("ignoring git submodule at {:?}", path);
                            self.file_states.remove(&path);
                            continue;
                        }
                        (_, TreeValue::Tree(_id)) => {
                            panic!("unexpected tree entry in diff at {:?}", path);
                        }
                    };

                    self.file_states.insert(path.clone(), file_state);
                    stats.updated_files += 1;
                }
            }
        }
        Ok(stats)
    }

    pub fn reset(&mut self, new_tree: &Tree) -> Result<(), ResetError> {
        let old_tree = self
            .store
            .get_tree(&RepoPath::root(), &self.tree_id)
            .map_err(|err| match err {
                BackendError::NotFound => ResetError::SourceNotFound,
                other => ResetError::InternalBackendError(other),
            })?;

        for (path, diff) in old_tree.diff(new_tree, &EverythingMatcher) {
            match diff {
                Diff::Removed(_before) => {
                    self.file_states.remove(&path);
                }
                Diff::Added(after) | Diff::Modified(_, after) => {
                    let file_type = match after {
                        TreeValue::Normal { id: _, executable } => FileType::Normal { executable },
                        TreeValue::Symlink(_id) => FileType::Symlink,
                        TreeValue::Conflict(id) => FileType::Conflict { id },
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {:?}", path);
                            continue;
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {:?}", path);
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

pub struct WorkingCopy {
    store: Arc<Store>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    operation_id: RefCell<Option<OperationId>>,
    workspace_id: RefCell<Option<WorkspaceId>>,
    tree_state: RefCell<Option<TreeState>>,
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
        let mut proto = crate::protos::working_copy::Checkout::new();
        proto.operation_id = operation_id.to_bytes();
        proto.workspace_id = workspace_id.as_str().to_string();
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state_path.join("checkout"))
            .unwrap();
        proto.write_to_writer(&mut file).unwrap();
        WorkingCopy {
            store,
            working_copy_path,
            state_path,
            operation_id: RefCell::new(Some(operation_id)),
            workspace_id: RefCell::new(Some(workspace_id)),
            tree_state: RefCell::new(None),
        }
    }

    pub fn load(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> WorkingCopy {
        WorkingCopy {
            store,
            working_copy_path,
            state_path,
            operation_id: RefCell::new(None),
            workspace_id: RefCell::new(None),
            tree_state: RefCell::new(None),
        }
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    fn write_proto(&self, proto: crate::protos::working_copy::Checkout) {
        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        proto.write_to_writer(temp_file.as_file_mut()).unwrap();
        // TODO: Retry if persisting fails (it will on Windows if the file happened to
        // be open for read).
        temp_file.persist(self.state_path.join("checkout")).unwrap();
    }

    fn load_proto(&self) {
        let mut file = File::open(self.state_path.join("checkout")).unwrap();
        let proto: crate::protos::working_copy::Checkout =
            Message::parse_from_reader(&mut file).unwrap();
        self.operation_id
            .replace(Some(OperationId::new(proto.operation_id)));
        let workspace_id = if proto.workspace_id.is_empty() {
            // For compatibility with old working copies.
            // TODO: Delete in mid 2022 or so
            WorkspaceId::default()
        } else {
            WorkspaceId::new(proto.workspace_id)
        };
        self.workspace_id.replace(Some(workspace_id));
    }

    pub fn operation_id(&self) -> OperationId {
        if self.operation_id.borrow().is_none() {
            self.load_proto();
        }

        self.operation_id.borrow().as_ref().unwrap().clone()
    }

    pub fn workspace_id(&self) -> WorkspaceId {
        if self.workspace_id.borrow().is_none() {
            self.load_proto();
        }

        self.workspace_id.borrow().as_ref().unwrap().clone()
    }

    fn tree_state(&self) -> RefMut<Option<TreeState>> {
        if self.tree_state.borrow().is_none() {
            self.tree_state.replace(Some(TreeState::load(
                self.store.clone(),
                self.working_copy_path.clone(),
                self.state_path.clone(),
            )));
        }
        self.tree_state.borrow_mut()
    }

    pub fn current_tree_id(&self) -> TreeId {
        self.tree_state()
            .as_ref()
            .unwrap()
            .current_tree_id()
            .clone()
    }

    pub fn file_states(&self) -> BTreeMap<RepoPath, FileState> {
        self.tree_state().as_ref().unwrap().file_states().clone()
    }

    fn save(&mut self) {
        let mut proto = crate::protos::working_copy::Checkout::new();
        proto.operation_id = self.operation_id().to_bytes();
        proto.workspace_id = self.workspace_id().as_str().to_string();
        self.write_proto(proto);
    }

    pub fn start_mutation(&mut self) -> LockedWorkingCopy {
        let lock_path = self.state_path.join("working_copy.lock");
        let lock = FileLock::lock(lock_path);

        // Re-read from disk after taking the lock
        self.load_proto();
        // TODO: It's expensive to reload the whole tree. We should first check if it
        // has changed.
        self.tree_state.replace(None);
        let old_operation_id = self.operation_id();
        let old_tree_id = self.current_tree_id();

        LockedWorkingCopy {
            wc: self,
            lock,
            old_operation_id,
            old_tree_id,
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
        // Check if the current checkout has changed on disk compared to what the caller
        // expected. It's safe to check out another commit regardless, but it's
        // probably not what  the caller wanted, so we let them know.
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
}

/// A working copy that's locked on disk. The lock is held until you call
/// `finish()` or `discard()`.
pub struct LockedWorkingCopy<'a> {
    wc: &'a mut WorkingCopy,
    #[allow(dead_code)]
    lock: FileLock,
    old_operation_id: OperationId,
    old_tree_id: TreeId,
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

    // The base_ignores are passed in here rather than being set on the TreeState
    // because the TreeState may be long-lived if the library is used in a
    // long-lived process.
    pub fn write_tree(&mut self, base_ignores: Arc<GitIgnoreFile>) -> TreeId {
        self.wc
            .tree_state()
            .as_mut()
            .unwrap()
            .write_tree(base_ignores)
    }

    pub fn check_out(&mut self, new_tree: &Tree) -> Result<CheckoutStats, CheckoutError> {
        // TODO: Write a "pending_checkout" file with the old and new TreeIds so we can
        // continue an interrupted checkout if we find such a file.
        let stats = self.wc.tree_state().as_mut().unwrap().check_out(new_tree)?;
        Ok(stats)
    }

    pub fn reset(&mut self, new_tree: &Tree) -> Result<(), ResetError> {
        self.wc.tree_state().as_mut().unwrap().reset(new_tree)
    }

    pub fn finish(mut self, operation_id: OperationId) {
        self.wc.tree_state().as_mut().unwrap().save();
        self.wc.operation_id.replace(Some(operation_id));
        self.wc.save();
        // TODO: Clear the "pending_checkout" file here.
        self.closed = true;
    }

    pub fn discard(mut self) {
        // Undo the changes in memory
        self.wc.load_proto();
        self.wc.tree_state.replace(None);
        self.closed = true;
    }
}

impl Drop for LockedWorkingCopy<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                self.closed,
                "Working copy lock was dropped without being closed."
            );
        }
    }
}
