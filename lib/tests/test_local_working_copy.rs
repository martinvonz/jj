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

// TODO: Remove when MSRV passes 1.72
// https://github.com/frondeus/test-case/issues/126#issuecomment-1635916592
#![allow(clippy::items_after_test_module)]

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use itertools::Itertools;
use jj_lib::backend::{MergedTreeId, ObjectId, TreeId, TreeValue};
use jj_lib::fsmonitor::FsmonitorKind;
use jj_lib::local_working_copy::LocalWorkingCopy;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::{MergedTree, MergedTreeBuilder};
use jj_lib::op_store::{OperationId, WorkspaceId};
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use jj_lib::settings::UserSettings;
use jj_lib::working_copy::{CheckoutStats, LockedWorkingCopy, SnapshotError, SnapshotOptions};
use jj_lib::workspace::LockedWorkspace;
use test_case::test_case;
use testutils::{create_tree, write_random_commit, TestRepoBackend, TestWorkspace};

#[test]
fn test_root() {
    // Test that the working copy is clean and empty after init.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);

    let wc = test_workspace.workspace.working_copy();
    assert_eq!(wc.sparse_patterns().unwrap(), vec![RepoPath::root()]);
    let new_tree = test_workspace.snapshot().unwrap();
    let repo = &test_workspace.repo;
    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(&WorkspaceId::default())
        .unwrap();
    let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();
    assert_eq!(new_tree.id(), *wc_commit.tree_id());
    assert_eq!(new_tree.id(), repo.store().empty_merged_tree_id());
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_checkout_file_transitions(backend: TestRepoBackend) {
    // Tests switching between commits where a certain path is of one type in one
    // commit and another type in the other. Includes a "missing" type, so we cover
    // additions and removals as well.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init_with_backend(&settings, backend);
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
        tree_builder: &mut MergedTreeBuilder,
        kind: Kind,
        path: &RepoPath,
    ) {
        let store = repo.store();
        let value = match kind {
            Kind::Missing => Merge::absent(),
            Kind::Normal => {
                let id = testutils::write_file(store, path, "normal file contents");
                Merge::normal(TreeValue::File {
                    id,
                    executable: false,
                })
            }
            Kind::Executable => {
                let id = testutils::write_file(store, path, "executable file contents");
                Merge::normal(TreeValue::File {
                    id,
                    executable: true,
                })
            }
            Kind::ExecutableNormalContent => {
                let id = testutils::write_file(store, path, "normal file contents");
                Merge::normal(TreeValue::File {
                    id,
                    executable: true,
                })
            }
            Kind::Conflict => {
                let base_file_id = testutils::write_file(store, path, "base file contents");
                let left_file_id = testutils::write_file(store, path, "left file contents");
                let right_file_id = testutils::write_file(store, path, "right file contents");
                Merge::new(
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
                )
            }
            Kind::Symlink => {
                let id = store.write_symlink(path, "target").unwrap();
                Merge::normal(TreeValue::Symlink(id))
            }
            Kind::Tree => {
                let file_path = path.join(&RepoPathComponent::from("file"));
                let id = testutils::write_file(store, &file_path, "normal file contents");
                let value = TreeValue::File {
                    id,
                    executable: false,
                };
                tree_builder.set_or_remove(file_path, Merge::normal(value));
                return;
            }
            Kind::GitSubmodule => {
                let mut tx = repo.start_transaction(settings, "test");
                let id = write_random_commit(tx.mut_repo(), settings).id().clone();
                tx.commit();
                Merge::normal(TreeValue::GitSubmodule(id))
            }
        };
        tree_builder.set_or_remove(path.clone(), value);
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
    if backend == TestRepoBackend::Git {
        kinds.push(Kind::GitSubmodule);
    }
    let mut left_tree_builder = MergedTreeBuilder::new(store.empty_merged_tree_id());
    let mut right_tree_builder = MergedTreeBuilder::new(store.empty_merged_tree_id());
    let mut files = vec![];
    for left_kind in &kinds {
        for right_kind in &kinds {
            let path = RepoPath::from_internal_string(&format!("{left_kind:?}_{right_kind:?}"));
            write_path(&settings, repo, &mut left_tree_builder, *left_kind, &path);
            write_path(&settings, repo, &mut right_tree_builder, *right_kind, &path);
            files.push((*left_kind, *right_kind, path));
        }
    }
    let left_tree_id = left_tree_builder.write_tree(&store).unwrap();
    let right_tree_id = right_tree_builder.write_tree(&store).unwrap();
    let left_tree = store.get_root_tree(&left_tree_id).unwrap();
    let right_tree = store.get_root_tree(&right_tree_id).unwrap();

    let ws = &mut test_workspace.workspace;
    ws.check_out(repo.op_id().clone(), None, &left_tree)
        .unwrap();
    ws.check_out(repo.op_id().clone(), None, &right_tree)
        .unwrap();

    // Check that the working copy is clean.
    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(new_tree.id(), right_tree_id);

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

// Test case for issue #2165
#[test]
fn test_conflict_subdirectory() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;

    let path = RepoPath::from_internal_string("sub/file");
    let empty_tree = create_tree(repo, &[]);
    let tree1 = create_tree(repo, &[(&path, "0")]);
    let tree2 = create_tree(repo, &[(&path, "1")]);
    let merged_tree = tree1.merge(&empty_tree, &tree2).unwrap();
    let repo = &test_workspace.repo;
    let ws = &mut test_workspace.workspace;
    ws.check_out(repo.op_id().clone(), None, &tree1).unwrap();
    ws.check_out(repo.op_id().clone(), None, &merged_tree)
        .unwrap();
}

#[test]
fn test_tree_builder_file_directory_transition() {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let store = repo.store();
    let mut ws = test_workspace.workspace;
    let workspace_root = ws.workspace_root().clone();
    let mut check_out_tree = |tree_id: &TreeId| {
        let tree = repo.store().get_tree(&RepoPath::root(), tree_id).unwrap();
        ws.check_out(repo.op_id().clone(), None, &MergedTree::legacy(tree))
            .unwrap();
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
fn test_conflicting_changes_on_disk() {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let mut ws = test_workspace.workspace;
    let workspace_root = ws.workspace_root().clone();

    // file on disk conflicts with file in target commit
    let file_file_path = RepoPath::from_internal_string("file-file");
    // file on disk conflicts with directory in target commit
    let file_dir_path = RepoPath::from_internal_string("file-dir");
    // directory on disk conflicts with file in target commit
    let dir_file_path = RepoPath::from_internal_string("dir-file");
    let tree = create_tree(
        repo,
        &[
            (&file_file_path, "committed contents"),
            (
                &file_dir_path.join(&RepoPathComponent::from("file")),
                "committed contents",
            ),
            (&dir_file_path, "committed contents"),
        ],
    );

    std::fs::write(
        file_file_path.to_fs_path(&workspace_root),
        "contents on disk",
    )
    .unwrap();
    std::fs::write(
        file_dir_path.to_fs_path(&workspace_root),
        "contents on disk",
    )
    .unwrap();
    std::fs::create_dir(dir_file_path.to_fs_path(&workspace_root)).unwrap();
    std::fs::write(
        dir_file_path.to_fs_path(&workspace_root).join("file"),
        "contents on disk",
    )
    .unwrap();

    let stats = ws.check_out(repo.op_id().clone(), None, &tree).unwrap();
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 3,
            removed_files: 0,
            skipped_files: 3,
        }
    );

    assert_eq!(
        std::fs::read_to_string(file_file_path.to_fs_path(&workspace_root)).ok(),
        Some("contents on disk".to_string())
    );
    assert_eq!(
        std::fs::read_to_string(file_dir_path.to_fs_path(&workspace_root)).ok(),
        Some("contents on disk".to_string())
    );
    assert_eq!(
        std::fs::read_to_string(dir_file_path.to_fs_path(&workspace_root).join("file")).ok(),
        Some("contents on disk".to_string())
    );
}

#[test]
fn test_reset() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let op_id = repo.op_id().clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let ignored_path = RepoPath::from_internal_string("ignored");
    let gitignore_path = RepoPath::from_internal_string(".gitignore");

    let tree_without_file = create_tree(repo, &[(&gitignore_path, "ignored\n")]);
    let tree_with_file = create_tree(
        repo,
        &[(&gitignore_path, "ignored\n"), (&ignored_path, "code")],
    );

    let ws = &mut test_workspace.workspace;
    ws.check_out(repo.op_id().clone(), None, &tree_with_file)
        .unwrap();

    // Test the setup: the file should exist on disk and in the tree state.
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    let wc = ws.working_copy();
    assert!(wc.file_states().unwrap().contains_key(&ignored_path));

    // After we reset to the commit without the file, it should still exist on disk,
    // but it should not be in the tree state, and it should not get added when we
    // commit the working copy (because it's ignored).
    let mut locked_ws = ws.start_working_copy_mutation().unwrap();
    locked_ws.locked_wc().reset(&tree_without_file).unwrap();
    locked_ws.finish(op_id.clone()).unwrap();
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    let wc = ws.working_copy();
    assert!(!wc.file_states().unwrap().contains_key(&ignored_path));
    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(new_tree.id(), tree_without_file.id());

    // Now test the opposite direction: resetting to a commit where the file is
    // tracked. The file should become tracked (even though it's ignored).
    let ws = &mut test_workspace.workspace;
    let mut locked_ws = ws.start_working_copy_mutation().unwrap();
    locked_ws.locked_wc().reset(&tree_with_file).unwrap();
    locked_ws.finish(op_id.clone()).unwrap();
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    let wc = ws.working_copy();
    assert!(wc.file_states().unwrap().contains_key(&ignored_path));
    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(new_tree.id(), tree_with_file.id());
}

