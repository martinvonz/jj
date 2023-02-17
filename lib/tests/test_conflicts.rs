// Copyright 2021 The Jujutsu Authors
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

use jujutsu_lib::backend::{Conflict, ConflictPart, FileId, TreeValue};
use jujutsu_lib::conflicts::{materialize_conflict, parse_conflict, update_conflict_from_content};
use jujutsu_lib::files::{ConflictHunk, MergeHunk};
use jujutsu_lib::repo::Repo;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::store::Store;
use testutils::TestRepo;

fn file_conflict_part(file_id: &FileId) -> ConflictPart {
    ConflictPart {
        value: TreeValue::File {
            id: file_id.clone(),
            executable: false,
        },
    }
}

#[test]
fn test_materialize_conflict_basic() {
    let test_repo = TestRepo::init(false);
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
line 3
line 4
line 5
",
    );
    let left_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
left 3.1
left 3.2
left 3.3
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
right 3.1
line 4
line 5
",
    );

    let mut conflict = Conflict {
        removes: vec![file_conflict_part(&base_id)],
        adds: vec![file_conflict_part(&left_id), file_conflict_part(&right_id)],
    };
    insta::assert_snapshot!(
        &materialize_conflict_string(store, &path, &conflict),
        @r###"
    line 1
    line 2
    <<<<<<<
    %%%%%%%
    -line 3
    +right 3.1
    +++++++
    left 3.1
    left 3.2
    left 3.3
    >>>>>>>
    line 4
    line 5
    "###
    );
    // Test with the larger diff first. We still want the small diff.
    conflict.adds.reverse();
    insta::assert_snapshot!(
        &materialize_conflict_string(store, &path, &conflict),
        @r###"
    line 1
    line 2
    <<<<<<<
    %%%%%%%
    -line 3
    +right 3.1
    +++++++
    left 3.1
    left 3.2
    left 3.3
    >>>>>>>
    line 4
    line 5
    "###
    );
}

#[test]
fn test_materialize_conflict_modify_delete() {
    let test_repo = TestRepo::init(false);
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
line 3
line 4
line 5
",
    );
    let modified_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
modified
line 4
line 5
",
    );
    let deleted_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
line 4
line 5
",
    );

    // left modifies a line, right deletes the same line.
    let conflict = Conflict {
        removes: vec![file_conflict_part(&base_id)],
        adds: vec![
            file_conflict_part(&modified_id),
            file_conflict_part(&deleted_id),
        ],
    };
    insta::assert_snapshot!(&materialize_conflict_string(store, &path, &conflict), @r###"
    line 1
    line 2
    <<<<<<<
    %%%%%%%
    -line 3
    +++++++
    modified
    >>>>>>>
    line 4
    line 5
    "###
    );

    // right modifies a line, left deletes the same line.
    let conflict = Conflict {
        removes: vec![file_conflict_part(&base_id)],
        adds: vec![
            file_conflict_part(&deleted_id),
            file_conflict_part(&modified_id),
        ],
    };
    insta::assert_snapshot!(&materialize_conflict_string(store, &path, &conflict), @r###"
    line 1
    line 2
    <<<<<<<
    %%%%%%%
    -line 3
    +++++++
    modified
    >>>>>>>
    line 4
    line 5
    "###
    );

    // modify/delete conflict at the file level
    let conflict = Conflict {
        removes: vec![file_conflict_part(&base_id)],
        adds: vec![file_conflict_part(&modified_id)],
    };
    // TODO: THis should have context around the conflict (#1244)
    insta::assert_snapshot!(&materialize_conflict_string(store, &path, &conflict), @r###"
    <<<<<<<
    %%%%%%%
    -line 3
    +modified
    >>>>>>>
    "###
    );
}

#[test]
fn test_parse_conflict_resolved() {
    assert_eq!(
        parse_conflict(
            b"line 1
line 2
line 3
line 4
line 5
",
            1,
            2
        ),
        None
    )
}

#[test]
fn test_parse_conflict_simple() {
    assert_eq!(
        parse_conflict(
            b"line 1
<<<<<<<
%%%%%%%
 line 2
-line 3
+left
 line 4
+++++++
right
>>>>>>>
line 5
",
            1,
            2
        ),
        Some(vec![
            MergeHunk::Resolved(b"line 1\n".to_vec()),
            MergeHunk::Conflict(ConflictHunk {
                removes: vec![b"line 2\nline 3\nline 4\n".to_vec()],
                adds: vec![b"line 2\nleft\nline 4\n".to_vec(), b"right\n".to_vec()]
            }),
            MergeHunk::Resolved(b"line 5\n".to_vec())
        ])
    )
}

