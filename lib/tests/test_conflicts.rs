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
use jujutsu_lib::conflicts::materialize_conflict;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::testutils;

#[test]
fn test_materialize_conflict_basic() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, false);

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        repo.store(),
        &path,
        "line 1
line 2
line 3
line 4
line 5
",
    );
    let left_id = testutils::write_file(
        repo.store(),
        &path,
        "line 1
line 2
left
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        repo.store(),
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
    materialize_conflict(repo.store(), &path, &conflict, &mut result).unwrap();
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
    let (_temp_dir, repo) = testutils::init_repo(&settings, false);

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        repo.store(),
        &path,
        "line 1
line 2
line 3
line 4
line 5
",
    );
    let left_id = testutils::write_file(
        repo.store(),
        &path,
        "line 1
line 2
left
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        repo.store(),
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
    materialize_conflict(repo.store(), &path, &conflict, &mut result).unwrap();
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
    let (_temp_dir, repo) = testutils::init_repo(&settings, false);

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        repo.store(),
        &path,
        "line 1
line 2
line 3
line 4
line 5
",
    );
    let left_id = testutils::write_file(
        repo.store(),
        &path,
        "line 1
line 2
line 4
line 5
",
    );
    let right_id = testutils::write_file(
        repo.store(),
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
    materialize_conflict(repo.store(), &path, &conflict, &mut result).unwrap();
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
