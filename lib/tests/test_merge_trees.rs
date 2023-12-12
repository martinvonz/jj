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

use itertools::Itertools;
use jj_lib::backend::TreeValue;
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathComponent};
use jj_lib::rewrite::rebase_commit;
use jj_lib::tree::{merge_trees, Tree};
use testutils::{create_single_tree, create_tree, TestRepo};

#[test]
fn test_same_type() {
    // Tests all possible cases where the entry type is unchanged, specifically
    // using only normal files in all trees (no symlinks, no trees, etc.).

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
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
        for &path in &files {
            let contents = &path[index..][..1];
            if contents != "_" {
                testutils::write_normal_file(
                    &mut tree_builder,
                    RepoPath::from_internal_string(path),
                    contents,
                );
            }
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(RepoPath::root(), &tree_id).unwrap()
    };

    let base_tree = write_tree(0);
    let side1_tree = write_tree(1);
    let side2_tree = write_tree(2);

    // Create the merged tree
    let merged_tree = merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();

    // Check that we have exactly the paths we expect in the merged tree
    let names = merged_tree
        .entries_non_recursive()
        .map(|entry| entry.name().as_str())
        .collect_vec();
    assert_eq!(
        names,
        vec!["__a", "_a_", "_aa", "_ab", "a_b", "aaa", "aab", "ab_", "aba", "abb", "abc",]
    );

    // Check that the simple, non-conflicting cases were resolved correctly
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("__a")),
        side2_tree.value(RepoPathComponent::new("__a"))
    );
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("_a_")),
        side1_tree.value(RepoPathComponent::new("_a_"))
    );
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("_aa")),
        side1_tree.value(RepoPathComponent::new("_aa"))
    );
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("aaa")),
        side1_tree.value(RepoPathComponent::new("aaa"))
    );
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("aab")),
        side2_tree.value(RepoPathComponent::new("aab"))
    );
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("aba")),
        side1_tree.value(RepoPathComponent::new("aba"))
    );
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("abb")),
        side1_tree.value(RepoPathComponent::new("abb"))
    );

    // Check the conflicting cases
    let component = RepoPathComponent::new("_ab");
    match merged_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(RepoPath::from_internal_string("_ab"), id)
                .unwrap();
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![side1_tree.value(component), side2_tree.value(component)]
            );
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![None]
            );
        }
        _ => panic!("unexpected value"),
    };
    let component = RepoPathComponent::new("a_b");
    match merged_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(RepoPath::from_internal_string("a_b"), id)
                .unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![side2_tree.value(component), None]
            );
        }
        _ => panic!("unexpected value"),
    };
    let component = RepoPathComponent::new("ab_");
    match merged_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(RepoPath::from_internal_string("ab_"), id)
                .unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![side1_tree.value(component), None]
            );
        }
        _ => panic!("unexpected value"),
    };
    let component = RepoPathComponent::new("abc");
    match merged_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(RepoPath::from_internal_string("abc"), id)
                .unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![side1_tree.value(component), side2_tree.value(component)]
            );
        }
        _ => panic!("unexpected value"),
    };
}

#[test]
fn test_executable() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    // The file name encodes whether the file was executable or normal in the base
    // and in each side
    let files = vec!["nnn", "nnx", "nxn", "nxx", "xnn", "xnx", "xxn", "xxx"];

    let write_tree = |files: &[(&str, bool)]| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for &(path, executable) in files {
            let repo_path = RepoPath::from_internal_string(path);
            if executable {
                testutils::write_executable_file(&mut tree_builder, repo_path, "contents");
            } else {
                testutils::write_normal_file(&mut tree_builder, repo_path, "contents");
            }
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(RepoPath::root(), &tree_id).unwrap()
    };

    fn contents_in_tree<'a>(files: &[&'a str], index: usize) -> Vec<(&'a str, bool)> {
        files
            .iter()
            .map(|f| (*f, &f[index..][..1] == "x"))
            .collect()
    }

    let base_tree = write_tree(&contents_in_tree(&files, 0));
    let side1_tree = write_tree(&contents_in_tree(&files, 1));
    let side2_tree = write_tree(&contents_in_tree(&files, 2));

    // Create the merged tree
    let merged_tree = merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();

    // Check that the merged tree has the correct executable bits
    let norm = base_tree.value(RepoPathComponent::new("nnn"));
    let exec = base_tree.value(RepoPathComponent::new("xxx"));
    assert_eq!(merged_tree.value(RepoPathComponent::new("nnn")), norm);
    assert_eq!(merged_tree.value(RepoPathComponent::new("nnx")), exec);
    assert_eq!(merged_tree.value(RepoPathComponent::new("nxn")), exec);
    assert_eq!(merged_tree.value(RepoPathComponent::new("nxx")), exec);
    assert_eq!(merged_tree.value(RepoPathComponent::new("xnn")), norm);
    assert_eq!(merged_tree.value(RepoPathComponent::new("xnx")), norm);
    assert_eq!(merged_tree.value(RepoPathComponent::new("xxn")), norm);
    assert_eq!(merged_tree.value(RepoPathComponent::new("xxx")), exec);
}

