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

use futures::executor::block_on;
use futures::StreamExt;
use itertools::Itertools;
use jj_lib::backend::{FileId, MergedTreeId, TreeValue};
use jj_lib::files::MergeResult;
use jj_lib::matchers::{EverythingMatcher, FilesMatcher, Matcher, PrefixMatcher};
use jj_lib::merge::{Merge, MergeBuilder};
use jj_lib::merged_tree::{
    MergedTree, MergedTreeBuilder, MergedTreeVal, TreeDiffIterator, TreeDiffStreamImpl,
};
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathComponent};
use jj_lib::tree::merge_trees;
use pretty_assertions::assert_eq;
use testutils::{create_single_tree, write_file, TestRepo};

fn file_value(file_id: &FileId) -> TreeValue {
    TreeValue::File {
        id: file_id.clone(),
        executable: false,
    }
}

fn diff_stream_equals_iter(tree1: &MergedTree, tree2: &MergedTree, matcher: &dyn Matcher) {
    let iter_diff: Vec<_> = TreeDiffIterator::new(tree1.clone(), tree2.clone(), matcher)
        .map(|(path, diff)| (path, diff.unwrap()))
        .collect();
    let max_concurrent_reads = 10;
    let stream_diff: Vec<_> = block_on(
        TreeDiffStreamImpl::new(tree1.clone(), tree2.clone(), matcher, max_concurrent_reads)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect(),
    );
    assert_eq!(stream_diff, iter_diff);
}