#[test]
fn test_checkout_discard() {
    // Start a mutation, do a checkout, and then discard the mutation. The working
    // copy files should remain changed, but the state files should not be
    // written.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = test_workspace.repo.clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let file1_path = RepoPath::from_internal_string("file1");
    let file2_path = RepoPath::from_internal_string("file2");

    let store = repo.store();
    let tree1 = create_tree(&repo, &[(&file1_path, "contents")]);
    let tree2 = create_tree(&repo, &[(&file2_path, "contents")]);

    let ws = &mut test_workspace.workspace;
    ws.check_out(repo.op_id().clone(), None, &tree1).unwrap();
    let state_path = ws.working_copy().state_path().to_path_buf();

    // Test the setup: the file should exist on disk and in the tree state.
    assert!(file1_path.to_fs_path(&workspace_root).is_file());
    let wc = ws.working_copy();
    assert!(wc.file_states().unwrap().contains_key(&file1_path));

    // Start a checkout
    let mut locked_ws = ws.start_working_copy_mutation().unwrap();
    locked_ws.locked_wc().check_out(&tree2).unwrap();
    // The change should be reflected in the working copy but not saved
    assert!(!file1_path.to_fs_path(&workspace_root).is_file());
    assert!(file2_path.to_fs_path(&workspace_root).is_file());
    let reloaded_wc =
        LocalWorkingCopy::load(store.clone(), workspace_root.clone(), state_path.clone());
    assert!(reloaded_wc.file_states().unwrap().contains_key(&file1_path));
    assert!(!reloaded_wc.file_states().unwrap().contains_key(&file2_path));
    drop(locked_ws);

    // The change should remain in the working copy, but not in memory and not saved
    let wc = ws.working_copy();
    assert!(wc.file_states().unwrap().contains_key(&file1_path));
    assert!(!wc.file_states().unwrap().contains_key(&file2_path));
    assert!(!file1_path.to_fs_path(&workspace_root).is_file());
    assert!(file2_path.to_fs_path(&workspace_root).is_file());
    let reloaded_wc = LocalWorkingCopy::load(store.clone(), workspace_root, state_path);
    assert!(reloaded_wc.file_states().unwrap().contains_key(&file1_path));
    assert!(!reloaded_wc.file_states().unwrap().contains_key(&file2_path));
}