#[test]
fn test_subtrees() {
    // Tests that subtrees are merged.

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let write_tree = |paths: Vec<&str>| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for path in paths {
            testutils::write_normal_file(
                &mut tree_builder,
                RepoPath::from_internal_string(path),
                &format!("contents of {path:?}"),
            );
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(RepoPath::root(), &tree_id).unwrap()
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

    let merged_tree = merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();
    let entries = merged_tree.entries().collect_vec();

    let expected_tree = write_tree(vec![
        "f1",
        "f2",
        "d1/f1",
        "d1/f2",
        "d1/d1/f1",
        "d1/d1/d1/f1",
        "d1/d1/d1/f2",
    ]);
    let expected_entries = expected_tree.entries().collect_vec();
    assert_eq!(entries, expected_entries);
}

#[test]
fn test_subtree_becomes_empty() {
    // Tests that subtrees that become empty are removed from the parent tree.

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let write_tree = |paths: Vec<&str>| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for path in paths {
            testutils::write_normal_file(
                &mut tree_builder,
                RepoPath::from_internal_string(path),
                &format!("contents of {path:?}"),
            );
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(RepoPath::root(), &tree_id).unwrap()
    };

    let base_tree = write_tree(vec!["f1", "d1/f1", "d1/d1/d1/f1", "d1/d1/d1/f2"]);
    let side1_tree = write_tree(vec!["f1", "d1/f1", "d1/d1/d1/f1"]);
    let side2_tree = write_tree(vec!["d1/d1/d1/f2"]);

    let merged_tree = merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();
    assert_eq!(merged_tree.id(), store.empty_tree_id());
}

#[test]
fn test_subtree_one_missing() {
    // Tests that merging trees where one side is missing is resolved as if the
    // missing side was empty.
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let write_tree = |paths: Vec<&str>| -> Tree {
        let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
        for path in paths {
            testutils::write_normal_file(
                &mut tree_builder,
                RepoPath::from_internal_string(path),
                &format!("contents of {path:?}"),
            );
        }
        let tree_id = tree_builder.write_tree();
        store.get_tree(RepoPath::root(), &tree_id).unwrap()
    };

    let tree1 = write_tree(vec![]);
    let tree2 = write_tree(vec!["d1/f1"]);
    let tree3 = write_tree(vec!["d1/f1", "d1/f2"]);

    // The two sides add different trees
    let merged_tree = merge_trees(&tree2, &tree1, &tree3).unwrap();
    let expected_entries = write_tree(vec!["d1/f1", "d1/f2"]).entries().collect_vec();
    assert_eq!(merged_tree.entries().collect_vec(), expected_entries);
    // Same tree other way
    let reverse_merged_tree = merge_trees(&tree3, &tree1, &tree2).unwrap();
    assert_eq!(reverse_merged_tree.id(), merged_tree.id());

    // One side removes, the other side modifies
    let merged_tree = merge_trees(&tree1, &tree2, &tree3).unwrap();
    let expected_entries = write_tree(vec!["d1/f2"]).entries().collect_vec();
    assert_eq!(merged_tree.entries().collect_vec(), expected_entries);
    // Same tree other way
    let reverse_merged_tree = merge_trees(&tree3, &tree2, &tree1).unwrap();
    assert_eq!(reverse_merged_tree.id(), merged_tree.id());
}

#[test]
fn test_types() {
    // Tests conflicts between different types. This is mostly to test that the
    // conflicts survive the roundtrip to the store.

    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let mut base_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut side1_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut side2_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    testutils::write_normal_file(
        &mut base_tree_builder,
        RepoPath::from_internal_string("normal_executable_symlink"),
        "contents",
    );
    testutils::write_executable_file(
        &mut side1_tree_builder,
        RepoPath::from_internal_string("normal_executable_symlink"),
        "contents",
    );
    testutils::write_symlink(
        &mut side2_tree_builder,
        RepoPath::from_internal_string("normal_executable_symlink"),
        "contents",
    );
    let tree_id = store.empty_tree_id().clone();
    base_tree_builder.set(
        RepoPath::from_internal_string("tree_normal_symlink").to_owned(),
        TreeValue::Tree(tree_id),
    );
    testutils::write_normal_file(
        &mut side1_tree_builder,
        RepoPath::from_internal_string("tree_normal_symlink"),
        "contents",
    );
    testutils::write_symlink(
        &mut side2_tree_builder,
        RepoPath::from_internal_string("tree_normal_symlink"),
        "contents",
    );
    let base_tree_id = base_tree_builder.write_tree();
    let base_tree = store.get_tree(RepoPath::root(), &base_tree_id).unwrap();
    let side1_tree_id = side1_tree_builder.write_tree();
    let side1_tree = store.get_tree(RepoPath::root(), &side1_tree_id).unwrap();
    let side2_tree_id = side2_tree_builder.write_tree();
    let side2_tree = store.get_tree(RepoPath::root(), &side2_tree_id).unwrap();

    // Created the merged tree
    let merged_tree = merge_trees(&side1_tree, &base_tree, &side2_tree).unwrap();

    // Check the conflicting cases
    let component = RepoPathComponent::new("normal_executable_symlink");
    match merged_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(
                    RepoPath::from_internal_string("normal_executable_symlink"),
                    id,
                )
                .unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![side1_tree.value(component), side2_tree.value(component)]
            );
        }
        _ => panic!("unexpected value"),
    };
    let component = RepoPathComponent::new("tree_normal_symlink");
    match merged_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(RepoPath::from_internal_string("tree_normal_symlink"), id)
                .unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![side1_tree.value(component), side2_tree.value(component)]
            );
        }
        _ => panic!("unexpected value"),
    };
}