#[test]
fn test_from_legacy_tree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let mut tree_builder = store.tree_builder(repo.store().empty_tree_id().clone());

    // file1: regular file without conflicts
    let file1_path = RepoPath::from_internal_string("no_conflict");
    let file1_id = write_file(store.as_ref(), file1_path, "foo");
    tree_builder.set(file1_path.to_owned(), file_value(&file1_id));

    // file2: 3-way conflict
    let file2_path = RepoPath::from_internal_string("3way");
    let file2_v1_id = write_file(store.as_ref(), file2_path, "file2_v1");
    let file2_v2_id = write_file(store.as_ref(), file2_path, "file2_v2");
    let file2_v3_id = write_file(store.as_ref(), file2_path, "file2_v3");
    let file2_conflict = Merge::from_removes_adds(
        vec![Some(file_value(&file2_v1_id))],
        vec![
            Some(file_value(&file2_v2_id)),
            Some(file_value(&file2_v3_id)),
        ],
    );
    let file2_conflict_id = store.write_conflict(file2_path, &file2_conflict).unwrap();
    tree_builder.set(
        file2_path.to_owned(),
        TreeValue::Conflict(file2_conflict_id),
    );

    // file3: modify/delete conflict
    let file3_path = RepoPath::from_internal_string("modify_delete");
    let file3_v1_id = write_file(store.as_ref(), file3_path, "file3_v1");
    let file3_v2_id = write_file(store.as_ref(), file3_path, "file3_v2");
    let file3_conflict = Merge::from_removes_adds(
        vec![Some(file_value(&file3_v1_id))],
        vec![Some(file_value(&file3_v2_id)), None],
    );
    let file3_conflict_id = store.write_conflict(file3_path, &file3_conflict).unwrap();
    tree_builder.set(
        file3_path.to_owned(),
        TreeValue::Conflict(file3_conflict_id),
    );

    // file4: add/add conflict
    let file4_path = RepoPath::from_internal_string("add_add");
    let file4_v1_id = write_file(store.as_ref(), file4_path, "file4_v1");
    let file4_v2_id = write_file(store.as_ref(), file4_path, "file4_v2");
    let file4_conflict = Merge::from_removes_adds(
        vec![None],
        vec![
            Some(file_value(&file4_v1_id)),
            Some(file_value(&file4_v2_id)),
        ],
    );
    let file4_conflict_id = store.write_conflict(file4_path, &file4_conflict).unwrap();
    tree_builder.set(
        file4_path.to_owned(),
        TreeValue::Conflict(file4_conflict_id),
    );

    // file5: 5-way conflict
    let file5_path = RepoPath::from_internal_string("5way");
    let file5_v1_id = write_file(store.as_ref(), file5_path, "file5_v1");
    let file5_v2_id = write_file(store.as_ref(), file5_path, "file5_v2");
    let file5_v3_id = write_file(store.as_ref(), file5_path, "file5_v3");
    let file5_v4_id = write_file(store.as_ref(), file5_path, "file5_v4");
    let file5_v5_id = write_file(store.as_ref(), file5_path, "file5_v5");
    let file5_conflict = Merge::from_removes_adds(
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
    let file5_conflict_id = store.write_conflict(file5_path, &file5_conflict).unwrap();
    tree_builder.set(
        file5_path.to_owned(),
        TreeValue::Conflict(file5_conflict_id),
    );

    // dir1: directory without conflicts
    let dir1_basename = RepoPathComponent::new("dir1");
    let dir1_filename = &RepoPath::root()
        .join(dir1_basename)
        .join(RepoPathComponent::new("file"));
    let dir1_filename_id = write_file(store.as_ref(), dir1_filename, "file5_v2");
    tree_builder.set(dir1_filename.to_owned(), file_value(&dir1_filename_id));

    let tree_id = tree_builder.write_tree();
    let tree = store.get_tree(RepoPath::root(), &tree_id).unwrap();

    let merged_tree = MergedTree::from_legacy_tree(tree.clone()).unwrap();
    assert_eq!(
        merged_tree.value(RepoPathComponent::new("missing")),
        MergedTreeVal::Resolved(None)
    );
    // file1: regular file without conflicts
    assert_eq!(
        merged_tree.value(file1_path.components().next().unwrap()),
        MergedTreeVal::Resolved(Some(&TreeValue::File {
            id: file1_id.clone(),
            executable: false,
        }))
    );
    // file2: 3-way conflict
    assert_eq!(
        merged_tree.value(file2_path.components().next().unwrap()),
        MergedTreeVal::Conflict(Merge::from_removes_adds(
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
        merged_tree.value(file3_path.components().next().unwrap()),
        MergedTreeVal::Conflict(Merge::from_removes_adds(
            vec![Some(file_value(&file3_v1_id)), None],
            vec![Some(file_value(&file3_v2_id)), None, None],
        ))
    );
    // file4: add/add conflict
    assert_eq!(
        merged_tree.value(file4_path.components().next().unwrap()),
        MergedTreeVal::Conflict(Merge::from_removes_adds(
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
        merged_tree.value(file5_path.components().next().unwrap()),
        MergedTreeVal::Conflict(Merge::from_removes_adds(
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
        merged_tree.value(dir1_basename),
        MergedTreeVal::Resolved(tree.value(dir1_basename))
    );

    // Also test that MergedTreeBuilder can create the same tree by starting from an
    // empty legacy tree.
    let mut tree_builder = MergedTreeBuilder::new(store.empty_merged_tree_id());
    for (path, value) in tree.entries() {
        tree_builder.set_or_remove(path, Merge::normal(value));
    }
    let recreated_legacy_id = tree_builder.write_tree(store).unwrap();
    assert_eq!(recreated_legacy_id, MergedTreeId::Legacy(tree_id.clone()));

    // Create the merged tree by starting from an empty merged tree and adding
    // entries from the merged tree we created before
    let empty_merged_id_builder: MergeBuilder<_> = std::iter::repeat(store.empty_tree_id())
        .take(5)
        .cloned()
        .collect();
    let empty_merged_id = MergedTreeId::Merge(empty_merged_id_builder.build());
    let mut tree_builder = MergedTreeBuilder::new(empty_merged_id);
    for (path, value) in merged_tree.entries() {
        tree_builder.set_or_remove(path, value);
    }
    let recreated_merged_id = tree_builder.write_tree(store).unwrap();
    assert_eq!(recreated_merged_id, merged_tree.id());

    // Create the merged tree by adding the same (variable-arity) entries as we
    // added to the single-tree TreeBuilder.
    let mut tree_builder = MergedTreeBuilder::new(MergedTreeId::Merge(Merge::resolved(
        store.empty_tree_id().clone(),
    )));
    // Add the entries out of order, so we test both increasing and reducing the
    // arity (going up from 1-way to 3-way to 5-way, then to 3-way again)
    tree_builder.set_or_remove(file1_path.to_owned(), Merge::normal(file_value(&file1_id)));
    tree_builder.set_or_remove(file2_path.to_owned(), file2_conflict);
    tree_builder.set_or_remove(file5_path.to_owned(), file5_conflict);
    tree_builder.set_or_remove(file3_path.to_owned(), file3_conflict);
    tree_builder.set_or_remove(file4_path.to_owned(), file4_conflict);
    tree_builder.set_or_remove(
        dir1_filename.to_owned(),
        Merge::normal(file_value(&dir1_filename_id)),
    );
    let recreated_merged_id = tree_builder.write_tree(store).unwrap();
    assert_eq!(recreated_merged_id, merged_tree.id());
}

#[test]
fn test_path_value_and_entries() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Create a MergedTree
    let resolved_file_path = RepoPath::from_internal_string("dir1/subdir/resolved");
    let resolved_dir_path = &resolved_file_path.parent().unwrap();
    let conflicted_file_path = RepoPath::from_internal_string("dir2/conflicted");
    let missing_path = RepoPath::from_internal_string("dir2/missing_file");
    let modify_delete_path = RepoPath::from_internal_string("dir2/modify_delete");
    let file_dir_conflict_path = RepoPath::from_internal_string("file_dir");
    let file_dir_conflict_sub_path = RepoPath::from_internal_string("file_dir/file");
    let tree1 = create_single_tree(
        repo,
        &[
            (resolved_file_path, "unchanged"),
            (conflicted_file_path, "1"),
            (modify_delete_path, "1"),
            (file_dir_conflict_path, "1"),
        ],
    );
    let tree2 = create_single_tree(
        repo,
        &[
            (resolved_file_path, "unchanged"),
            (conflicted_file_path, "2"),
            (modify_delete_path, "2"),
            (file_dir_conflict_path, "2"),
        ],
    );
    let tree3 = create_single_tree(
        repo,
        &[
            (resolved_file_path, "unchanged"),
            (conflicted_file_path, "3"),
            // No modify_delete_path in this tree
            (file_dir_conflict_sub_path, "1"),
        ],
    );
    let merged_tree = MergedTree::Merge(Merge::from_removes_adds(
        vec![tree1.clone()],
        vec![tree2.clone(), tree3.clone()],
    ));

    // Get the root tree
    assert_eq!(
        merged_tree.path_value(RepoPath::root()),
        Merge::from_removes_adds(
            vec![Some(TreeValue::Tree(tree1.id().clone()))],
            vec![
                Some(TreeValue::Tree(tree2.id().clone())),
                Some(TreeValue::Tree(tree3.id().clone())),
            ]
        )
    );
    // Get file path without conflict
    assert_eq!(
        merged_tree.path_value(resolved_file_path),
        Merge::resolved(tree1.path_value(resolved_file_path)),
    );
    // Get directory path without conflict
    assert_eq!(
        merged_tree.path_value(resolved_dir_path),
        Merge::resolved(tree1.path_value(resolved_dir_path)),
    );
    // Get missing path
    assert_eq!(merged_tree.path_value(missing_path), Merge::absent());
    // Get modify/delete conflict (some None values)
    assert_eq!(
        merged_tree.path_value(modify_delete_path),
        Merge::from_removes_adds(
            vec![tree1.path_value(modify_delete_path)],
            vec![tree2.path_value(modify_delete_path), None]
        ),
    );
    // Get file/dir conflict path
    assert_eq!(
        merged_tree.path_value(file_dir_conflict_path),
        Merge::from_removes_adds(
            vec![tree1.path_value(file_dir_conflict_path)],
            vec![
                tree2.path_value(file_dir_conflict_path),
                tree3.path_value(file_dir_conflict_path)
            ]
        ),
    );
    // Get file inside file/dir conflict
    // There is a conflict in the parent directory, but this file is still resolved
    assert_eq!(
        merged_tree.path_value(file_dir_conflict_sub_path),
        Merge::resolved(tree3.path_value(file_dir_conflict_sub_path)),
    );

    // Test entries()
    let actual_entries = merged_tree.entries().collect_vec();
    // missing_path, resolved_dir_path, and file_dir_conflict_sub_path should not
    // appear
    let expected_entries = [
        resolved_file_path,
        conflicted_file_path,
        modify_delete_path,
        file_dir_conflict_path,
    ]
    .iter()
    .sorted()
    .map(|&path| (path.to_owned(), merged_tree.path_value(path)))
    .collect_vec();
    assert_eq!(actual_entries, expected_entries);

    let actual_entries = merged_tree
        .entries_matching(&FilesMatcher::new([
            &resolved_file_path,
            &modify_delete_path,
            &file_dir_conflict_sub_path,
        ]))
        .collect_vec();
    let expected_entries = [resolved_file_path, modify_delete_path]
        .iter()
        .sorted()
        .map(|&path| (path.to_owned(), merged_tree.path_value(path)))
        .collect_vec();
    assert_eq!(actual_entries, expected_entries);
}

#[test]
fn test_resolve_success() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let unchanged_path = RepoPath::from_internal_string("unchanged");
    let trivial_file_path = RepoPath::from_internal_string("trivial-file");
    let trivial_hunk_path = RepoPath::from_internal_string("trivial-hunk");
    let both_added_dir_path = RepoPath::from_internal_string("added-dir");
    let both_added_dir_file1_path = &both_added_dir_path.join(RepoPathComponent::new("file1"));
    let both_added_dir_file2_path = &both_added_dir_path.join(RepoPathComponent::new("file2"));
    let emptied_dir_path = RepoPath::from_internal_string("to-become-empty");
    let emptied_dir_file1_path = &emptied_dir_path.join(RepoPathComponent::new("file1"));
    let emptied_dir_file2_path = &emptied_dir_path.join(RepoPathComponent::new("file2"));
    let base1 = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_file_path, "base1"),
            (trivial_hunk_path, "line1\nline2\nline3\n"),
            (emptied_dir_file1_path, "base1"),
            (emptied_dir_file2_path, "base1"),
        ],
    );
    let side1 = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_file_path, "base1"),
            (trivial_hunk_path, "line1 side1\nline2\nline3\n"),
            (both_added_dir_file1_path, "side1"),
            (emptied_dir_file2_path, "base1"),
        ],
    );
    let side2 = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_file_path, "side2"),
            (trivial_hunk_path, "line1\nline2\nline3 side2\n"),
            (both_added_dir_file2_path, "side2"),
            (emptied_dir_file1_path, "base1"),
        ],
    );
    let expected = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_file_path, "side2"),
            (trivial_hunk_path, "line1 side1\nline2\nline3 side2\n"),
            (both_added_dir_file1_path, "side1"),
            (both_added_dir_file2_path, "side2"),
        ],
    );

    let tree = MergedTree::new(Merge::from_removes_adds(vec![base1], vec![side1, side2]));
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
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let store = repo.store();

    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let base1 = create_single_tree(repo, &[(path1, "base1"), (path2, "base1")]);
    let side1 = create_single_tree(repo, &[(path2, "base1")]);
    let side2 = create_single_tree(repo, &[(path1, "base1")]);

    let tree = MergedTree::new(Merge::from_removes_adds(vec![base1], vec![side1, side2]));
    let resolved = tree.resolve().unwrap();
    assert_eq!(resolved.as_resolved().unwrap().id(), store.empty_tree_id());
}

