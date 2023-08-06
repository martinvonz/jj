// Copyright 2023 The Jujutsu Authors
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
use jj_lib::backend::{FileId, TreeValue};
use jj_lib::merge::Merge;
use jj_lib::merged_tree::{MergedTree, MergedTreeValue};
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use jj_lib::tree::merge_trees;
use testutils::{write_file, write_normal_file, TestRepo};

fn file_value(file_id: &FileId) -> TreeValue {
    TreeValue::File {
        id: file_id.clone(),
        executable: false,
    }
}

#[test]
fn test_from_legacy_tree() {
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let store = repo.store();

    let mut tree_builder = store.tree_builder(repo.store().empty_tree_id().clone());

    // file1: regular file without conflicts
    let file1_path = RepoPath::from_internal_string("no_conflict");
    let file1_id = write_normal_file(&mut tree_builder, &file1_path, "foo");

    // file2: 3-way conflict
    let file2_path = RepoPath::from_internal_string("3way");
    let file2_v1_id = write_file(store.as_ref(), &file2_path, "file2_v1");
    let file2_v2_id = write_file(store.as_ref(), &file2_path, "file2_v2");
    let file2_v3_id = write_file(store.as_ref(), &file2_path, "file2_v3");
    let file2_conflict = Merge::new(
        vec![Some(file_value(&file2_v1_id))],
        vec![
            Some(file_value(&file2_v2_id)),
            Some(file_value(&file2_v3_id)),
        ],
    );
    let file2_conflict_id = store.write_conflict(&file2_path, &file2_conflict).unwrap();
    tree_builder.set(file2_path.clone(), TreeValue::Conflict(file2_conflict_id));

    // file3: modify/delete conflict
    let file3_path = RepoPath::from_internal_string("modify_delete");
    let file3_v1_id = write_file(store.as_ref(), &file3_path, "file3_v1");
    let file3_v2_id = write_file(store.as_ref(), &file3_path, "file3_v2");
    let file3_conflict = Merge::new(
        vec![Some(file_value(&file3_v1_id))],
        vec![Some(file_value(&file3_v2_id)), None],
    );
    let file3_conflict_id = store.write_conflict(&file3_path, &file3_conflict).unwrap();
    tree_builder.set(file3_path.clone(), TreeValue::Conflict(file3_conflict_id));

    // file4: add/add conflict
    let file4_path = RepoPath::from_internal_string("add_add");
    let file4_v1_id = write_file(store.as_ref(), &file4_path, "file4_v1");
    let file4_v2_id = write_file(store.as_ref(), &file4_path, "file4_v2");
    let file4_conflict = Merge::new(
        vec![None],
        vec![
            Some(file_value(&file4_v1_id)),
            Some(file_value(&file4_v2_id)),
        ],
    );
    let file4_conflict_id = store.write_conflict(&file4_path, &file4_conflict).unwrap();
    tree_builder.set(file4_path.clone(), TreeValue::Conflict(file4_conflict_id));

    // file5: 5-way conflict
    let file5_path = RepoPath::from_internal_string("5way");
    let file5_v1_id = write_file(store.as_ref(), &file5_path, "file5_v1");
    let file5_v2_id = write_file(store.as_ref(), &file5_path, "file5_v2");
    let file5_v3_id = write_file(store.as_ref(), &file5_path, "file5_v3");
    let file5_v4_id = write_file(store.as_ref(), &file5_path, "file5_v4");
    let file5_v5_id = write_file(store.as_ref(), &file5_path, "file5_v5");
    let file5_conflict = Merge::new(
        vec![
            Some(file_value(&file5_v1_id)),
            Some(file_value(&file5_v2_id)),
        ],
        vec![
            Some(file_value(&file5_v3_id)),
            Some(file_value(&file5_v4_id)),
            Some(file_value(&file5_v5_id)),
        ],
    );
    let file5_conflict_id = store.write_conflict(&file5_path, &file5_conflict).unwrap();
    tree_builder.set(file5_path.clone(), TreeValue::Conflict(file5_conflict_id));

    // dir1: directory without conflicts
    let dir1_basename = RepoPathComponent::from("dir1");
    write_normal_file(
        &mut tree_builder,
        &RepoPath::root()
            .join(&dir1_basename)
            .join(&RepoPathComponent::from("file")),
        "foo",
    );

    let tree_id = tree_builder.write_tree();
    let tree = store.get_tree(&RepoPath::root(), &tree_id).unwrap();

    let merged_tree = MergedTree::from_legacy_tree(tree.clone());
    assert_eq!(
        merged_tree.value(&RepoPathComponent::from("missing")),
        MergedTreeValue::Resolved(None)
    );
    // file1: regular file without conflicts
    assert_eq!(
        merged_tree.value(&file1_path.components()[0]),
        MergedTreeValue::Resolved(Some(&TreeValue::File {
            id: file1_id,
            executable: false,
        }))
    );
    // file2: 3-way conflict
    assert_eq!(
        merged_tree.value(&file2_path.components()[0]),
        MergedTreeValue::Conflict(Merge::new(
            vec![Some(file_value(&file2_v1_id)), None],
            vec![
                Some(file_value(&file2_v2_id)),
                Some(file_value(&file2_v3_id)),
                None,
            ],
        ))
    );
    // file3: modify/delete conflict
    assert_eq!(
        merged_tree.value(&file3_path.components()[0]),
        MergedTreeValue::Conflict(Merge::new(
            vec![Some(file_value(&file3_v1_id)), None],
            vec![Some(file_value(&file3_v2_id)), None, None],
        ))
    );
    // file4: add/add conflict
    assert_eq!(
        merged_tree.value(&file4_path.components()[0]),
        MergedTreeValue::Conflict(Merge::new(
            vec![None, None],
            vec![
                Some(file_value(&file4_v1_id)),
                Some(file_value(&file4_v2_id)),
                None
            ],
        ))
    );
    // file5: 5-way conflict
    assert_eq!(
        merged_tree.value(&file5_path.components()[0]),
        MergedTreeValue::Conflict(Merge::new(
            vec![
                Some(file_value(&file5_v1_id)),
                Some(file_value(&file5_v2_id)),
            ],
            vec![
                Some(file_value(&file5_v3_id)),
                Some(file_value(&file5_v4_id)),
                Some(file_value(&file5_v5_id)),
            ],
        ))
    );
    // file6: directory without conflicts
    assert_eq!(
        merged_tree.value(&dir1_basename),
        MergedTreeValue::Resolved(tree.value(&dir1_basename))
    );
}

