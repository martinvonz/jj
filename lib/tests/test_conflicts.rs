// Copyright 2021 Google LLC
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

use jujutsu_lib::backend::{Conflict, ConflictPart, TreeValue};
use jujutsu_lib::conflicts::{materialize_conflict, parse_conflict, update_conflict_from_content};
use jujutsu_lib::files::MergeHunk;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::testutils;

#[test]
fn test_materialize_conflict_basic() {
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_workspace(&settings, false);
    let store = test_workspace.repo.store();

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
left
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
right
line 4
line 5
",
    );

    let conflict = Conflict {
        removes: vec![ConflictPart {
            value: TreeValue::Normal {
                id: base_id,
                executable: false,
            },
        }],
        adds: vec![
            ConflictPart {
                value: TreeValue::Normal {
                    id: left_id,
                    executable: false,
                },
            },
            ConflictPart {
                value: TreeValue::Normal {
                    id: right_id,
                    executable: false,
                },
            },
        ],
    };
    let mut result: Vec<u8> = vec![];
    materialize_conflict(store, &path, &conflict, &mut result).unwrap();
    assert_eq!(
        String::from_utf8(result).unwrap().as_str(),
        "line 1
line 2
<<<<<<<
-------
+++++++
-line 3
+left
+++++++
right
>>>>>>>
line 4
line 5
"
    );
}

#[test]
fn test_materialize_conflict_modify_delete() {
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_workspace(&settings, false);
    let store = test_workspace.repo.store();

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
left
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
line 4
line 5
",
    );

    let conflict = Conflict {
        removes: vec![ConflictPart {
            value: TreeValue::Normal {
                id: base_id,
                executable: false,
            },
        }],
        adds: vec![
            ConflictPart {
                value: TreeValue::Normal {
                    id: left_id,
                    executable: false,
                },
            },
            ConflictPart {
                value: TreeValue::Normal {
                    id: right_id,
                    executable: false,
                },
            },
        ],
    };
    let mut result: Vec<u8> = vec![];
    materialize_conflict(store, &path, &conflict, &mut result).unwrap();
    assert_eq!(
        String::from_utf8(result).unwrap().as_str(),
        "line 1
line 2
<<<<<<<
-------
+++++++
-line 3
+left
+++++++
>>>>>>>
line 4
line 5
"
    );
}

#[test]
fn test_materialize_conflict_delete_modify() {
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_workspace(&settings, false);
    let store = test_workspace.repo.store();

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
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        store,
        &path,
        "line 1
line 2
right
line 4
line 5
",
    );

    let conflict = Conflict {
        removes: vec![ConflictPart {
            value: TreeValue::Normal {
                id: base_id,
                executable: false,
            },
        }],
        adds: vec![
            ConflictPart {
                value: TreeValue::Normal {
                    id: left_id,
                    executable: false,
                },
            },
            ConflictPart {
                value: TreeValue::Normal {
                    id: right_id,
                    executable: false,
                },
            },
        ],
    };

    let mut result: Vec<u8> = vec![];
    materialize_conflict(store, &path, &conflict, &mut result).unwrap();
    assert_eq!(
        String::from_utf8(result).unwrap().as_str(),
        "line 1
line 2
<<<<<<<
-------
+++++++
-line 3
+++++++
right
>>>>>>>
line 4
line 5
"
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
-------
+++++++
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
            MergeHunk::Conflict {
                removes: vec![b"line 2\nline 3\nline 4\n".to_vec()],
                adds: vec![b"line 2\nleft\nline 4\n".to_vec(), b"right\n".to_vec()]
            },
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
-------
+++++++
 line 2
-line 3
+left
 line 4
+++++++
right
-------
+++++++
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
            MergeHunk::Conflict {
                removes: vec![
                    b"line 2\nline 3\nline 4\n".to_vec(),
                    b"line 2\nline 3\nline 4\n".to_vec()
                ],
                adds: vec![
                    b"line 2\nleft\nline 4\n".to_vec(),
                    b"right\n".to_vec(),
                    b"line 2\nforward\nline 3\nline 4\n".to_vec()
                ]
            },
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
-------
+++++++
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
    // The conflict marker is missing `-------` and `+++++++` (it needs at least one
    // of them)
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
-------
+++++++
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
    let settings = testutils::user_settings();
    let test_workspace = testutils::init_workspace(&settings, false);
    let store = test_workspace.repo.store();

    let path = RepoPath::from_internal_string("dir/file");
    let base_file_id = testutils::write_file(store, &path, "line 1\nline 2\nline 3\n");
    let left_file_id = testutils::write_file(store, &path, "left 1\nline 2\nleft 3\n");
    let right_file_id = testutils::write_file(store, &path, "right 1\nline 2\nright 3\n");
    let conflict = Conflict {
        removes: vec![ConflictPart {
            value: TreeValue::Normal {
                id: base_file_id,
                executable: false,
            },
        }],
        adds: vec![
            ConflictPart {
                value: TreeValue::Normal {
                    id: left_file_id,
                    executable: false,
                },
            },
            ConflictPart {
                value: TreeValue::Normal {
                    id: right_file_id,
                    executable: false,
                },
            },
        ],
    };
    let conflict_id = store.write_conflict(&conflict).unwrap();

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
    let result = update_conflict_from_content(store, &path, &conflict_id, b"resolved 1\nline 2\n<<<<<<<\n-------\n+++++++\n-line 3\n+left 3\n+++++++\nright 3\n>>>>>>>\n").unwrap();
    assert_ne!(result, None);
    assert_ne!(result, Some(conflict_id));
    let new_conflict = store.read_conflict(&result.unwrap()).unwrap();
    // Calculate expected new FileIds
    let new_base_file_id = testutils::write_file(store, &path, "resolved 1\nline 2\nline 3\n");
    let new_left_file_id = testutils::write_file(store, &path, "resolved 1\nline 2\nleft 3\n");
    let new_right_file_id = testutils::write_file(store, &path, "resolved 1\nline 2\nright 3\n");
    assert_eq!(
        new_conflict,
        Conflict {
            removes: vec![ConflictPart {
                value: TreeValue::Normal {
                    id: new_base_file_id,
                    executable: false
                }
            }],
            adds: vec![
                ConflictPart {
                    value: TreeValue::Normal {
                        id: new_left_file_id,
                        executable: false
                    }
                },
                ConflictPart {
                    value: TreeValue::Normal {
                        id: new_right_file_id,
                        executable: false
                    }
                }
            ]
        }
    )
}