#[test]
fn test_snapshot_racy_timestamps() {
    // Tests that file modifications are detected even if they happen the same
    // millisecond as the updated working copy state.
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let file_path = workspace_root.join("file");
    let mut previous_tree_id = repo.store().empty_merged_tree_id();
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
        let mut locked_ws = test_workspace
            .workspace
            .start_working_copy_mutation()
            .unwrap();
        let new_tree_id = locked_ws
            .locked_wc()
            .snapshot(SnapshotOptions::empty_for_test())
            .unwrap();
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
    let mut test_workspace = TestWorkspace::init(&settings);
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
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .unwrap();
    let tree_id = locked_ws
        .locked_wc()
        .snapshot(SnapshotOptions::empty_for_test())
        .unwrap();
    locked_ws.finish(OperationId::from_hex("abc123")).unwrap();
    let tree = store.get_root_tree(&tree_id).unwrap();
    // Only the regular files should be in the tree
    assert_eq!(
        tree.entries().map(|(path, _value)| path).collect_vec(),
        vec![file1_path.clone(), file2_path.clone()]
    );
    let wc = test_workspace.workspace.working_copy();
    assert_eq!(
        wc.file_states().unwrap().keys().cloned().collect_vec(),
        vec![file1_path, file2_path.clone()]
    );

    // Replace a regular file by a socket and snapshot the working copy again
    std::fs::remove_file(&file1_disk_path).unwrap();
    UnixListener::bind(&file1_disk_path).unwrap();
    let tree = test_workspace.snapshot().unwrap();
    // Only the regular file should be in the tree
    assert_eq!(
        tree.entries().map(|(path, _value)| path).collect_vec(),
        vec![file2_path.clone()]
    );
    let wc = test_workspace.workspace.working_copy();
    assert_eq!(
        wc.file_states().unwrap().keys().cloned().collect_vec(),
        vec![file2_path]
    );
}