#[test]
fn test_resolve_with_conflict() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // The trivial conflict should be resolved but the non-trivial should not (and
    // cannot)
    let trivial_path = RepoPath::from_internal_string("dir1/trivial");
    let conflict_path = RepoPath::from_internal_string("dir2/file_conflict");
    let base1 = create_single_tree(repo, &[(trivial_path, "base1"), (conflict_path, "base1")]);
    let side1 = create_single_tree(repo, &[(trivial_path, "side1"), (conflict_path, "side1")]);
    let side2 = create_single_tree(repo, &[(trivial_path, "base1"), (conflict_path, "side2")]);
    let expected_base1 =
        create_single_tree(repo, &[(trivial_path, "side1"), (conflict_path, "base1")]);
    let expected_side1 =
        create_single_tree(repo, &[(trivial_path, "side1"), (conflict_path, "side1")]);
    let expected_side2 =
        create_single_tree(repo, &[(trivial_path, "side1"), (conflict_path, "side2")]);

    let tree = MergedTree::new(Merge::from_removes_adds(vec![base1], vec![side1, side2]));
    let resolved_tree = tree.resolve().unwrap();
    assert_eq!(
        resolved_tree,
        Merge::from_removes_adds(vec![expected_base1], vec![expected_side1, expected_side2])
    )
}

#[test]
fn test_resolve_with_conflict_containing_empty_subtree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Since "dir" in side2 is absent, the root tree should be empty as well.
    // If it were added to the root tree, side2.id() would differ.
    let conflict_path = RepoPath::from_internal_string("dir/file_conflict");
    let base1 = create_single_tree(repo, &[(conflict_path, "base1")]);
    let side1 = create_single_tree(repo, &[(conflict_path, "side1")]);
    let side2 = create_single_tree(repo, &[]);

    let original_tree = Merge::from_removes_adds(vec![base1], vec![side1, side2]);
    let tree = MergedTree::new(original_tree.clone());
    let resolved_tree = tree.resolve().unwrap();
    assert_eq!(resolved_tree, original_tree);
}

