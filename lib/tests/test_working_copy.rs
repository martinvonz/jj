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

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use itertools::Itertools;
use jj_lib::backend::{TreeId, TreeValue};
use jj_lib::conflicts::Conflict;
use jj_lib::fsmonitor::FsmonitorKind;
#[cfg(unix)]
use jj_lib::op_store::OperationId;
use jj_lib::op_store::WorkspaceId;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use jj_lib::settings::UserSettings;
use jj_lib::tree_builder::TreeBuilder;
use jj_lib::working_copy::{LockedWorkingCopy, SnapshotOptions, WorkingCopy};
use test_case::test_case;
use testutils::{write_random_commit, TestWorkspace};

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_root(use_git: bool) {
    // Test that the working copy is clean and empty after init.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;

    let wc = test_workspace.workspace.working_copy_mut();
    assert_eq!(wc.sparse_patterns(), vec![RepoPath::root()]);
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.discard();
    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(&WorkspaceId::default())
        .unwrap();
    let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();
    assert_eq!(&new_tree_id, wc_commit.tree_id());
    assert_eq!(&new_tree_id, repo.store().empty_tree_id());
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_checkout_file_transitions(use_git: bool) {
    // Tests switching between commits where a certain path is of one type in one
    // commit and another type in the other. Includes a "missing" type, so we cover
    // additions and removals as well.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let store = repo.store().clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum Kind {
        Missing,
        Normal,
        Executable,
        // Executable, but same content as Normal, to test transition where only the bit changed
        ExecutableNormalContent,
        Conflict,
        #[cfg_attr(windows, allow(dead_code))]
        Symlink,
        Tree,
        GitSubmodule,
    }

    fn write_path(
        settings: &UserSettings,
        repo: &Arc<ReadonlyRepo>,
        tree_builder: &mut TreeBuilder,
        kind: Kind,
        path: &RepoPath,
    ) {
        let store = repo.store();
        let value = match kind {
            Kind::Missing => {
                return;
            }
            Kind::Normal => {
                let id = testutils::write_file(store, path, "normal file contents");
                TreeValue::File {
                    id,
                    executable: false,
                }
            }
            Kind::Executable => {
                let id = testutils::write_file(store, path, "executable file contents");
                TreeValue::File {
                    id,
                    executable: true,
                }
            }
            Kind::ExecutableNormalContent => {
                let id = testutils::write_file(store, path, "normal file contents");
                TreeValue::File {
                    id,
                    executable: true,
                }
            }
            Kind::Conflict => {
                let base_file_id = testutils::write_file(store, path, "base file contents");
                let left_file_id = testutils::write_file(store, path, "left file contents");
                let right_file_id = testutils::write_file(store, path, "right file contents");
                let conflict = Conflict::new(
                    vec![Some(TreeValue::File {
                        id: base_file_id,
                        executable: false,
                    })],
                    vec![
                        Some(TreeValue::File {
                            id: left_file_id,
                            executable: false,
                        }),
                        Some(TreeValue::File {
                            id: right_file_id,
                            executable: false,
                        }),
                    ],
                );
                let conflict_id = store.write_conflict(path, &conflict).unwrap();
                TreeValue::Conflict(conflict_id)
            }
            Kind::Symlink => {
                let id = store.write_symlink(path, "target").unwrap();
                TreeValue::Symlink(id)
            }
            Kind::Tree => {
                let mut sub_tree_builder = store.tree_builder(store.empty_tree_id().clone());
                let file_path = path.join(&RepoPathComponent::from("file"));
                write_path(
                    settings,
                    repo,
                    &mut sub_tree_builder,
                    Kind::Normal,
                    &file_path,
                );
                let id = sub_tree_builder.write_tree();
                TreeValue::Tree(id)
            }
            Kind::GitSubmodule => {
                let mut tx = repo.start_transaction(settings, "test");
                let id = write_random_commit(tx.mut_repo(), settings).id().clone();
                tx.commit();
                TreeValue::GitSubmodule(id)
            }
        };
        tree_builder.set(path.clone(), value);
    }

    let mut kinds = vec![
        Kind::Missing,
        Kind::Normal,
        Kind::Executable,
        Kind::ExecutableNormalContent,
        Kind::Conflict,
        Kind::Tree,
    ];
    #[cfg(unix)]
    kinds.push(Kind::Symlink);
    if use_git {
        kinds.push(Kind::GitSubmodule);
    }
    let mut left_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut right_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut files = vec![];
    for left_kind in &kinds {
        for right_kind in &kinds {
            let path = RepoPath::from_internal_string(&format!("{left_kind:?}_{right_kind:?}"));
            write_path(&settings, repo, &mut left_tree_builder, *left_kind, &path);
            write_path(&settings, repo, &mut right_tree_builder, *right_kind, &path);
            files.push((*left_kind, *right_kind, path));
        }
    }
    let left_tree_id = left_tree_builder.write_tree();
    let right_tree_id = right_tree_builder.write_tree();
    let left_tree = store.get_tree(&RepoPath::root(), &left_tree_id).unwrap();
    let right_tree = store.get_tree(&RepoPath::root(), &right_tree_id).unwrap();

    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, &left_tree)
        .unwrap();
    wc.check_out(repo.op_id().clone(), None, &right_tree)
        .unwrap();

    // Check that the working copy is clean.
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.discard();
    assert_eq!(new_tree_id, right_tree_id);

    for (_left_kind, right_kind, path) in &files {
        let wc_path = workspace_root.join(path.to_internal_file_string());
        let maybe_metadata = wc_path.symlink_metadata();
        match right_kind {
            Kind::Missing => {
                assert!(maybe_metadata.is_err(), "{path:?} should not exist");
            }
            Kind::Normal => {
                assert!(maybe_metadata.is_ok(), "{path:?} should exist");
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_file(), "{path:?} should be a file");
                #[cfg(unix)]
                assert_eq!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{path:?} should not be executable"
                );
            }
            Kind::Executable | Kind::ExecutableNormalContent => {
                assert!(maybe_metadata.is_ok(), "{path:?} should exist");
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_file(), "{path:?} should be a file");
                #[cfg(unix)]
                assert_ne!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{path:?} should be executable"
                );
            }
            Kind::Conflict => {
                assert!(maybe_metadata.is_ok(), "{path:?} should exist");
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_file(), "{path:?} should be a file");
                #[cfg(unix)]
                assert_eq!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{path:?} should not be executable"
                );
            }
            Kind::Symlink => {
                assert!(maybe_metadata.is_ok(), "{path:?} should exist");
                let metadata = maybe_metadata.unwrap();
                assert!(
                    metadata.file_type().is_symlink(),
                    "{path:?} should be a symlink"
                );
            }
            Kind::Tree => {
                assert!(maybe_metadata.is_ok(), "{path:?} should exist");
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_dir(), "{path:?} should be a directory");
            }
            Kind::GitSubmodule => {
                // Not supported for now
                assert!(maybe_metadata.is_err(), "{path:?} should not exist");
            }
        };
    }
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_tree_builder_file_directory_transition(use_git: bool) {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let store = repo.store();
    let mut workspace = test_workspace.workspace;
    let workspace_root = workspace.workspace_root().clone();
    let mut check_out_tree = |tree_id: &TreeId| {
        let tree = repo.store().get_tree(&RepoPath::root(), tree_id).unwrap();
        let wc = workspace.working_copy_mut();
        wc.check_out(repo.op_id().clone(), None, &tree).unwrap();
    };

    let parent_path = RepoPath::from_internal_string("foo/bar");
    let child_path = RepoPath::from_internal_string("foo/bar/baz");

    // Add file at parent_path
    let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
    testutils::write_normal_file(&mut tree_builder, &parent_path, "");
    let tree_id = tree_builder.write_tree();
    check_out_tree(&tree_id);
    assert!(parent_path.to_fs_path(&workspace_root).is_file());
    assert!(!child_path.to_fs_path(&workspace_root).exists());

    // Turn parent_path into directory, add file at child_path
    let mut tree_builder = store.tree_builder(tree_id);
    tree_builder.remove(parent_path.clone());
    testutils::write_normal_file(&mut tree_builder, &child_path, "");
    let tree_id = tree_builder.write_tree();
    check_out_tree(&tree_id);
    assert!(parent_path.to_fs_path(&workspace_root).is_dir());
    assert!(child_path.to_fs_path(&workspace_root).is_file());

    // Turn parent_path back to file
    let mut tree_builder = store.tree_builder(tree_id);
    tree_builder.remove(child_path.clone());
    testutils::write_normal_file(&mut tree_builder, &parent_path, "");
    let tree_id = tree_builder.write_tree();
    check_out_tree(&tree_id);
    assert!(parent_path.to_fs_path(&workspace_root).is_file());
    assert!(!child_path.to_fs_path(&workspace_root).exists());
}

