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

use jujube_lib::repo_path::{DirRepoPath, FileRepoPath, RepoPath};
use jujube_lib::store::{ConflictPart, TreeValue};
use jujube_lib::testutils;
use jujube_lib::tree::Tree;
use jujube_lib::trees;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_same_type(use_git: bool) {
    // Tests all possible cases where the entry type is unchanged, specifically
    // using only normal files in all trees (no symlinks, no trees, etc.).

    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    // The file name encodes the state in the base and in each side ("_" means
    // missing)
    let files = vec![
        "__a", // side 2 added
        "_a_", // side 1 added
        "_aa", // both sides added, same content
        "_ab", // both sides added, different content
        "a__", // both sides removed
        "a_a", // side 1 removed
        "a_b", // side 1 removed, side 2 modified
        "aa_", // side 2 removed
        "aaa", // no changes
        "aab", // side 2 modified
        "ab_", // side 1 modified, side 2 removed
        "aba", // side 1 modified
        "abb", // both sides modified, same content
        "abc", // both sides modified, different content
    ];

    let write_tree = |index: usize| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for path in &files {
            let contents = &path[index..index + 1];
            if contents != "_" {
                testutils::write_normal_file(
                    &mut tree_builder,
                    &FileRepoPath::from(*path),
                    contents,
                );
            }
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(&DirRepoPath::root(), &tree_id).unwrap()
    };

    let base_tree = write_tree(0);
    let side1_tree = write_tree(1);
    let side2_tree = write_tree(2);

    // Create the merged tree
    let merged_tree_id = trees::merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();
    let merged_tree = store
        .get_tree(&DirRepoPath::root(), &merged_tree_id)
        .unwrap();

    // Check that we have exactly the paths we expect in the merged tree
    let names: Vec<&str> = merged_tree
        .entries_non_recursive()
        .map(|entry| entry.name().as_ref())
        .collect();
    assert_eq!(
        names,
        vec!["__a", "_a_", "_aa", "_ab", "a_b", "aaa", "aab", "ab_", "aba", "abb", "abc",]
    );

    // Check that the simple, non-conflicting cases were resolved correctly
    assert_eq!(merged_tree.value("__a"), side2_tree.value("__a"));
    assert_eq!(merged_tree.value("_a_"), side1_tree.value("_a_"));
    assert_eq!(merged_tree.value("_aa"), side1_tree.value("_aa"));
    assert_eq!(merged_tree.value("aaa"), side1_tree.value("aaa"));
    assert_eq!(merged_tree.value("aab"), side2_tree.value("aab"));
    assert_eq!(merged_tree.value("aba"), side1_tree.value("aba"));
    assert_eq!(merged_tree.value("abb"), side1_tree.value("abb"));

    // Check the conflicting cases
    match merged_tree.value("_ab").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.adds,
                vec![
                    ConflictPart {
                        value: side1_tree.value("_ab").cloned().unwrap()
                    },
                    ConflictPart {
                        value: side2_tree.value("_ab").cloned().unwrap()
                    }
                ]
            );
            assert!(conflict.removes.is_empty());
        }
        _ => panic!("unexpected value"),
    };
    match merged_tree.value("a_b").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree.value("a_b").cloned().unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![ConflictPart {
                    value: side2_tree.value("a_b").cloned().unwrap()
                }]
            );
        }
        _ => panic!("unexpected value"),
    };
    match merged_tree.value("ab_").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree.value("ab_").cloned().unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![ConflictPart {
                    value: side1_tree.value("ab_").cloned().unwrap()
                }]
            );
        }
        _ => panic!("unexpected value"),
    };
    match merged_tree.value("abc").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree.value("abc").cloned().unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![
                    ConflictPart {
                        value: side1_tree.value("abc").cloned().unwrap()
                    },
                    ConflictPart {
                        value: side2_tree.value("abc").cloned().unwrap()
                    }
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_subtrees(use_git: bool) {
    // Tests that subtrees are merged.

    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let write_tree = |paths: Vec<&str>| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for path in paths {
            testutils::write_normal_file(
                &mut tree_builder,
                &FileRepoPath::from(path),
                &format!("contents of {:?}", path),
            );
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(&DirRepoPath::root(), &tree_id).unwrap()
    };

    let base_tree = write_tree(vec!["f1", "d1/f1", "d1/d1/f1", "d1/d1/d1/f1"]);
    let side1_tree = write_tree(vec![
        "f1",
        "f2",
        "d1/f1",
        "d1/f2",
        "d1/d1/f1",
        "d1/d1/d1/f1",
    ]);
    let side2_tree = write_tree(vec![
        "f1",
        "d1/f1",
        "d1/d1/f1",
        "d1/d1/d1/f1",
        "d1/d1/d1/f2",
    ]);

    let merged_tree_id = trees::merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();
    let merged_tree = store
        .get_tree(&DirRepoPath::root(), &merged_tree_id)
        .unwrap();
    let entries: Vec<_> = merged_tree.entries().collect();

    let expected_tree = write_tree(vec![
        "f1",
        "f2",
        "d1/f1",
        "d1/f2",
        "d1/d1/f1",
        "d1/d1/d1/f1",
        "d1/d1/d1/f2",
    ]);
    let expected_entries: Vec<_> = expected_tree.entries().collect();
    assert_eq!(entries, expected_entries);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_subtree_becomes_empty(use_git: bool) {
    // Tests that subtrees that become empty are removed from the parent tree.

    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let write_tree = |paths: Vec<&str>| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for path in paths {
            testutils::write_normal_file(
                &mut tree_builder,
                &FileRepoPath::from(path),
                &format!("contents of {:?}", path),
            );
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(&DirRepoPath::root(), &tree_id).unwrap()
    };

    let base_tree = write_tree(vec!["f1", "d1/f1", "d1/d1/d1/f1", "d1/d1/d1/f2"]);
    let side1_tree = write_tree(vec!["f1", "d1/f1", "d1/d1/d1/f1"]);
    let side2_tree = write_tree(vec!["d1/d1/d1/f2"]);

    let merged_tree_id = trees::merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();
    let merged_tree = store
        .get_tree(&DirRepoPath::root(), &merged_tree_id)
        .unwrap();
    assert_eq!(merged_tree.id(), store.empty_tree_id());
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_types(use_git: bool) {
    // Tests conflicts between different types. This is mostly to test that the
    // conflicts survive the roundtrip to the store.

    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let mut base_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut side1_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut side2_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    testutils::write_normal_file(
        &mut base_tree_builder,
        &FileRepoPath::from("normal_executable_symlink"),
        "contents",
    );
    testutils::write_executable_file(
        &mut side1_tree_builder,
        &FileRepoPath::from("normal_executable_symlink"),
        "contents",
    );
    testutils::write_symlink(
        &mut side2_tree_builder,
        &FileRepoPath::from("normal_executable_symlink"),
        "contents",
    );
    let tree_id = store.empty_tree_id().clone();
    base_tree_builder.set(
        RepoPath::from("tree_normal_symlink"),
        TreeValue::Tree(tree_id),
    );
    testutils::write_normal_file(
        &mut side1_tree_builder,
        &FileRepoPath::from("tree_normal_symlink"),
        "contents",
    );
    testutils::write_symlink(
        &mut side2_tree_builder,
        &FileRepoPath::from("tree_normal_symlink"),
        "contents",
    );
    let base_tree_id = base_tree_builder.write_tree();
    let base_tree = store.get_tree(&DirRepoPath::root(), &base_tree_id).unwrap();
    let side1_tree_id = side1_tree_builder.write_tree();
    let side1_tree = store
        .get_tree(&DirRepoPath::root(), &side1_tree_id)
        .unwrap();
    let side2_tree_id = side2_tree_builder.write_tree();
    let side2_tree = store
        .get_tree(&DirRepoPath::root(), &side2_tree_id)
        .unwrap();

    // Created the merged tree
    let merged_tree_id = trees::merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();
    let merged_tree = store
        .get_tree(&DirRepoPath::root(), &merged_tree_id)
        .unwrap();

    // Check the conflicting cases
    match merged_tree.value("normal_executable_symlink").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(&id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree
                        .value("normal_executable_symlink")
                        .cloned()
                        .unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![
                    ConflictPart {
                        value: side1_tree
                            .value("normal_executable_symlink")
                            .cloned()
                            .unwrap()
                    },
                    ConflictPart {
                        value: side2_tree
                            .value("normal_executable_symlink")
                            .cloned()
                            .unwrap()
                    },
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
    match merged_tree.value("tree_normal_symlink").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree.value("tree_normal_symlink").cloned().unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![
                    ConflictPart {
                        value: side1_tree.value("tree_normal_symlink").cloned().unwrap()
                    },
                    ConflictPart {
                        value: side2_tree.value("tree_normal_symlink").cloned().unwrap()
                    },
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_simplify_conflict(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();

    let write_tree = |contents: &str| -> Tree {
        testutils::create_tree(&repo, &[(&FileRepoPath::from("file"), contents)])
    };

    let base_tree = write_tree("base contents");
    let branch_tree = write_tree("branch contents");
    let upstream1_tree = write_tree("upstream1 contents");
    let upstream2_tree = write_tree("upstream2 contents");

    let merge_trees = |base: &Tree, side1: &Tree, side2: &Tree| -> Tree {
        let tree_id = trees::merge_trees(&side1, &base, &side2).unwrap();
        store.get_tree(&DirRepoPath::root(), &tree_id).unwrap()
    };

    // Rebase the branch tree to the first upstream tree
    let rebased1_tree = merge_trees(&base_tree, &branch_tree, &upstream1_tree);
    // Make sure we have a conflict (testing the test setup)
    match rebased1_tree.value("file").unwrap() {
        TreeValue::Conflict(_) => {
            // expected
        }
        _ => panic!("unexpected value"),
    };

    // Rebase the rebased tree back to the base. The conflict should be gone. Try
    // both directions.
    let rebased_back_tree = merge_trees(&upstream1_tree, &rebased1_tree, &base_tree);
    assert_eq!(rebased_back_tree.value("file"), branch_tree.value("file"));
    let rebased_back_tree = merge_trees(&upstream1_tree, &base_tree, &rebased1_tree);
    assert_eq!(rebased_back_tree.value("file"), branch_tree.value("file"));

    // Rebase the rebased tree further upstream. The conflict should be simplified
    // to not mention the contents from the first rebase.
    let further_rebased_tree = merge_trees(&upstream1_tree, &rebased1_tree, &upstream2_tree);
    match further_rebased_tree.value("file").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree.value("file").cloned().unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![
                    ConflictPart {
                        value: branch_tree.value("file").cloned().unwrap()
                    },
                    ConflictPart {
                        value: upstream2_tree.value("file").cloned().unwrap()
                    },
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
    let further_rebased_tree = merge_trees(&upstream1_tree, &upstream2_tree, &rebased1_tree);
    match further_rebased_tree.value("file").unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id).unwrap();
            assert_eq!(
                conflict.removes,
                vec![ConflictPart {
                    value: base_tree.value("file").cloned().unwrap()
                }]
            );
            assert_eq!(
                conflict.adds,
                vec![
                    ConflictPart {
                        value: upstream2_tree.value("file").cloned().unwrap()
                    },
                    ConflictPart {
                        value: branch_tree.value("file").cloned().unwrap()
                    },
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
}