#[test]
fn test_conflict_iterator() {
    let test_repo = TestRepo::init();
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
    let base1 = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_path, "base"),
            (trivial_hunk_path, "line1\nline2\nline3\n"),
            (file_conflict_path, "base"),
            (modify_delete_path, "base"),
            // no same_add_path
            // no different_add_path
            (dir_file_path, "base"),
            // no added_dir_path
            (
                &modify_delete_dir_path.join(RepoPathComponent::new("base")),
                "base",
            ),
        ],
    );
    let side1 = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_path, "base"),
            (file_conflict_path, "side1"),
            (trivial_hunk_path, "line1 side1\nline2\nline3\n"),
            (modify_delete_path, "modified"),
            (same_add_path, "same"),
            (different_add_path, "side1"),
            (dir_file_path, "side1"),
            (
                &added_dir_path.join(RepoPathComponent::new("side1")),
                "side1",
            ),
            (
                &modify_delete_dir_path.join(RepoPathComponent::new("side1")),
                "side1",
            ),
        ],
    );
    let side2 = create_single_tree(
        repo,
        &[
            (unchanged_path, "unchanged"),
            (trivial_path, "side2"),
            (file_conflict_path, "side2"),
            (trivial_hunk_path, "line1\nline2\nline3 side2\n"),
            // no modify_delete_path
            (same_add_path, "same"),
            (different_add_path, "side2"),
            (&dir_file_path.join(RepoPathComponent::new("dir")), "new"),
            (
                &added_dir_path.join(RepoPathComponent::new("side2")),
                "side2",
            ),
            // no modify_delete_dir_path
        ],
    );

    let tree = MergedTree::new(Merge::from_removes_adds(
        vec![base1.clone()],
        vec![side1.clone(), side2.clone()],
    ));
    let conflicts = tree.conflicts().collect_vec();
    let conflict_at = |path: &RepoPath| {
        Merge::from_removes_adds(
            vec![base1.path_value(path)],
            vec![side1.path_value(path), side2.path_value(path)],
        )
    };
    // We initially also get a conflict in trivial_hunk_path because we had
    // forgotten to resolve conflicts
    assert_eq!(
        conflicts,
        vec![
            (trivial_hunk_path.to_owned(), conflict_at(trivial_hunk_path)),
            (
                different_add_path.to_owned(),
                conflict_at(different_add_path)
            ),
            (dir_file_path.to_owned(), conflict_at(dir_file_path)),
            (
                file_conflict_path.to_owned(),
                conflict_at(file_conflict_path)
            ),
            (
                modify_delete_path.to_owned(),
                conflict_at(modify_delete_path)
            ),
        ]
    );

    // After we resolve conflicts, there are only non-trivial conflicts left
    let tree = MergedTree::Merge(tree.resolve().unwrap());
    let conflicts = tree.conflicts().collect_vec();
    assert_eq!(
        conflicts,
        vec![
            (
                different_add_path.to_owned(),
                conflict_at(different_add_path)
            ),
            (dir_file_path.to_owned(), conflict_at(dir_file_path)),
            (
                file_conflict_path.to_owned(),
                conflict_at(file_conflict_path)
            ),
            (
                modify_delete_path.to_owned(),
                conflict_at(modify_delete_path)
            ),
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
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let two_sided_path = RepoPath::from_internal_string("dir/2-sided");
    let three_sided_path = RepoPath::from_internal_string("dir/3-sided");
    let base1 = create_single_tree(
        repo,
        &[(two_sided_path, "base1"), (three_sided_path, "base1")],
    );
    let base2 = create_single_tree(
        repo,
        &[(two_sided_path, "base2"), (three_sided_path, "base2")],
    );
    let side1 = create_single_tree(
        repo,
        &[(two_sided_path, "side1"), (three_sided_path, "side1")],
    );
    let side2 = create_single_tree(
        repo,
        &[(two_sided_path, "base1"), (three_sided_path, "side2")],
    );
    let side3 = create_single_tree(
        repo,
        &[(two_sided_path, "side3"), (three_sided_path, "side3")],
    );

    let tree = MergedTree::new(Merge::from_removes_adds(
        vec![base1.clone(), base2.clone()],
        vec![side1.clone(), side2.clone(), side3.clone()],
    ));
    let conflicts = tree.conflicts().collect_vec();
    let conflict_at = |path: &RepoPath| {
        Merge::from_removes_adds(
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
            (two_sided_path.to_owned(), conflict_at(two_sided_path)),
            (three_sided_path.to_owned(), conflict_at(three_sided_path))
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
                two_sided_path.to_owned(),
                Merge::from_removes_adds(
                    vec![base2.path_value(two_sided_path)],
                    vec![
                        side1.path_value(two_sided_path),
                        side3.path_value(two_sided_path),
                    ],
                )
            ),
            (three_sided_path.to_owned(), conflict_at(three_sided_path))
        ]
    );
}

/// Diff two resolved trees
#[test]
fn test_diff_resolved() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let clean_path = RepoPath::from_internal_string("dir1/file");
    let modified_path = RepoPath::from_internal_string("dir2/file");
    let removed_path = RepoPath::from_internal_string("dir3/file");
    let added_path = RepoPath::from_internal_string("dir4/file");
    let before = create_single_tree(
        repo,
        &[
            (clean_path, "clean"),
            (modified_path, "before"),
            (removed_path, "before"),
        ],
    );
    let after = create_single_tree(
        repo,
        &[
            (clean_path, "clean"),
            (modified_path, "after"),
            (added_path, "after"),
        ],
    );
    let before_merged = MergedTree::new(Merge::resolved(before.clone()));
    let after_merged = MergedTree::new(Merge::resolved(after.clone()));

    let diff = before_merged
        .diff(&after_merged, &EverythingMatcher)
        .map(|(path, diff)| (path, diff.unwrap()))
        .collect_vec();
    assert_eq!(diff.len(), 3);
    assert_eq!(
        diff[0].clone(),
        (
            modified_path.to_owned(),
            (
                Merge::resolved(before.path_value(modified_path)),
                Merge::resolved(after.path_value(modified_path))
            ),
        )
    );
    assert_eq!(
        diff[1].clone(),
        (
            removed_path.to_owned(),
            (
                Merge::resolved(before.path_value(removed_path)),
                Merge::absent()
            ),
        )
    );
    assert_eq!(
        diff[2].clone(),
        (
            added_path.to_owned(),
            (
                Merge::absent(),
                Merge::resolved(after.path_value(added_path))
            ),
        )
    );
    diff_stream_equals_iter(&before_merged, &after_merged, &EverythingMatcher);
}