#[test]
fn test_reset() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, false);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let ignored_path = RepoPath::from_internal_string("ignored");
    let gitignore_path = RepoPath::from_internal_string(".gitignore");

    let tree_without_file = testutils::create_tree(repo, &[(&gitignore_path, "ignored\n")]);
    let tree_with_file = testutils::create_tree(
        repo,
        &[(&gitignore_path, "ignored\n"), (&ignored_path, "code")],
    );

    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, &tree_with_file)
        .unwrap();

    // Test the setup: the file should exist on disk and in the tree state.
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(wc.file_states().contains_key(&ignored_path));

    // After we reset to the commit without the file, it should still exist on disk,
    // but it should not be in the tree state, and it should not get added when we
    // commit the working copy (because it's ignored).
    let mut locked_wc = wc.start_mutation();
    locked_wc.reset(&tree_without_file).unwrap();
    locked_wc.finish(repo.op_id().clone());
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(!wc.file_states().contains_key(&ignored_path));
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    assert_eq!(new_tree_id, *tree_without_file.id());
    locked_wc.discard();

    // After we reset to the commit without the file, it should still exist on disk,
    // but it should not be in the tree state, and it should not get added when we
    // commit the working copy (because it's ignored).
    let mut locked_wc = wc.start_mutation();
    locked_wc.reset(&tree_without_file).unwrap();
    locked_wc.finish(repo.op_id().clone());
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(!wc.file_states().contains_key(&ignored_path));
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    assert_eq!(new_tree_id, *tree_without_file.id());
    locked_wc.discard();

    // Now test the opposite direction: resetting to a commit where the file is
    // tracked. The file should become tracked (even though it's ignored).
    let mut locked_wc = wc.start_mutation();
    locked_wc.reset(&tree_with_file).unwrap();
    locked_wc.finish(repo.op_id().clone());
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(wc.file_states().contains_key(&ignored_path));
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    assert_eq!(new_tree_id, *tree_with_file.id());
    locked_wc.discard();
}