#[test]
fn test_resolve_success() {
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    let unchanged_path = RepoPath::from_internal_string("unchanged");
    let trivial_file_path = RepoPath::from_internal_string("trivial-file");
    let trivial_hunk_path = RepoPath::from_internal_string("trivial-hunk");
    let both_added_dir_path = RepoPath::from_internal_string("added-dir");
    let both_added_dir_file1_path = both_added_dir_path.join(&RepoPathComponent::from("file1"));
    let both_added_dir_file2_path = both_added_dir_path.join(&RepoPathComponent::from("file2"));
    let emptied_dir_path = RepoPath::from_internal_string("to-become-empty");
    let emptied_dir_file1_path = emptied_dir_path.join(&RepoPathComponent::from("file1"));
    let emptied_dir_file2_path = emptied_dir_path.join(&RepoPathComponent::from("file2"));
    let base1 = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_file_path, "base1"),
            (&trivial_hunk_path, "line1\nline2\nline3\n"),
            (&emptied_dir_file1_path, "base1"),
            (&emptied_dir_file2_path, "base1"),
        ],
    );
    let side1 = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_file_path, "base1"),
            (&trivial_hunk_path, "line1 side1\nline2\nline3\n"),
            (&both_added_dir_file1_path, "side1"),
            (&emptied_dir_file2_path, "base1"),
        ],
    );
    let side2 = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_file_path, "side2"),
            (&trivial_hunk_path, "line1\nline2\nline3 side2\n"),
            (&both_added_dir_file2_path, "side2"),
            (&emptied_dir_file1_path, "base1"),
        ],
    );
    let expected = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_file_path, "side2"),
            (&trivial_hunk_path, "line1 side1\nline2\nline3 side2\n"),
            (&both_added_dir_file1_path, "side1"),
            (&both_added_dir_file2_path, "side2"),
        ],
    );

    let tree = MergedTree::new(Merge::new(vec![base1], vec![side1, side2]));
    let resolved = tree.resolve().unwrap();
    let resolved_tree = resolved.as_resolved().unwrap().clone();
    assert_eq!(
        resolved_tree,
        expected,
        "actual entries: {:#?}, expected entries {:#?}",
        resolved_tree.entries().collect_vec(),
        expected.entries().collect_vec()
    );
}