/// Diff two conflicted trees
#[test]
fn test_diff_conflicted() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // path1 is a clean (unchanged) conflict
    // path2 is a conflict before and different conflict after
    // path3 is resolved before and a conflict after
    // path4 is missing before and a conflict after
    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let path3 = RepoPath::from_internal_string("dir4/file");
    let path4 = RepoPath::from_internal_string("dir6/file");
    let left_base = create_single_tree(
        repo,
        &[(path1, "clean-base"), (path2, "left-base"), (path3, "left")],
    );
    let left_side1 = create_single_tree(
        repo,
        &[
            (path1, "clean-side1"),
            (path2, "left-side1"),
            (path3, "left"),
        ],
    );
    let left_side2 = create_single_tree(
        repo,
        &[
            (path1, "clean-side2"),
            (path2, "left-side2"),
            (path3, "left"),
        ],
    );
    let right_base = create_single_tree(
        repo,
        &[
            (path1, "clean-base"),
            (path2, "right-base"),
            (path3, "right-base"),
            (path4, "right-base"),
        ],
    );
    let right_side1 = create_single_tree(
        repo,
        &[
            (path1, "clean-side1"),
            (path2, "right-side1"),
            (path3, "right-side1"),
            (path4, "right-side1"),
        ],
    );
    let right_side2 = create_single_tree(
        repo,
        &[
            (path1, "clean-side2"),
            (path2, "right-side2"),
            (path3, "right-side2"),
            (path4, "right-side2"),
        ],
    );
    let left_merged = MergedTree::new(Merge::from_removes_adds(
        vec![left_base.clone()],
        vec![left_side1.clone(), left_side2.clone()],
    ));
    let right_merged = MergedTree::new(Merge::from_removes_adds(
        vec![right_base.clone()],
        vec![right_side1.clone(), right_side2.clone()],
    ));

    // Test the forwards diff
    let actual_diff = left_merged
        .diff(&right_merged, &EverythingMatcher)
        .map(|(path, diff)| (path, diff.unwrap()))
        .collect_vec();
    let expected_diff = [path2, path3, path4]
        .iter()
        .map(|&path| {
            (
                path.to_owned(),
                (left_merged.path_value(path), right_merged.path_value(path)),
            )
        })
        .collect_vec();
    assert_eq!(actual_diff, expected_diff);
    diff_stream_equals_iter(&left_merged, &right_merged, &EverythingMatcher);
    // Test the reverse diff
    let actual_diff = right_merged
        .diff(&left_merged, &EverythingMatcher)
        .map(|(path, diff)| (path, diff.unwrap()))
        .collect_vec();
    let expected_diff = [path2, path3, path4]
        .iter()
        .map(|&path| {
            (
                path.to_owned(),
                (right_merged.path_value(path), left_merged.path_value(path)),
            )
        })
        .collect_vec();
    assert_eq!(actual_diff, expected_diff);
    diff_stream_equals_iter(&right_merged, &left_merged, &EverythingMatcher);
}