#[test]
fn test_checkout_discard() {
    // Start a mutation, do a checkout, and then discard the mutation. The working
    // copy files should remain changed, but the state files should not be
    // written.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, false);
    let repo = test_workspace.repo.clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let file1_path = RepoPath::from_internal_string("file1");
    let file2_path = RepoPath::from_internal_string("file2");

    let store = repo.store();
    let tree1 = testutils::create_tree(&repo, &[(&file1_path, "contents")]);
    let tree2 = testutils::create_tree(&repo, &[(&file2_path, "contents")]);

    let wc = test_workspace.workspace.working_copy_mut();
    let state_path = wc.state_path().to_path_buf();
    wc.check_out(repo.op_id().clone(), None, &tree1).unwrap();

    // Test the setup: the file should exist on disk and in the tree state.
    assert!(file1_path.to_fs_path(&workspace_root).is_file());
    assert!(wc.file_states().contains_key(&file1_path));

    // Start a checkout
    let mut locked_wc = wc.start_mutation();
    locked_wc.check_out(&tree2).unwrap();
    // The change should be reflected in the working copy but not saved
    assert!(!file1_path.to_fs_path(&workspace_root).is_file());
    assert!(file2_path.to_fs_path(&workspace_root).is_file());
    let reloaded_wc = WorkingCopy::load(store.clone(), workspace_root.clone(), state_path.clone());
    assert!(reloaded_wc.file_states().contains_key(&file1_path));
    assert!(!reloaded_wc.file_states().contains_key(&file2_path));
    locked_wc.discard();

    // The change should remain in the working copy, but not in memory and not saved
    assert!(wc.file_states().contains_key(&file1_path));
    assert!(!wc.file_states().contains_key(&file2_path));
    assert!(!file1_path.to_fs_path(&workspace_root).is_file());
    assert!(file2_path.to_fs_path(&workspace_root).is_file());
    let reloaded_wc = WorkingCopy::load(store.clone(), workspace_root, state_path);
    assert!(reloaded_wc.file_states().contains_key(&file1_path));
    assert!(!reloaded_wc.file_states().contains_key(&file2_path));
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_snapshot_racy_timestamps(use_git: bool) {
    // Tests that file modifications are detected even if they happen the same
    // millisecond as the updated working copy state.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let file_path = workspace_root.join("file");
    let mut previous_tree_id = repo.store().empty_tree_id().clone();
    let wc = test_workspace.workspace.working_copy_mut();
    for i in 0..100 {
        {
            // https://github.com/rust-lang/rust-clippy/issues/9778
            #[allow(clippy::needless_borrow)]
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .open(&file_path)
                .unwrap();
            file.write_all(format!("contents {i}").as_bytes()).unwrap();
        }
        let mut locked_wc = wc.start_mutation();
        let new_tree_id = locked_wc
            .snapshot(SnapshotOptions::empty_for_test())
            .unwrap();
        locked_wc.discard();
        assert_ne!(new_tree_id, previous_tree_id);
        previous_tree_id = new_tree_id;
    }
}

#[cfg(unix)]
#[test]
fn test_snapshot_special_file() {
    // Tests that we ignore when special files (such as sockets and pipes) exist on
    // disk.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, false);
    let workspace_root = test_workspace.workspace.workspace_root().clone();
    let store = test_workspace.repo.store();

    let file1_path = RepoPath::from_internal_string("file1");
    let file1_disk_path = file1_path.to_fs_path(&workspace_root);
    std::fs::write(&file1_disk_path, "contents".as_bytes()).unwrap();
    let file2_path = RepoPath::from_internal_string("file2");
    let file2_disk_path = file2_path.to_fs_path(&workspace_root);
    std::fs::write(file2_disk_path, "contents".as_bytes()).unwrap();
    let socket_disk_path = workspace_root.join("socket");
    UnixListener::bind(&socket_disk_path).unwrap();
    // Test the setup
    assert!(socket_disk_path.exists());
    assert!(!socket_disk_path.is_file());

    // Snapshot the working copy with the socket file
    let wc = test_workspace.workspace.working_copy_mut();
    let mut locked_wc = wc.start_mutation();
    let tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.finish(OperationId::from_hex("abc123"));
    let tree = store.get_tree(&RepoPath::root(), &tree_id).unwrap();
    // Only the regular files should be in the tree
    assert_eq!(
        tree.entries().map(|(path, _value)| path).collect_vec(),
        vec![file1_path.clone(), file2_path.clone()]
    );
    assert_eq!(
        wc.file_states().keys().cloned().collect_vec(),
        vec![file1_path, file2_path.clone()]
    );

    // Replace a regular file by a socket and snapshot the working copy again
    std::fs::remove_file(&file1_disk_path).unwrap();
    UnixListener::bind(&file1_disk_path).unwrap();
    let mut locked_wc = wc.start_mutation();
    let tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.finish(OperationId::from_hex("abc123"));
    let tree = store.get_tree(&RepoPath::root(), &tree_id).unwrap();
    // Only the regular file should be in the tree
    assert_eq!(
        tree.entries().map(|(path, _value)| path).collect_vec(),
        vec![file2_path.clone()]
    );
    assert_eq!(
        wc.file_states().keys().cloned().collect_vec(),
        vec![file2_path]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_gitignores(use_git: bool) {
    // Tests that .gitignore files are respected.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    let added_path = RepoPath::from_internal_string("added");
    let modified_path = RepoPath::from_internal_string("modified");
    let removed_path = RepoPath::from_internal_string("removed");
    let ignored_path = RepoPath::from_internal_string("ignored");
    let subdir_modified_path = RepoPath::from_internal_string("dir/modified");
    let subdir_ignored_path = RepoPath::from_internal_string("dir/ignored");

    testutils::write_working_copy_file(&workspace_root, &gitignore_path, "ignored\n");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "1");
    testutils::write_working_copy_file(&workspace_root, &removed_path, "1");
    std::fs::create_dir(workspace_root.join("dir")).unwrap();
    testutils::write_working_copy_file(&workspace_root, &subdir_modified_path, "1");

    let wc = test_workspace.workspace.working_copy_mut();
    let mut locked_wc = wc.start_mutation();
    let new_tree_id1 = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.finish(repo.op_id().clone());
    let tree1 = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id1)
        .unwrap();
    let files1 = tree1.entries().map(|(name, _value)| name).collect_vec();
    assert_eq!(
        files1,
        vec![
            gitignore_path.clone(),
            subdir_modified_path.clone(),
            modified_path.clone(),
            removed_path.clone(),
        ]
    );

    testutils::write_working_copy_file(
        &workspace_root,
        &gitignore_path,
        "ignored\nmodified\nremoved\n",
    );
    testutils::write_working_copy_file(&workspace_root, &added_path, "2");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "2");
    std::fs::remove_file(removed_path.to_fs_path(&workspace_root)).unwrap();
    testutils::write_working_copy_file(&workspace_root, &ignored_path, "2");
    testutils::write_working_copy_file(&workspace_root, &subdir_modified_path, "2");
    testutils::write_working_copy_file(&workspace_root, &subdir_ignored_path, "2");

    let mut locked_wc = wc.start_mutation();
    let new_tree_id2 = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.discard();
    let tree2 = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id2)
        .unwrap();
    let files2 = tree2.entries().map(|(name, _value)| name).collect_vec();
    assert_eq!(
        files2,
        vec![
            gitignore_path,
            added_path,
            subdir_modified_path,
            modified_path,
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_gitignores_checkout_never_overwrites_ignored(use_git: bool) {
    // Tests that a .gitignore'd file doesn't get overwritten if check out a commit
    // where the file is tracked.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Write an ignored file called "modified" to disk
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(&workspace_root, &gitignore_path, "modified\n");
    let modified_path = RepoPath::from_internal_string("modified");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "garbage");

    // Create a tree that adds the same file but with different contents
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    testutils::write_normal_file(&mut tree_builder, &modified_path, "contents");
    let tree_id = tree_builder.write_tree();
    let tree = repo.store().get_tree(&RepoPath::root(), &tree_id).unwrap();

    // Now check out the tree that adds the file "modified" with contents
    // "contents". The exiting contents ("garbage") shouldn't be replaced in the
    // working copy.
    let wc = test_workspace.workspace.working_copy_mut();
    assert!(wc.check_out(repo.op_id().clone(), None, &tree).is_err());

    // Check that the old contents are in the working copy
    let path = workspace_root.join("modified");
    assert!(path.is_file());
    assert_eq!(std::fs::read(&path).unwrap(), b"garbage");
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_gitignores_ignored_directory_already_tracked(use_git: bool) {
    // Tests that a .gitignore'd directory that already has a tracked file in it
    // does not get removed when snapshotting the working directory.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;

    // Add a .gitignore file saying to ignore the directory "ignored/"
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(
        test_workspace.workspace.workspace_root(),
        &gitignore_path,
        "/ignored/\n",
    );
    let file_path = RepoPath::from_internal_string("ignored/file");

    // Create a tree that adds a file in the ignored directory
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    testutils::write_normal_file(&mut tree_builder, &file_path, "contents");
    let tree_id = tree_builder.write_tree();
    let tree = repo.store().get_tree(&RepoPath::root(), &tree_id).unwrap();

    // Check out the tree with the file in ignored/
    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, &tree).unwrap();

    // Check that the file is still in the tree created by snapshotting the working
    // copy (that it didn't get removed because the directory is ignored)
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.discard();
    let new_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id)
        .unwrap();
    assert!(new_tree.path_value(&file_path).is_some());
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_dotgit_ignored(use_git: bool) {
    // Tests that .git directories and files are always ignored (we could accept
    // them if the backend is not git).

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Test with a .git/ directory (with a file in, since we don't write empty
    // trees)
    let dotgit_path = workspace_root.join(".git");
    std::fs::create_dir(&dotgit_path).unwrap();
    testutils::write_working_copy_file(
        &workspace_root,
        &RepoPath::from_internal_string(".git/file"),
        "contents",
    );
    let mut locked_wc = test_workspace.workspace.working_copy_mut().start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    assert_eq!(new_tree_id, *repo.store().empty_tree_id());
    locked_wc.discard();
    std::fs::remove_dir_all(&dotgit_path).unwrap();

    // Test with a .git file
    testutils::write_working_copy_file(
        &workspace_root,
        &RepoPath::from_internal_string(".git"),
        "contents",
    );
    let mut locked_wc = test_workspace.workspace.working_copy_mut().start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    assert_eq!(new_tree_id, *repo.store().empty_tree_id());
    locked_wc.discard();
}

#[test]
fn test_gitsubmodule() {
    // Tests that git submodules are ignored.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, true);
    let repo = &test_workspace.repo;
    let store = repo.store().clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());

    let added_path = RepoPath::from_internal_string("added");
    let submodule_path = RepoPath::from_internal_string("submodule");
    let added_submodule_path = RepoPath::from_internal_string("submodule/added");

    tree_builder.set(
        added_path.clone(),
        TreeValue::File {
            id: testutils::write_file(repo.store(), &added_path, "added\n"),
            executable: false,
        },
    );

    let mut tx = repo.start_transaction(&settings, "create submodule commit");
    let submodule_id = write_random_commit(tx.mut_repo(), &settings).id().clone();
    tx.commit();

    tree_builder.set(
        submodule_path.clone(),
        TreeValue::GitSubmodule(submodule_id),
    );

    let tree_id = tree_builder.write_tree();
    let tree = store.get_tree(&RepoPath::root(), &tree_id).unwrap();
    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, &tree).unwrap();

    std::fs::create_dir(submodule_path.to_fs_path(&workspace_root)).unwrap();

    testutils::write_working_copy_file(
        &workspace_root,
        &added_submodule_path,
        "i am a file in a submodule\n",
    );

    // Check that the files present in the submodule are not tracked
    // when we snapshot
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_wc.discard();
    assert_eq!(new_tree_id, tree_id);

    // Check that the files in the submodule are not deleted
    let file_in_submodule_path = added_submodule_path.to_fs_path(&workspace_root);
    assert!(
        file_in_submodule_path.metadata().is_ok(),
        "{file_in_submodule_path:?} should exist"
    );
}