#[test]
fn test_parse_conflict_multi_way() {
    assert_eq!(
        parse_conflict(
            b"line 1
<<<<<<<
%%%%%%%
 line 2
-line 3
+left
 line 4
+++++++
right
%%%%%%%
 line 2
+forward
 line 3
 line 4
>>>>>>>
line 5
",
            2,
            3
        ),
        Some(vec![
            MergeHunk::Resolved(b"line 1\n".to_vec()),
            MergeHunk::Conflict(ConflictHunk {
                removes: vec![
                    b"line 2\nline 3\nline 4\n".to_vec(),
                    b"line 2\nline 3\nline 4\n".to_vec()
                ],
                adds: vec![
                    b"line 2\nleft\nline 4\n".to_vec(),
                    b"right\n".to_vec(),
                    b"line 2\nforward\nline 3\nline 4\n".to_vec()
                ]
            }),
            MergeHunk::Resolved(b"line 5\n".to_vec())
        ])
    )
}

#[test]
fn test_parse_conflict_different_wrong_arity() {
    assert_eq!(
        parse_conflict(
            b"line 1
<<<<<<<
%%%%%%%
 line 2
-line 3
+left
 line 4
+++++++
right
>>>>>>>
line 5
",
            2,
            3
        ),
        None
    )
}

#[test]
fn test_parse_conflict_malformed_marker() {
    // The conflict marker is missing `%%%%%%%`
    assert_eq!(
        parse_conflict(
            b"line 1
<<<<<<<
 line 2
-line 3
+left
 line 4
+++++++
right
>>>>>>>
line 5
",
            1,
            2
        ),
        None
    )
}

#[test]
fn test_parse_conflict_malformed_diff() {
    // The diff part is invalid (missing space before "line 4")
    assert_eq!(
        parse_conflict(
            b"line 1
<<<<<<<
%%%%%%%
 line 2
-line 3
+left
line 4
+++++++
right
>>>>>>>
line 5
",
            1,
            2
        ),
        None
    )
}

#[test]
fn test_update_conflict_from_content() {
    let test_repo = TestRepo::init(false);
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("dir/file");
    let base_file_id = testutils::write_file(store, &path, "line 1\nline 2\nline 3\n");
    let left_file_id = testutils::write_file(store, &path, "left 1\nline 2\nleft 3\n");
    let right_file_id = testutils::write_file(store, &path, "right 1\nline 2\nright 3\n");
    let conflict = Conflict {
        removes: vec![file_conflict_part(&base_file_id)],
        adds: vec![
            file_conflict_part(&left_file_id),
            file_conflict_part(&right_file_id),
        ],
    };
    let conflict_id = store.write_conflict(&path, &conflict).unwrap();

    // If the content is unchanged compared to the materialized value, we get the
    // old conflict id back.
    let mut materialized = vec![];
    materialize_conflict(store, &path, &conflict, &mut materialized).unwrap();
    let result = update_conflict_from_content(store, &path, &conflict_id, &materialized).unwrap();
    assert_eq!(result, Some(conflict_id.clone()));

    // If the conflict is resolved, we None back to indicate that.
    let result = update_conflict_from_content(
        store,
        &path,
        &conflict_id,
        b"resolved 1\nline 2\nresolved 3\n",
    )
    .unwrap();
    assert_eq!(result, None);

    // If the conflict is partially resolved, we get a new conflict back.
    let result = update_conflict_from_content(
        store,
        &path,
        &conflict_id,
        b"resolved 1\nline 2\n<<<<<<<\n%%%%%%%\n-line 3\n+left 3\n+++++++\nright 3\n>>>>>>>\n",
    )
    .unwrap();
    assert_ne!(result, None);
    assert_ne!(result, Some(conflict_id));
    let new_conflict = store.read_conflict(&path, &result.unwrap()).unwrap();
    // Calculate expected new FileIds
    let new_base_file_id = testutils::write_file(store, &path, "resolved 1\nline 2\nline 3\n");
    let new_left_file_id = testutils::write_file(store, &path, "resolved 1\nline 2\nleft 3\n");
    let new_right_file_id = testutils::write_file(store, &path, "resolved 1\nline 2\nright 3\n");
    assert_eq!(
        new_conflict,
        Conflict {
            removes: vec![file_conflict_part(&new_base_file_id)],
            adds: vec![
                file_conflict_part(&new_left_file_id),
                file_conflict_part(&new_right_file_id)
            ]
        }
    )
}

fn materialize_conflict_string(store: &Store, path: &RepoPath, conflict: &Conflict) -> String {
    let mut result: Vec<u8> = vec![];
    materialize_conflict(store, path, conflict, &mut result).unwrap();
    String::from_utf8(result).unwrap()
}