#[test]
fn test_diff_dir_file() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // path1: file1 -> directory1
    // path2: file1 -> directory1+(directory2-absent)
    // path3: file1 -> directory1+(file1-absent)
    // path4: file1+(file2-file3) -> directory1+(directory2-directory3)
    // path5: directory1 -> file1+(file2-absent)
    // path6: directory1 -> file1+(directory1-absent)
    let path1 = RepoPath::from_internal_string("path1");
    let path2 = RepoPath::from_internal_string("path2");
    let path3 = RepoPath::from_internal_string("path3");
    let path4 = RepoPath::from_internal_string("path4");
    let path5 = RepoPath::from_internal_string("path5");
    let path6 = RepoPath::from_internal_string("path6");
    let file = RepoPathComponent::new("file");
    let left_base = create_single_tree(
        repo,
        &[
            (path1, "left"),
            (path2, "left"),
            (path3, "left"),
            (path4, "left-base"),
            (&path5.join(file), "left"),
            (&path6.join(file), "left"),
        ],
    );
    let left_side1 = create_single_tree(
        repo,
        &[
            (path1, "left"),
            (path2, "left"),
            (path3, "left"),
            (path4, "left-side1"),
            (&path5.join(file), "left"),
            (&path6.join(file), "left"),
        ],
    );
    let left_side2 = create_single_tree(
        repo,
        &[
            (path1, "left"),
            (path2, "left"),
            (path3, "left"),
            (path4, "left-side2"),
            (&path5.join(file), "left"),
            (&path6.join(file), "left"),
        ],
    );
    let right_base = create_single_tree(
        repo,
        &[
            (&path1.join(file), "right"),
            // path2 absent
            // path3 absent
            (&path4.join(file), "right-base"),
            // path5 is absent
            // path6 is absent
        ],
    );
    let right_side1 = create_single_tree(
        repo,
        &[
            (&path1.join(file), "right"),
            (&path2.join(file), "right"),
            (&path3.join(file), "right-side1"),
            (&path4.join(file), "right-side1"),
            (path5, "right-side1"),
            (path6, "right"),
        ],
    );
    let right_side2 = create_single_tree(
        repo,
        &[
            (&path1.join(file), "right"),
            (&path2.join(file), "right"),
            (path3, "right-side2"),
            (&path4.join(file), "right-side2"),
            (path5, "right-side2"),
            (&path6.join(file), "right"),
        ],
    );
    let left_merged = MergedTree::new(Merge::from_removes_adds(
        vec![left_base],
        vec![left_side1, left_side2],
    ));
    let right_merged = MergedTree::new(Merge::from_removes_adds(
        vec![right_base],
        vec![right_side1, right_side2],
    ));

    // Test the forwards diff
    {
        let actual_diff = left_merged
            .diff(&right_merged, &EverythingMatcher)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect_vec();
        let expected_diff = vec![
            // path1: file1 -> directory1
            (
                path1.to_owned(),
                (left_merged.path_value(path1), Merge::absent()),
            ),
            (
                path1.join(file),
                (Merge::absent(), right_merged.path_value(&path1.join(file))),
            ),
            // path2: file1 -> directory1+(directory2-absent)
            (
                path2.to_owned(),
                (left_merged.path_value(path2), Merge::absent()),
            ),
            (
                path2.join(file),
                (Merge::absent(), right_merged.path_value(&path2.join(file))),
            ),
            // path3: file1 -> directory1+(file1-absent)
            (
                path3.to_owned(),
                (
                    left_merged.path_value(path3),
                    right_merged.path_value(path3),
                ),
            ),
            // path4: file1+(file2-file3) -> directory1+(directory2-directory3)
            (
                path4.to_owned(),
                (left_merged.path_value(path4), Merge::absent()),
            ),
            (
                path4.join(file),
                (Merge::absent(), right_merged.path_value(&path4.join(file))),
            ),
            // path5: directory1 -> file1+(file2-absent)
            (
                path5.join(file),
                (left_merged.path_value(&path5.join(file)), Merge::absent()),
            ),
            (
                path5.to_owned(),
                (Merge::absent(), right_merged.path_value(path5)),
            ),
            // path6: directory1 -> file1+(directory1-absent)
            (
                path6.join(file),
                (left_merged.path_value(&path6.join(file)), Merge::absent()),
            ),
            (
                path6.to_owned(),
                (Merge::absent(), right_merged.path_value(path6)),
            ),
        ];
        assert_eq!(actual_diff, expected_diff);
        diff_stream_equals_iter(&left_merged, &right_merged, &EverythingMatcher);
    }

    // Test the reverse diff
    {
        let actual_diff = right_merged
            .diff(&left_merged, &EverythingMatcher)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect_vec();
        let expected_diff = vec![
            // path1: file1 -> directory1
            (
                path1.join(file),
                (right_merged.path_value(&path1.join(file)), Merge::absent()),
            ),
            (
                path1.to_owned(),
                (Merge::absent(), left_merged.path_value(path1)),
            ),
            // path2: file1 -> directory1+(directory2-absent)
            (
                path2.join(file),
                (right_merged.path_value(&path2.join(file)), Merge::absent()),
            ),
            (
                path2.to_owned(),
                (Merge::absent(), left_merged.path_value(path2)),
            ),
            // path3: file1 -> directory1+(file1-absent)
            (
                path3.to_owned(),
                (
                    right_merged.path_value(path3),
                    left_merged.path_value(path3),
                ),
            ),
            // path4: file1+(file2-file3) -> directory1+(directory2-directory3)
            (
                path4.join(file),
                (right_merged.path_value(&path4.join(file)), Merge::absent()),
            ),
            (
                path4.to_owned(),
                (Merge::absent(), left_merged.path_value(path4)),
            ),
            // path5: directory1 -> file1+(file2-absent)
            (
                path5.to_owned(),
                (right_merged.path_value(path5), Merge::absent()),
            ),
            (
                path5.join(file),
                (Merge::absent(), left_merged.path_value(&path5.join(file))),
            ),
            // path6: directory1 -> file1+(directory1-absent)
            (
                path6.to_owned(),
                (right_merged.path_value(path6), Merge::absent()),
            ),
            (
                path6.join(file),
                (Merge::absent(), left_merged.path_value(&path6.join(file))),
            ),
        ];
        assert_eq!(actual_diff, expected_diff);
        diff_stream_equals_iter(&right_merged, &left_merged, &EverythingMatcher);
    }

    // Diff while filtering by `path1` (file1 -> directory1) as a file
    {
        let matcher = FilesMatcher::new([&path1]);
        let actual_diff = left_merged
            .diff(&right_merged, &matcher)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect_vec();
        let expected_diff = vec![
            // path1: file1 -> directory1
            (
                path1.to_owned(),
                (left_merged.path_value(path1), Merge::absent()),
            ),
        ];
        assert_eq!(actual_diff, expected_diff);
        diff_stream_equals_iter(&left_merged, &right_merged, &matcher);
    }

    // Diff while filtering by `path1/file` (file1 -> directory1) as a file
    {
        let matcher = FilesMatcher::new([path1.join(file)]);
        let actual_diff = left_merged
            .diff(&right_merged, &matcher)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect_vec();
        let expected_diff = vec![
            // path1: file1 -> directory1
            (
                path1.join(file),
                (Merge::absent(), right_merged.path_value(&path1.join(file))),
            ),
        ];
        assert_eq!(actual_diff, expected_diff);
        diff_stream_equals_iter(&left_merged, &right_merged, &matcher);
    }

    // Diff while filtering by `path1` (file1 -> directory1) as a prefix
    {
        let matcher = PrefixMatcher::new([&path1]);
        let actual_diff = left_merged
            .diff(&right_merged, &matcher)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect_vec();
        let expected_diff = vec![
            (
                path1.to_owned(),
                (left_merged.path_value(path1), Merge::absent()),
            ),
            (
                path1.join(file),
                (Merge::absent(), right_merged.path_value(&path1.join(file))),
            ),
        ];
        assert_eq!(actual_diff, expected_diff);
        diff_stream_equals_iter(&left_merged, &right_merged, &matcher);
    }

    // Diff while filtering by `path6` (directory1 -> file1+(directory1-absent)) as
    // a file. We don't see the directory at `path6` on the left side, but we
    // do see the directory that's included in the conflict with a file on the right
    // side.
    {
        let matcher = FilesMatcher::new([&path6]);
        let actual_diff = left_merged
            .diff(&right_merged, &matcher)
            .map(|(path, diff)| (path, diff.unwrap()))
            .collect_vec();
        let expected_diff = vec![(
            path6.to_owned(),
            (Merge::absent(), right_merged.path_value(path6)),
        )];
        assert_eq!(actual_diff, expected_diff);
        diff_stream_equals_iter(&left_merged, &right_merged, &matcher);
    }
}

