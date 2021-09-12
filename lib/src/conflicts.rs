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

use std::cmp::min;
use std::io::{Cursor, Write};

use itertools::Itertools;

use crate::backend::{Conflict, ConflictPart, TreeValue};
use crate::diff::{find_line_ranges, Diff, DiffHunk};
use crate::files;
use crate::files::{MergeHunk, MergeResult};
use crate::repo_path::RepoPath;
use crate::store::Store;

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

fn get_file_contents(store: &Store, path: &RepoPath, part: &ConflictPart) -> Vec<u8> {
    if let TreeValue::Normal {
        id,
        executable: false,
    } = &part.value
    {
        let mut content: Vec<u8> = vec![];
        store
            .read_file(path, id)
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        content
    } else {
        panic!("unexpectedly got a non-file conflict part");
    }
}

fn write_diff_hunks(left: &[u8], right: &[u8], file: &mut dyn Write) -> std::io::Result<()> {
    let diff = Diff::for_tokenizer(&[left, right], &find_line_ranges);
    for hunk in diff.hunks() {
        match hunk {
            DiffHunk::Matching(content) => {
                for line in content.split_inclusive(|b| *b == b'\n') {
                    file.write_all(b" ")?;
                    file.write_all(line)?;
                }
            }
            DiffHunk::Different(content) => {
                for line in content[0].split_inclusive(|b| *b == b'\n') {
                    file.write_all(b"-")?;
                    file.write_all(line)?;
                }
                for line in content[1].split_inclusive(|b| *b == b'\n') {
                    file.write_all(b"+")?;
                    file.write_all(line)?;
                }
            }
        }
    }
    Ok(())
}

pub fn materialize_conflict(
    store: &Store,
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

    let added_content = file_adds
        .iter()
        .map(|part| get_file_contents(store, path, part))
        .collect_vec();
    let removed_content = file_removes
        .iter()
        .map(|part| get_file_contents(store, path, part))
        .collect_vec();
    let removed_slices = removed_content
        .iter()
        .map(|vec| vec.as_slice())
        .collect_vec();
    let added_slices = added_content.iter().map(|vec| vec.as_slice()).collect_vec();

    let merge_result = files::merge(&removed_slices, &added_slices);
    match merge_result {
        MergeResult::Resolved(content) => {
            file.write_all(&content).unwrap();
        }
        MergeResult::Conflict(hunks) => {
            for hunk in hunks {
                match hunk {
                    MergeHunk::Resolved(content) => {
                        file.write_all(&content).unwrap();
                    }
                    MergeHunk::Conflict { removes, adds } => {
                        let num_diffs = min(removes.len(), adds.len());

                        // TODO: Pair up a remove with an add in a way that minimizes the size of
                        // the diff
                        file.write_all(b"<<<<<<<\n").unwrap();
                        for i in 0..num_diffs {
                            file.write_all(b"-------\n").unwrap();
                            file.write_all(b"+++++++\n").unwrap();
                            write_diff_hunks(&removes[i], &adds[i], file).unwrap();
                        }
                        for slice in removes.iter().skip(num_diffs) {
                            file.write_all(b"-------\n").unwrap();
                            file.write_all(slice).unwrap();
                        }
                        for slice in adds.iter().skip(num_diffs) {
                            file.write_all(b"+++++++\n").unwrap();
                            file.write_all(slice).unwrap();
                        }
                        file.write_all(b">>>>>>>\n").unwrap();
                    }
                }
            }
        }
    }
}

pub fn conflict_to_materialized_value(
    store: &Store,
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