#[test]
fn test_gitignores() {
    // Tests that .gitignore files are respected.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
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

    let tree1 = test_workspace.snapshot().unwrap();
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

    let tree2 = test_workspace.snapshot().unwrap();
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

#[test]
fn test_gitignores_in_ignored_dir() {
    // Tests that .gitignore files in an ignored directory are ignored, i.e. that
    // they cannot override the ignores from the parent

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let op_id = test_workspace.repo.op_id().clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    let nested_gitignore_path = RepoPath::from_internal_string("ignored/.gitignore");
    let ignored_path = RepoPath::from_internal_string("ignored/file");

    let tree1 = create_tree(&test_workspace.repo, &[(&gitignore_path, "ignored\n")]);
    let ws = &mut test_workspace.workspace;
    ws.check_out(op_id.clone(), None, &tree1).unwrap();

    testutils::write_working_copy_file(&workspace_root, &nested_gitignore_path, "!file\n");
    testutils::write_working_copy_file(&workspace_root, &ignored_path, "contents");

    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(
        new_tree.entries().collect_vec(),
        tree1.entries().collect_vec()
    );

    // The nested .gitignore is ignored even if it's tracked
    let tree2 = create_tree(
        &test_workspace.repo,
        &[
            (&gitignore_path, "ignored\n"),
            (&nested_gitignore_path, "!file\n"),
        ],
    );
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .unwrap();
    locked_ws.locked_wc().reset(&tree2).unwrap();
    locked_ws.finish(OperationId::from_hex("abc123")).unwrap();

    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(
        new_tree.entries().collect_vec(),
        tree2.entries().collect_vec()
    );
}

#[test]
fn test_gitignores_checkout_never_overwrites_ignored() {
    // Tests that a .gitignore'd file doesn't get overwritten if check out a commit
    // where the file is tracked.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Write an ignored file called "modified" to disk
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(&workspace_root, &gitignore_path, "modified\n");
    let modified_path = RepoPath::from_internal_string("modified");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "garbage");

    // Create a tree that adds the same file but with different contents
    let tree = create_tree(repo, &[(&modified_path, "contents")]);

    // Now check out the tree that adds the file "modified" with contents
    // "contents". The exiting contents ("garbage") shouldn't be replaced in the
    // working copy.
    let ws = &mut test_workspace.workspace;
    assert!(ws.check_out(repo.op_id().clone(), None, &tree).is_ok());

    // Check that the old contents are in the working copy
    let path = workspace_root.join("modified");
    assert!(path.is_file());
    assert_eq!(std::fs::read(&path).unwrap(), b"garbage");
}