#[test]
fn test_simplify_conflict() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let component = RepoPathComponent::new("file");
    let path = RepoPath::from_internal_string("file");
    let write_tree = |contents: &str| -> Tree { create_single_tree(repo, &[(path, contents)]) };

    let base_tree = write_tree("base contents");
    let branch_tree = write_tree("branch contents");
    let upstream1_tree = write_tree("upstream1 contents");
    let upstream2_tree = write_tree("upstream2 contents");

    // Rebase the branch tree to the first upstream tree
    let rebased1_tree = merge_trees(&branch_tree, &base_tree, &upstream1_tree).unwrap();
    // Make sure we have a conflict (testing the test setup)
    match rebased1_tree.value(component).unwrap() {
        TreeValue::Conflict(_) => {
            // expected
        }
        _ => panic!("unexpected value"),
    };

    // Rebase the rebased tree back to the base. The conflict should be gone. Try
    // both directions.
    let rebased_back_tree = merge_trees(&rebased1_tree, &upstream1_tree, &base_tree).unwrap();
    assert_eq!(
        rebased_back_tree.value(component),
        branch_tree.value(component)
    );
    let rebased_back_tree = merge_trees(&base_tree, &upstream1_tree, &rebased1_tree).unwrap();
    assert_eq!(
        rebased_back_tree.value(component),
        branch_tree.value(component)
    );

    // Rebase the rebased tree further upstream. The conflict should be simplified
    // to not mention the contents from the first rebase.
    let further_rebased_tree =
        merge_trees(&rebased1_tree, &upstream1_tree, &upstream2_tree).unwrap();
    match further_rebased_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store
                .read_conflict(RepoPath::from_internal_string(component.as_str()), id)
                .unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![
                    branch_tree.value(component),
                    upstream2_tree.value(component),
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
    let further_rebased_tree =
        merge_trees(&upstream2_tree, &upstream1_tree, &rebased1_tree).unwrap();
    match further_rebased_tree.value(component).unwrap() {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(path, id).unwrap();
            assert_eq!(
                conflict.removes().map(|v| v.as_ref()).collect_vec(),
                vec![base_tree.value(component)]
            );
            assert_eq!(
                conflict.adds().map(|v| v.as_ref()).collect_vec(),
                vec![
                    upstream2_tree.value(component),
                    branch_tree.value(component)
                ]
            );
        }
        _ => panic!("unexpected value"),
    };
}

