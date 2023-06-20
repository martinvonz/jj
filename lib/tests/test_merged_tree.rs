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

use jj_lib::backend::{FileId, TreeValue};
use jj_lib::conflicts::Conflict;
use jj_lib::merged_tree::{MergedTree, MergedTreeValue};
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
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
    let file2_conflict = Conflict::new(
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
    let file3_conflict = Conflict::new(
        vec![Some(file_value(&file3_v1_id))],
        vec![Some(file_value(&file3_v2_id)), None],
    );
    let file3_conflict_id = store.write_conflict(&file3_path, &file3_conflict).unwrap();
    tree_builder.set(file3_path.clone(), TreeValue::Conflict(file3_conflict_id));

    // file4: add/add conflict
    let file4_path = RepoPath::from_internal_string("add_add");
    let file4_v1_id = write_file(store.as_ref(), &file4_path, "file4_v1");
    let file4_v2_id = write_file(store.as_ref(), &file4_path, "file4_v2");
    let file4_conflict = Conflict::new(
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
    let file5_conflict = Conflict::new(
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
        MergedTreeValue::Conflict(Conflict::new(
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
        MergedTreeValue::Conflict(Conflict::new(
            vec![Some(file_value(&file3_v1_id)), None],
            vec![Some(file_value(&file3_v2_id)), None, None],
        ))
    );
    // file4: add/add conflict
    assert_eq!(
        merged_tree.value(&file4_path.components()[0]),
        MergedTreeValue::Conflict(Conflict::new(
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
        MergedTreeValue::Conflict(Conflict::new(
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