/// Merge 3 resolved trees that can be resolved
#[test]
fn test_merge_simple() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let base1 = create_single_tree(repo, &[(path1, "base"), (path2, "base")]);
    let side1 = create_single_tree(repo, &[(path1, "side1"), (path2, "base")]);
    let side2 = create_single_tree(repo, &[(path1, "base"), (path2, "side2")]);
    let expected = create_single_tree(repo, &[(path1, "side1"), (path2, "side2")]);
    let base1_merged = MergedTree::new(Merge::resolved(base1));
    let side1_merged = MergedTree::new(Merge::resolved(side1));
    let side2_merged = MergedTree::new(Merge::resolved(side2));
    let expected_merged = MergedTree::new(Merge::resolved(expected));

    let merged = side1_merged.merge(&base1_merged, &side2_merged).unwrap();
    assert_eq!(merged, expected_merged);
}

/// Merge 3 resolved trees that can be partially resolved
#[test]
fn test_merge_partial_resolution() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // path1 can be resolved, path2 cannot
    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let base1 = create_single_tree(repo, &[(path1, "base"), (path2, "base")]);
    let side1 = create_single_tree(repo, &[(path1, "side1"), (path2, "side1")]);
    let side2 = create_single_tree(repo, &[(path1, "base"), (path2, "side2")]);
    let expected_base1 = create_single_tree(repo, &[(path1, "side1"), (path2, "base")]);
    let expected_side1 = create_single_tree(repo, &[(path1, "side1"), (path2, "side1")]);
    let expected_side2 = create_single_tree(repo, &[(path1, "side1"), (path2, "side2")]);
    let base1_merged = MergedTree::new(Merge::resolved(base1));
    let side1_merged = MergedTree::new(Merge::resolved(side1));
    let side2_merged = MergedTree::new(Merge::resolved(side2));
    let expected_merged = MergedTree::new(Merge::from_removes_adds(
        vec![expected_base1],
        vec![expected_side1, expected_side2],
    ));

    let merged = side1_merged.merge(&base1_merged, &side2_merged).unwrap();
    assert_eq!(merged, expected_merged);
}

/// Merge 3 resolved trees, including one empty legacy tree
#[test]
fn test_merge_with_empty_legacy_tree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let base1 = repo
        .store()
        .get_tree(RepoPath::root(), repo.store().empty_tree_id())
        .unwrap();
    let side1 = create_single_tree(repo, &[(path1, "side1")]);
    let side2 = create_single_tree(repo, &[(path2, "side2")]);
    let expected = create_single_tree(repo, &[(path1, "side1"), (path2, "side2")]);
    let base1_merged = MergedTree::legacy(base1);
    let side1_merged = MergedTree::new(Merge::resolved(side1));
    let side2_merged = MergedTree::new(Merge::resolved(side2));
    let expected_merged = MergedTree::new(Merge::resolved(expected));

    let merged = side1_merged.merge(&base1_merged, &side2_merged).unwrap();
    assert_eq!(merged, expected_merged);
}

/// Merge 3 trees where each one is a 3-way conflict and the result is arrived
/// at by only simplifying the conflict (no need to recurse)
#[test]
fn test_merge_simplify_only() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let path = RepoPath::from_internal_string("dir1/file");
    let tree1 = create_single_tree(repo, &[(path, "1")]);
    let tree2 = create_single_tree(repo, &[(path, "2")]);
    let tree3 = create_single_tree(repo, &[(path, "3")]);
    let tree4 = create_single_tree(repo, &[(path, "4")]);
    let tree5 = create_single_tree(repo, &[(path, "5")]);
    let expected = tree5.clone();
    let base1_merged = MergedTree::new(Merge::from_removes_adds(
        vec![tree1.clone()],
        vec![tree2.clone(), tree3.clone()],
    ));
    let side1_merged = MergedTree::new(Merge::from_removes_adds(
        vec![tree1.clone()],
        vec![tree4.clone(), tree2.clone()],
    ));
    let side2_merged = MergedTree::new(Merge::from_removes_adds(
        vec![tree4.clone()],
        vec![tree5.clone(), tree3.clone()],
    ));
    let expected_merged = MergedTree::new(Merge::resolved(expected));

    let merged = side1_merged.merge(&base1_merged, &side2_merged).unwrap();
    assert_eq!(merged, expected_merged);
}

/// Merge 3 trees with 3+1+1 terms (i.e. a 5-way conflict) such that resolving
/// the conflict between the trees leads to two trees being the same, so the
/// result is a 3-way conflict.
#[test]
fn test_merge_simplify_result() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // The conflict in path1 cannot be resolved, but the conflict in path2 can.
    let path1 = RepoPath::from_internal_string("dir1/file");
    let path2 = RepoPath::from_internal_string("dir2/file");
    let tree1 = create_single_tree(repo, &[(path1, "1"), (path2, "1")]);
    let tree2 = create_single_tree(repo, &[(path1, "2"), (path2, "2")]);
    let tree3 = create_single_tree(repo, &[(path1, "3"), (path2, "3")]);
    let tree4 = create_single_tree(repo, &[(path1, "4"), (path2, "2")]);
    let tree5 = create_single_tree(repo, &[(path1, "4"), (path2, "1")]);
    let expected_base1 = create_single_tree(repo, &[(path1, "1"), (path2, "3")]);
    let expected_side1 = create_single_tree(repo, &[(path1, "2"), (path2, "3")]);
    let expected_side2 = create_single_tree(repo, &[(path1, "3"), (path2, "3")]);
    let side1_merged = MergedTree::new(Merge::from_removes_adds(
        vec![tree1.clone()],
        vec![tree2.clone(), tree3.clone()],
    ));
    let base1_merged = MergedTree::new(Merge::resolved(tree4.clone()));
    let side2_merged = MergedTree::new(Merge::resolved(tree5.clone()));
    let expected_merged = MergedTree::new(Merge::from_removes_adds(
        vec![expected_base1],
        vec![expected_side1, expected_side2],
    ));

    let merged = side1_merged.merge(&base1_merged, &side2_merged).unwrap();
    assert_eq!(merged, expected_merged);
}