#[test]
fn test_simplify_conflict_after_resolving_parent() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Set up a repo like this:
    // D
    // | C
    // | B
    // |/
    // A
    //
    // Commit A has a file with 3 lines. B and D make conflicting changes to the
    // first line. C changes the third line. We then rebase B and C onto D,
    // which creates a conflict. We resolve the conflict in the first line and
    // rebase C2 (the rebased C) onto the resolved conflict. C3 should not have
    // a conflict since it changed an unrelated line.
    let path = RepoPath::from_internal_string("dir/file");
    let mut tx = repo.start_transaction(&settings);
    let tree_a = create_tree(repo, &[(path, "abc\ndef\nghi\n")]);
    let commit_a = tx
        .mut_repo()
        .new_commit(
            &settings,
            vec![repo.store().root_commit_id().clone()],
            tree_a.id(),
        )
        .write()
        .unwrap();
    let tree_b = create_tree(repo, &[(path, "Abc\ndef\nghi\n")]);
    let commit_b = tx
        .mut_repo()
        .new_commit(&settings, vec![commit_a.id().clone()], tree_b.id())
        .write()
        .unwrap();
    let tree_c = create_tree(repo, &[(path, "Abc\ndef\nGhi\n")]);
    let commit_c = tx
        .mut_repo()
        .new_commit(&settings, vec![commit_b.id().clone()], tree_c.id())
        .write()
        .unwrap();
    let tree_d = create_tree(repo, &[(path, "abC\ndef\nghi\n")]);
    let commit_d = tx
        .mut_repo()
        .new_commit(&settings, vec![commit_a.id().clone()], tree_d.id())
        .write()
        .unwrap();

    let commit_b2 = rebase_commit(&settings, tx.mut_repo(), &commit_b, &[commit_d]).unwrap();
    let commit_c2 =
        rebase_commit(&settings, tx.mut_repo(), &commit_c, &[commit_b2.clone()]).unwrap();

    // Test the setup: Both B and C should have conflicts.
    let tree_b2 = commit_b2.tree().unwrap();
    let tree_c2 = commit_b2.tree().unwrap();
    assert!(!tree_b2.path_value(path).is_resolved());
    assert!(!tree_c2.path_value(path).is_resolved());

    // Create the resolved B and rebase C on top.
    let tree_b3 = create_tree(repo, &[(path, "AbC\ndef\nghi\n")]);
    let commit_b3 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b2)
        .set_tree_id(tree_b3.id())
        .write()
        .unwrap();
    let commit_c3 = rebase_commit(&settings, tx.mut_repo(), &commit_c2, &[commit_b3]).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit("test");

    // The conflict should now be resolved.
    let tree_c2 = commit_c3.tree().unwrap();
    let resolved_value = tree_c2.path_value(path);
    match resolved_value.into_resolved() {
        Ok(Some(TreeValue::File {
            id,
            executable: false,
        })) => {
            assert_eq!(
                testutils::read_file(repo.store(), path, &id),
                b"AbC\ndef\nGhi\n"
            );
        }
        other => {
            panic!("unexpected value: {other:#?}");
        }
    }
}

// TODO: Add tests for simplification of multi-way conflicts. Both the content
// and the executable bit need testing.