#[test]
fn test_gitignores_ignored_directory_already_tracked() {
    // Tests that a .gitignore'd directory that already has a tracked file in it
    // does not get removed when snapshotting the working directory.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let workspace_root = test_workspace.workspace.workspace_root().clone();
    let repo = test_workspace.repo.clone();

    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    let unchanged_path = RepoPath::from_internal_string("ignored/unchanged");
    let modified_path = RepoPath::from_internal_string("ignored/modified");
    let deleted_path = RepoPath::from_internal_string("ignored/deleted");
    let tree = create_tree(
        &repo,
        &[
            (&gitignore_path, "/ignored/\n"),
            (&unchanged_path, "contents"),
            (&modified_path, "contents"),
            (&deleted_path, "contents"),
        ],
    );

    // Check out the tree with the files in `ignored/`
    let ws = &mut test_workspace.workspace;
    ws.check_out(repo.op_id().clone(), None, &tree).unwrap();

    // Make some changes inside the ignored directory and check that they are
    // detected when we snapshot. The files that are still there should not be
    // deleted from the resulting tree.
    std::fs::write(modified_path.to_fs_path(&workspace_root), "modified").unwrap();
    std::fs::remove_file(deleted_path.to_fs_path(&workspace_root)).unwrap();
    let new_tree = test_workspace.snapshot().unwrap();
    let expected_tree = create_tree(
        &repo,
        &[
            (&gitignore_path, "/ignored/\n"),
            (&unchanged_path, "contents"),
            (&modified_path, "modified"),
        ],
    );
    assert_eq!(
        new_tree.entries().collect_vec(),
        expected_tree.entries().collect_vec()
    );
}

#[test]
fn test_dotgit_ignored() {
    // Tests that .git directories and files are always ignored (we could accept
    // them if the backend is not git).

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let store = test_workspace.repo.store().clone();
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
    let new_tree = test_workspace.snapshot().unwrap();
    let empty_tree_id = store.empty_merged_tree_id();
    assert_eq!(new_tree.id(), empty_tree_id);
    std::fs::remove_dir_all(&dotgit_path).unwrap();

    // Test with a .git file
    testutils::write_working_copy_file(
        &workspace_root,
        &RepoPath::from_internal_string(".git"),
        "contents",
    );
    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(new_tree.id(), empty_tree_id);
}

#[test]
fn test_gitsubmodule() {
    // Tests that git submodules are ignored.

    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init_with_backend(&settings, TestRepoBackend::Git);
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

    let tree_id = MergedTreeId::Legacy(tree_builder.write_tree());
    let tree = store.get_root_tree(&tree_id).unwrap();
    let ws = &mut test_workspace.workspace;
    ws.check_out(repo.op_id().clone(), None, &tree).unwrap();

    std::fs::create_dir(submodule_path.to_fs_path(&workspace_root)).unwrap();

    testutils::write_working_copy_file(
        &workspace_root,
        &added_submodule_path,
        "i am a file in a submodule\n",
    );

    // Check that the files present in the submodule are not tracked
    // when we snapshot
    let new_tree = test_workspace.snapshot().unwrap();
    assert_eq!(new_tree.id(), tree_id);

    // Check that the files in the submodule are not deleted
    let file_in_submodule_path = added_submodule_path.to_fs_path(&workspace_root);
    assert!(
        file_in_submodule_path.metadata().is_ok(),
        "{file_in_submodule_path:?} should exist"
    );
}

#[cfg(unix)]
#[test]
fn test_existing_directory_symlink() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Creates a symlink in working directory, and a tree that will add a file under
    // the symlinked directory.
    std::os::unix::fs::symlink("..", workspace_root.join("parent")).unwrap();
    let file_path = RepoPath::from_internal_string("parent/escaped");
    let tree = create_tree(repo, &[(&file_path, "contents")]);

    // Checkout should fail because "parent" already exists and is a symlink.
    let ws = &mut test_workspace.workspace;
    assert!(ws.check_out(repo.op_id().clone(), None, &tree).is_err());

    // Therefore, "../escaped" shouldn't be created.
    assert!(!workspace_root.parent().unwrap().join("escaped").exists());
}