#[test]
fn test_resolve_root_becomes_empty() {
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let store = repo.store();

    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let base1 = testutils::create_tree(repo, &[(&path1, "base1"), (&path2, "base1")]);
    let side1 = testutils::create_tree(repo, &[(&path2, "base1")]);
    let side2 = testutils::create_tree(repo, &[(&path1, "base1")]);

    let tree = MergedTree::new(Merge::new(vec![base1], vec![side1, side2]));
    let resolved = tree.resolve().unwrap();
    assert_eq!(resolved.as_resolved().unwrap().id(), store.empty_tree_id());
}

#[test]
fn test_resolve_with_conflict() {
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    // The trivial conflict should be resolved but the non-trivial should not (and
    // cannot)
    let trivial_path = RepoPath::from_internal_string("dir1/trivial");
    let conflict_path = RepoPath::from_internal_string("dir2/file_conflict");
    let base1 =
        testutils::create_tree(repo, &[(&trivial_path, "base1"), (&conflict_path, "base1")]);
    let side1 =
        testutils::create_tree(repo, &[(&trivial_path, "side1"), (&conflict_path, "side1")]);
    let side2 =
        testutils::create_tree(repo, &[(&trivial_path, "base1"), (&conflict_path, "side2")]);
    let expected_base1 =
        testutils::create_tree(repo, &[(&trivial_path, "side1"), (&conflict_path, "base1")]);
    let expected_side1 =
        testutils::create_tree(repo, &[(&trivial_path, "side1"), (&conflict_path, "side1")]);
    let expected_side2 =
        testutils::create_tree(repo, &[(&trivial_path, "side1"), (&conflict_path, "side2")]);

    let tree = MergedTree::new(Merge::new(vec![base1], vec![side1, side2]));
    let resolved_tree = tree.resolve().unwrap();
    assert_eq!(
        resolved_tree,
        Merge::new(vec![expected_base1], vec![expected_side1, expected_side2])
    )
}