#[cfg(unix)]
#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_existing_directory_symlink(use_git: bool) {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Creates a symlink in working directory, and a tree that will add a file under
    // the symlinked directory.
    std::os::unix::fs::symlink("..", workspace_root.join("parent")).unwrap();
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    testutils::write_normal_file(
        &mut tree_builder,
        &RepoPath::from_internal_string("parent/escaped"),
        "contents",
    );
    let tree_id = tree_builder.write_tree();
    let tree = repo.store().get_tree(&RepoPath::root(), &tree_id).unwrap();

    // Checkout should fail because "parent" already exists and is a symlink.
    let wc = test_workspace.workspace.working_copy_mut();
    assert!(wc.check_out(repo.op_id().clone(), None, &tree).is_err());

    // Therefore, "../escaped" shouldn't be created.
    assert!(!workspace_root.parent().unwrap().join("escaped").exists());
}

#[test]
fn test_fsmonitor() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings, true);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let wc = test_workspace.workspace.working_copy_mut();
    assert_eq!(wc.sparse_patterns(), vec![RepoPath::root()]);

    let foo_path = RepoPath::from_internal_string("foo");
    let bar_path = RepoPath::from_internal_string("bar");
    let nested_path = RepoPath::from_internal_string("path/to/nested");
    testutils::write_working_copy_file(&workspace_root, &foo_path, "foo\n");
    testutils::write_working_copy_file(&workspace_root, &bar_path, "bar\n");
    testutils::write_working_copy_file(&workspace_root, &nested_path, "nested\n");

    let ignored_path = RepoPath::from_internal_string("path/to/ignored");
    let gitignore_path = RepoPath::from_internal_string("path/.gitignore");
    testutils::write_working_copy_file(&workspace_root, &ignored_path, "ignored\n");
    testutils::write_working_copy_file(&workspace_root, &gitignore_path, "to/ignored\n");

    let snapshot = |locked_wc: &mut LockedWorkingCopy, paths: &[&RepoPath]| {
        let fs_paths = paths
            .iter()
            .map(|p| p.to_fs_path(&workspace_root))
            .collect();
        locked_wc
            .snapshot(SnapshotOptions {
                fsmonitor_kind: Some(FsmonitorKind::Test {
                    changed_files: fs_paths,
                }),
                ..SnapshotOptions::empty_for_test()
            })
            .unwrap()
    };

    {
        let mut locked_wc = wc.start_mutation();
        let tree_id = snapshot(&mut locked_wc, &[]);
        assert_eq!(tree_id, *repo.store().empty_tree_id());
        locked_wc.discard();
    }

    {
        let mut locked_wc = wc.start_mutation();
        let tree_id = snapshot(&mut locked_wc, &[&foo_path]);
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree 205f6b799e7d5c2524468ca006a0131aa57ecce7
          file "foo" (257cc5642cb1a054f08cc83f2d943e56fd3ebe99): "foo\n"
        "###);
        locked_wc.discard();
    }

    {
        let mut locked_wc = wc.start_mutation();
        let tree_id = snapshot(
            &mut locked_wc,
            &[&foo_path, &bar_path, &nested_path, &ignored_path],
        );
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree ab5a0465cc71725a723f28b685844a5bc0f5b599
          file "bar" (5716ca5987cbf97d6bb54920bea6adde242d87e6): "bar\n"
          file "foo" (257cc5642cb1a054f08cc83f2d943e56fd3ebe99): "foo\n"
          file "path/to/nested" (79c53955ef856f16f2107446bc721c8879a1bd2e): "nested\n"
        "###);
        locked_wc.finish(repo.op_id().clone());
    }

    {
        testutils::write_working_copy_file(&workspace_root, &foo_path, "updated foo\n");
        testutils::write_working_copy_file(&workspace_root, &bar_path, "updated bar\n");
        let mut locked_wc = wc.start_mutation();
        let tree_id = snapshot(&mut locked_wc, &[&foo_path]);
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree 2f57ab8f48ae62e3137079f2add9878dfa1d1bcc
          file "bar" (5716ca5987cbf97d6bb54920bea6adde242d87e6): "bar\n"
          file "foo" (9d053d7c8a18a286dce9b99a59bb058be173b463): "updated foo\n"
          file "path/to/nested" (79c53955ef856f16f2107446bc721c8879a1bd2e): "nested\n"
        "###);
        locked_wc.discard();
    }

    {
        std::fs::remove_file(foo_path.to_fs_path(&workspace_root)).unwrap();
        let mut locked_wc = wc.start_mutation();
        let tree_id = snapshot(&mut locked_wc, &[&foo_path]);
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree 34b83765131477e1a7d72160079daec12c6144e3
          file "bar" (5716ca5987cbf97d6bb54920bea6adde242d87e6): "bar\n"
          file "path/to/nested" (79c53955ef856f16f2107446bc721c8879a1bd2e): "nested\n"
        "###);
        locked_wc.finish(repo.op_id().clone());
    }
}
