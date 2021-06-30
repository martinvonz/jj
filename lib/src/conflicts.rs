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

use std::io::{Cursor, Write};

use itertools::Itertools;

use crate::files;
use crate::repo_path::RepoPath;
use crate::store::{Conflict, ConflictPart, TreeValue};
use crate::store_wrapper::StoreWrapper;

fn describe_conflict_part(part: &ConflictPart) -> String {
    match &part.value {
        TreeValue::Normal {
            id,
            executable: false,
        } => {
            format!("file with id {}", id.hex())
        }
        TreeValue::Normal {
            id,
            executable: true,
        } => {
            format!("executable file with id {}", id.hex())
        }
        TreeValue::Symlink(id) => {
            format!("symlink with id {}", id.hex())
        }
        TreeValue::Tree(id) => {
            format!("tree with id {}", id.hex())
        }
        TreeValue::GitSubmodule(id) => {
            format!("Git submodule with id {}", id.hex())
        }
        TreeValue::Conflict(id) => {
            format!("Conflict with id {}", id.hex())
        }
    }
}

fn describe_conflict(conflict: &Conflict, file: &mut dyn Write) -> std::io::Result<()> {
    file.write_all(b"Conflict:\n")?;
    for part in &conflict.removes {
        file.write_all(format!("  Removing {}\n", describe_conflict_part(part)).as_bytes())?;
    }
    for part in &conflict.adds {
        file.write_all(format!("  Adding {}\n", describe_conflict_part(part)).as_bytes())?;
    }
    Ok(())
}

fn file_parts(parts: &[ConflictPart]) -> Vec<&ConflictPart> {
    parts
        .iter()
        .filter(|part| {
            matches!(
                part.value,
                TreeValue::Normal {
                    executable: false,
                    ..
                }
            )
        })
        .collect_vec()
}

pub fn materialize_conflict(
    store: &StoreWrapper,
    path: &RepoPath,
    conflict: &Conflict,
    file: &mut dyn Write,
) {
    let file_adds = file_parts(&conflict.adds);
    let file_removes = file_parts(&conflict.removes);
    if file_adds.len() != conflict.adds.len() || file_removes.len() != conflict.removes.len() {
        // Unless all parts are regular files, we can't do much better than to try to
        // describe the conflict.
        describe_conflict(conflict, file).unwrap();
        return;
    }

    match conflict.to_three_way() {
        None => {
            file.write_all(b"Unresolved complex conflict.\n").unwrap();
        }
        Some((Some(left), Some(base), Some(right))) => {
            match (left.value, base.value, right.value) {
                (
                    TreeValue::Normal {
                        id: left_id,
                        executable: false,
                    },
                    TreeValue::Normal {
                        id: base_id,
                        executable: false,
                    },
                    TreeValue::Normal {
                        id: right_id,
                        executable: false,
                    },
                ) => {
                    let mut left_contents: Vec<u8> = vec![];
                    let mut base_contents: Vec<u8> = vec![];
                    let mut right_contents: Vec<u8> = vec![];
                    store
                        .read_file(path, &left_id)
                        .unwrap()
                        .read_to_end(&mut left_contents)
                        .unwrap();
                    store
                        .read_file(path, &base_id)
                        .unwrap()
                        .read_to_end(&mut base_contents)
                        .unwrap();
                    store
                        .read_file(path, &right_id)
                        .unwrap()
                        .read_to_end(&mut right_contents)
                        .unwrap();
                    let merge_result =
                        files::merge(&[&base_contents], &[&left_contents, &right_contents]);
                    match merge_result {
                        files::MergeResult::Resolved(contents) => {
                            file.write_all(&contents).unwrap();
                        }
                        files::MergeResult::Conflict(hunks) => {
                            for hunk in hunks {
                                match hunk {
                                    files::MergeHunk::Resolved(contents) => {
                                        file.write_all(&contents).unwrap();
                                    }
                                    files::MergeHunk::Conflict { removes, adds } => {
                                        file.write_all(b"<<<<<<<\n").unwrap();
                                        file.write_all(&adds[0]).unwrap();
                                        file.write_all(b"|||||||\n").unwrap();
                                        file.write_all(&removes[0]).unwrap();
                                        file.write_all(b"=======\n").unwrap();
                                        file.write_all(&adds[1]).unwrap();
                                        file.write_all(b">>>>>>>\n").unwrap();
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    file.write_all(b"Unresolved 3-way conflict.\n").unwrap();
                }
            }
        }
        Some(_) => {
            file.write_all(b"Unresolved complex conflict.\n").unwrap();
        }
    }
}

pub fn conflict_to_materialized_value(
    store: &StoreWrapper,
    path: &RepoPath,
    conflict: &Conflict,
) -> TreeValue {
    let mut buf = vec![];
    materialize_conflict(store, path, conflict, &mut buf);
    let file_id = store.write_file(path, &mut Cursor::new(&buf)).unwrap();
    TreeValue::Normal {
        id: file_id,
        executable: false,
    }
}