#[test]
fn test_fsmonitor() {
    let settings = testutils::user_settings();
    let mut test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let ws = &mut test_workspace.workspace;
    assert_eq!(
        ws.working_copy().sparse_patterns().unwrap(),
        vec![RepoPath::root()]
    );

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

    let snapshot = |locked_ws: &mut LockedWorkspace, paths: &[&RepoPath]| {
        let fs_paths = paths
            .iter()
            .map(|p| p.to_fs_path(&workspace_root))
            .collect();
        locked_ws
            .locked_wc()
            .snapshot(SnapshotOptions {
                fsmonitor_kind: Some(FsmonitorKind::Test {
                    changed_files: fs_paths,
                }),
                ..SnapshotOptions::empty_for_test()
            })
            .unwrap()
    };

    {
        let mut locked_ws = ws.start_working_copy_mutation().unwrap();
        let tree_id = snapshot(&mut locked_ws, &[]);
        assert_eq!(tree_id, repo.store().empty_merged_tree_id());
    }

    {
        let mut locked_ws = ws.start_working_copy_mutation().unwrap();
        let tree_id = snapshot(&mut locked_ws, &[&foo_path]);
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree d5e38c0a1b0ee5de47c5
          file "foo" (e99c2057c15160add351): "foo\n"
        "###);
    }

    {
        let mut locked_ws = ws.start_working_copy_mutation().unwrap();
        let tree_id = snapshot(
            &mut locked_ws,
            &[&foo_path, &bar_path, &nested_path, &ignored_path],
        );
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree f408c8d080414f8e90e1
          file "bar" (94cc973e7e1aefb7eff6): "bar\n"
          file "foo" (e99c2057c15160add351): "foo\n"
          file "path/to/nested" (6209060941cd770c8d46): "nested\n"
        "###);
        locked_ws.finish(repo.op_id().clone()).unwrap();
    }

    {
        testutils::write_working_copy_file(&workspace_root, &foo_path, "updated foo\n");
        testutils::write_working_copy_file(&workspace_root, &bar_path, "updated bar\n");
        let mut locked_ws = ws.start_working_copy_mutation().unwrap();
        let tree_id = snapshot(&mut locked_ws, &[&foo_path]);
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree e994a93c46f41dc91704
          file "bar" (94cc973e7e1aefb7eff6): "bar\n"
          file "foo" (e0fbd106147cc04ccd05): "updated foo\n"
          file "path/to/nested" (6209060941cd770c8d46): "nested\n"
        "###);
    }

    {
        std::fs::remove_file(foo_path.to_fs_path(&workspace_root)).unwrap();
        let mut locked_ws = ws.start_working_copy_mutation().unwrap();
        let tree_id = snapshot(&mut locked_ws, &[&foo_path]);
        insta::assert_snapshot!(testutils::dump_tree(repo.store(), &tree_id), @r###"
        tree 1df764981d4d74a4ecfa
          file "bar" (94cc973e7e1aefb7eff6): "bar\n"
          file "path/to/nested" (6209060941cd770c8d46): "nested\n"
        "###);
        locked_ws.finish(repo.op_id().clone()).unwrap();
    }
}

#[test]
fn test_snapshot_max_new_file_size() {
    let settings = UserSettings::from_config(
        testutils::base_config()
            .add_source(config::File::from_str(
                "snapshot.max-new-file-size = \"1KiB\"",
                config::FileFormat::Toml,
            ))
            .build()
            .unwrap(),
    );
    let mut test_workspace = TestWorkspace::init(&settings);
    let workspace_root = test_workspace.workspace.workspace_root().clone();
    let small_path = RepoPath::from_internal_string("small");
    let large_path = RepoPath::from_internal_string("large");
    std::fs::write(small_path.to_fs_path(&workspace_root), vec![0; 1024]).unwrap();
    test_workspace
        .snapshot()
        .expect("files exactly matching the size limit should succeed");
    std::fs::write(small_path.to_fs_path(&workspace_root), vec![0; 1024 + 1]).unwrap();
    test_workspace
        .snapshot()
        .expect("existing files may grow beyond the size limit");
    // A new file of 1KiB + 1 bytes should fail
    std::fs::write(large_path.to_fs_path(&workspace_root), vec![0; 1024 + 1]).unwrap();
    let err = test_workspace
        .snapshot()
        .expect_err("new files beyond the size limit should fail");
    assert!(
        matches!(err, SnapshotError::NewFileTooLarge { .. }),
        "the failure should be attributed to new file size"
    );
}