/// Test that we simplify content-level conflicts before passing them to
/// files::merge().
///
/// This is what happens when you squash a conflict resolution into a conflict
/// and it gets propagated to a child where the conflict is different.
#[test]
fn test_merge_simplify_file_conflict() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let conflict_path = RepoPath::from_internal_string("CHANGELOG.md");
    let other_path = RepoPath::from_internal_string("other");

    let prefix = r#"### New features

* The `ancestors()` revset function now takes an optional `depth` argument
  to limit the depth of the ancestor set. For example, use `jj log -r
  'ancestors(@, 5)` to view the last 5 commits.

* Support for the Watchman filesystem monitor is now bundled by default. Set
  `core.fsmonitor = "watchman"` in your repo to enable.
"#;
    let suffix = r#"
### Fixed bugs 
"#;
    let parent_base_text = format!(r#"{prefix}{suffix}"#);
    let parent_left_text = format!(
        r#"{prefix}
* `jj op log` now supports `--no-graph`.
{suffix}"#
    );
    let parent_right_text = format!(
        r#"{prefix}
* You can now configure the set of immutable commits via
  `revsets.immutable-heads`. For example, set it to `"main"` to prevent
  rewriting commits on the `main` branch.
{suffix}"#
    );
    let child1_right_text = format!(
        r#"{prefix}
* You can now configure the set of immutable commits via
  `revsets.immutable-heads`. For example, set it to `"main"` to prevent
  rewriting commits on the `main` branch. The new `immutable()` revset resolves
  to these immutable commits.
{suffix}"#
    );
    let child2_text = format!(
        r#"{prefix}
* You can now configure the set of immutable commits via
  `revsets.immutable-heads`. For example, set it to `"main"` to prevent
  rewriting commits on the `main` branch.

* `jj op log` now supports `--no-graph`.
{suffix}"#
    );
    let expected_text = format!(
        r#"{prefix}
* You can now configure the set of immutable commits via
  `revsets.immutable-heads`. For example, set it to `"main"` to prevent
  rewriting commits on the `main` branch. The new `immutable()` revset resolves
  to these immutable commits.

* `jj op log` now supports `--no-graph`.
{suffix}"#
    );

    // conflict in parent commit
    let parent_base = create_single_tree(repo, &[(conflict_path, &parent_base_text)]);
    let parent_left = create_single_tree(repo, &[(conflict_path, &parent_left_text)]);
    let parent_right = create_single_tree(repo, &[(conflict_path, &parent_right_text)]);
    let parent_merged = MergedTree::new(Merge::from_removes_adds(
        vec![parent_base],
        vec![parent_left, parent_right],
    ));

    // different conflict in child
    let child1_base = create_single_tree(
        repo,
        &[(other_path, "child1"), (conflict_path, &parent_base_text)],
    );
    let child1_left = create_single_tree(
        repo,
        &[(other_path, "child1"), (conflict_path, &parent_left_text)],
    );
    let child1_right = create_single_tree(
        repo,
        &[(other_path, "child1"), (conflict_path, &child1_right_text)],
    );
    let child1_merged = MergedTree::new(Merge::from_removes_adds(
        vec![child1_base],
        vec![child1_left, child1_right],
    ));

    // resolved state
    let child2 = create_single_tree(repo, &[(conflict_path, &child2_text)]);
    let child2_merged = MergedTree::resolved(child2.clone());

    // expected result
    let expected = create_single_tree(
        repo,
        &[(other_path, "child1"), (conflict_path, &expected_text)],
    );
    let expected_merged = MergedTree::resolved(expected);

    let merged = child1_merged.merge(&parent_merged, &child2_merged).unwrap();
    assert_eq!(merged, expected_merged);

    // Also test the setup by checking that the unsimplified content conflict cannot
    // be resolved. If we later change files::merge() so this no longer fails,  it
    // probably means that we can delete this whole test (the Merge::simplify() call
    // in try_resolve_file_conflict() is just an optimization then).
    let text_merge = Merge::from_removes_adds(
        vec![Merge::from_removes_adds(
            vec![parent_base_text.as_bytes()],
            vec![parent_left_text.as_bytes(), parent_right_text.as_bytes()],
        )],
        vec![
            Merge::from_removes_adds(
                vec![parent_base_text.as_bytes()],
                vec![parent_left_text.as_bytes(), child1_right_text.as_bytes()],
            ),
            Merge::resolved(child2_text.as_bytes()),
        ],
    );
    assert!(matches!(
        jj_lib::files::merge(&text_merge.flatten()),
        MergeResult::Conflict(_)
    ));
}

/// Like `test_merge_simplify_file_conflict()`, but some of the conflicts are
/// absent.
#[test]
fn test_merge_simplify_file_conflict_with_absent() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // conflict_path doesn't exist in parent and child2_left, and these two
    // trees can't be canceled out at the root level. Still the file merge
    // should succeed by eliminating absent entries.
    let child2_path = RepoPath::from_internal_string("file_child2");
    let conflict_path = RepoPath::from_internal_string("dir/file_conflict");
    let child1 = create_single_tree(repo, &[(conflict_path, "1\n0\n")]);
    let parent = create_single_tree(repo, &[]);
    let child2_left = create_single_tree(repo, &[(child2_path, "")]);
    let child2_base = create_single_tree(repo, &[(child2_path, ""), (conflict_path, "0\n")]);
    let child2_right = create_single_tree(repo, &[(child2_path, ""), (conflict_path, "0\n2\n")]);
    let child1_merged = MergedTree::resolved(child1);
    let parent_merged = MergedTree::resolved(parent);
    let child2_merged = MergedTree::new(Merge::from_removes_adds(
        vec![child2_base],
        vec![child2_left, child2_right],
    ));

    let expected = create_single_tree(repo, &[(child2_path, ""), (conflict_path, "1\n0\n2\n")]);
    let expected_merged = MergedTree::resolved(expected);

    let merged = child1_merged.merge(&parent_merged, &child2_merged).unwrap();
    assert_eq!(merged, expected_merged);
}