#[test]
fn test_conflict_iterator() {
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    let unchanged_path = RepoPath::from_internal_string("dir/subdir/unchanged");
    let trivial_path = RepoPath::from_internal_string("dir/subdir/trivial");
    let trivial_hunk_path = RepoPath::from_internal_string("dir/non_trivial");
    let file_conflict_path = RepoPath::from_internal_string("dir/subdir/file_conflict");
    let modify_delete_path = RepoPath::from_internal_string("dir/subdir/modify_delete");
    let same_add_path = RepoPath::from_internal_string("dir/subdir/same_add");
    let different_add_path = RepoPath::from_internal_string("dir/subdir/different_add");
    let dir_file_path = RepoPath::from_internal_string("dir/subdir/dir_file");
    let added_dir_path = RepoPath::from_internal_string("dir/new_dir");
    let modify_delete_dir_path = RepoPath::from_internal_string("dir/modify_delete_dir");
    let base1 = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_path, "base"),
            (&trivial_hunk_path, "line1\nline2\nline3\n"),
            (&file_conflict_path, "base"),
            (&modify_delete_path, "base"),
            // no same_add_path
            // no different_add_path
            (&dir_file_path, "base"),
            // no added_dir_path
            (
                &modify_delete_dir_path.join(&RepoPathComponent::from("base")),
                "base",
            ),
        ],
    );
    let side1 = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_path, "base"),
            (&file_conflict_path, "side1"),
            (&trivial_hunk_path, "line1 side1\nline2\nline3\n"),
            (&modify_delete_path, "modified"),
            (&same_add_path, "same"),
            (&different_add_path, "side1"),
            (&dir_file_path, "side1"),
            (
                &added_dir_path.join(&RepoPathComponent::from("side1")),
                "side1",
            ),
            (
                &modify_delete_dir_path.join(&RepoPathComponent::from("side1")),
                "side1",
            ),
        ],
    );
    let side2 = testutils::create_tree(
        repo,
        &[
            (&unchanged_path, "unchanged"),
            (&trivial_path, "side2"),
            (&file_conflict_path, "side2"),
            (&trivial_hunk_path, "line1\nline2\nline3 side2\n"),
            // no modify_delete_path
            (&same_add_path, "same"),
            (&different_add_path, "side2"),
            (&dir_file_path.join(&RepoPathComponent::from("dir")), "new"),
            (
                &added_dir_path.join(&RepoPathComponent::from("side2")),
                "side2",
            ),
            // no modify_delete_dir_path
        ],
    );

    let tree = MergedTree::new(Merge::new(
        vec![base1.clone()],
        vec![side1.clone(), side2.clone()],
    ));
    let conflicts = tree.conflicts().collect_vec();
    let conflict_at = |path: &RepoPath| {
        Merge::new(
            vec![base1.path_value(path)],
            vec![side1.path_value(path), side2.path_value(path)],
        )
    };
    // We initially also get a conflict in trivial_hunk_path because we had
    // forgotten to resolve conflicts
    assert_eq!(
        conflicts,
        vec![
            (trivial_hunk_path.clone(), conflict_at(&trivial_hunk_path)),
            (different_add_path.clone(), conflict_at(&different_add_path)),
            (dir_file_path.clone(), conflict_at(&dir_file_path)),
            (file_conflict_path.clone(), conflict_at(&file_conflict_path)),
            (modify_delete_path.clone(), conflict_at(&modify_delete_path)),
        ]
    );

    // After we resolve conflicts, there are only non-trivial conflicts left
    let tree = MergedTree::Merge(tree.resolve().unwrap());
    let conflicts = tree.conflicts().collect_vec();
    assert_eq!(
        conflicts,
        vec![
            (different_add_path.clone(), conflict_at(&different_add_path)),
            (dir_file_path.clone(), conflict_at(&dir_file_path)),
            (file_conflict_path.clone(), conflict_at(&file_conflict_path)),
            (modify_delete_path.clone(), conflict_at(&modify_delete_path)),
        ]
    );

    let merged_legacy_tree = merge_trees(&side1, &base1, &side2).unwrap();
    let legacy_conflicts = MergedTree::legacy(merged_legacy_tree)
        .conflicts()
        .collect_vec();
    assert_eq!(legacy_conflicts, conflicts);
}
#[test]
fn test_conflict_iterator_higher_arity() {
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    let two_sided_path = RepoPath::from_internal_string("dir/2-sided");
    let three_sided_path = RepoPath::from_internal_string("dir/3-sided");
    let base1 = testutils::create_tree(
        repo,
        &[(&two_sided_path, "base1"), (&three_sided_path, "base1")],
    );
    let base2 = testutils::create_tree(
        repo,
        &[(&two_sided_path, "base2"), (&three_sided_path, "base2")],
    );
    let side1 = testutils::create_tree(
        repo,
        &[(&two_sided_path, "side1"), (&three_sided_path, "side1")],
    );
    let side2 = testutils::create_tree(
        repo,
        &[(&two_sided_path, "base1"), (&three_sided_path, "side2")],
    );
    let side3 = testutils::create_tree(
        repo,
        &[(&two_sided_path, "side3"), (&three_sided_path, "side3")],
    );

    let tree = MergedTree::new(Merge::new(
        vec![base1.clone(), base2.clone()],
        vec![side1.clone(), side2.clone(), side3.clone()],
    ));
    let conflicts = tree.conflicts().collect_vec();
    let conflict_at = |path: &RepoPath| {
        Merge::new(
            vec![base1.path_value(path), base2.path_value(path)],
            vec![
                side1.path_value(path),
                side2.path_value(path),
                side3.path_value(path),
            ],
        )
    };
    // Both paths have the full, unsimplified conflict (3-sided)
    assert_eq!(
        conflicts,
        vec![
            (two_sided_path.clone(), conflict_at(&two_sided_path)),
            (three_sided_path.clone(), conflict_at(&three_sided_path))
        ]
    );
    // Iterating over conflicts in a legacy tree yields the simplified conflict at
    // each path
    let merged_legacy_tree = merge_trees(&side1, &base1, &side2).unwrap();
    let merged_legacy_tree = merge_trees(&merged_legacy_tree, &base2, &side3).unwrap();
    let legacy_conflicts = MergedTree::legacy(merged_legacy_tree)
        .conflicts()
        .collect_vec();
    assert_eq!(
        legacy_conflicts,
        vec![
            (
                two_sided_path.clone(),
                Merge::new(
                    vec![base2.path_value(&two_sided_path)],
                    vec![
                        side1.path_value(&two_sided_path),
                        side3.path_value(&two_sided_path),
                    ],
                )
            ),
            (three_sided_path.clone(), conflict_at(&three_sided_path))
        ]
    );
}
