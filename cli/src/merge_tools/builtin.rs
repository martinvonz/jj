use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use futures::TryFutureExt;
use futures::TryStreamExt;
use itertools::Itertools;
use jj_lib::backend::BackendError;
use jj_lib::backend::BackendResult;
use jj_lib::backend::FileId;
use jj_lib::backend::MergedTreeId;
use jj_lib::backend::TreeValue;
use jj_lib::conflicts::materialize_tree_value;
use jj_lib::conflicts::MaterializedTreeValue;
use jj_lib::diff::Diff;
use jj_lib::diff::DiffHunk;
use jj_lib::files;
use jj_lib::files::ContentHunk;
use jj_lib::files::MergeResult;
use jj_lib::matchers::Matcher;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree::MergedTreeBuilder;
use jj_lib::merged_tree::TreeDiffEntry;
use jj_lib::object_id::ObjectId;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::store::Store;
use pollster::FutureExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuiltinToolError {
    #[error("Failed to record changes")]
    Record(#[from] scm_record::RecordError),
    #[error(transparent)]
    ReadFileBackend(BackendError),
    #[error("Failed to read file {path:?} with ID {id}", id = id.hex())]
    ReadFileIo {
        path: RepoPathBuf,
        id: FileId,
        source: std::io::Error,
    },
    #[error(transparent)]
    ReadSymlink(BackendError),
    #[error("Failed to decode UTF-8 text for item {item} (this should not happen)")]
    DecodeUtf8 {
        source: std::str::Utf8Error,
        item: &'static str,
    },
    #[error("Rendering {item} {id} is unimplemented for the builtin difftool/mergetool")]
    Unimplemented { item: &'static str, id: String },
    #[error("Backend error")]
    BackendError(#[from] jj_lib::backend::BackendError),
}

#[derive(Clone, Debug)]
enum FileContents {
    Absent,
    Text {
        contents: String,
        hash: Option<String>,
        num_bytes: u64,
    },
    Binary {
        hash: Option<String>,
        num_bytes: u64,
    },
}

/// Information about a file that was read from disk. Note that the file may not
/// have existed, in which case its contents will be marked as absent.
#[derive(Clone, Debug)]
pub struct FileInfo {
    file_mode: scm_record::FileMode,
    contents: FileContents,
}

impl FileInfo {
    // Returns `Some(true)` if the file exists and is empty.
    // Returns `None` if the file is absent and does not exist.
    fn is_empty(&self) -> Option<bool> {
        match self.contents {
            FileContents::Absent => {
                // If the contents are `Absent` the mode should be `Absent` as
                // well since the file doesn't exist.
                debug_assert!(self.file_mode == scm_record::FileMode::absent());
                None
            }
            FileContents::Text {
                contents: _,
                hash: _,
                num_bytes,
            }
            | FileContents::Binary { hash: _, num_bytes } => Some(num_bytes == 0),
        }
    }
}

/// File modes according to the Git file mode conventions. used for display
/// purposes and equality comparison.
///
/// TODO: let `scm-record` accept strings instead of numbers for file modes? Or
/// figure out some other way to represent file mode changes in a jj-compatible
/// manner?
mod mode {
    pub const NORMAL: usize = 0o100644;
    pub const EXECUTABLE: usize = 0o100755;
    pub const SYMLINK: usize = 0o120000;
}

fn describe_binary(hash: Option<&str>, num_bytes: u64) -> String {
    match hash {
        Some(hash) => {
            format!("{hash} ({num_bytes}B)")
        }
        None => format!("({num_bytes}B)"),
    }
}

fn buf_to_file_contents(hash: Option<String>, buf: Vec<u8>) -> FileContents {
    let num_bytes: u64 = buf.len().try_into().unwrap();
    let text = if buf.contains(&0) {
        None
    } else {
        String::from_utf8(buf).ok()
    };
    match text {
        Some(text) => FileContents::Text {
            contents: text,
            hash,
            num_bytes,
        },
        None => FileContents::Binary { hash, num_bytes },
    }
}

fn read_file_contents(
    store: &Store,
    tree: &MergedTree,
    path: &RepoPath,
) -> Result<FileInfo, BuiltinToolError> {
    let value = tree.path_value(path)?;
    let materialized_value = materialize_tree_value(store, path, value)
        .map_err(BuiltinToolError::BackendError)
        .block_on()?;
    match materialized_value {
        MaterializedTreeValue::Absent => Ok(FileInfo {
            file_mode: scm_record::FileMode::absent(),
            contents: FileContents::Absent,
        }),
        MaterializedTreeValue::AccessDenied(err) => Ok(FileInfo {
            file_mode: scm_record::FileMode(mode::NORMAL),
            contents: FileContents::Text {
                contents: format!("Access denied: {err}"),
                hash: None,
                num_bytes: 0,
            },
        }),

        MaterializedTreeValue::File {
            id,
            executable,
            mut reader,
        } => {
            let mut buf = Vec::new();
            reader
                .read_to_end(&mut buf)
                .map_err(|err| BuiltinToolError::ReadFileIo {
                    path: path.to_owned(),
                    id: id.clone(),
                    source: err,
                })?;

            let file_mode = if executable {
                scm_record::FileMode(mode::EXECUTABLE)
            } else {
                scm_record::FileMode(mode::NORMAL)
            };
            let contents = buf_to_file_contents(Some(id.hex()), buf);
            Ok(FileInfo {
                file_mode,
                contents,
            })
        }

        MaterializedTreeValue::Symlink { id, target } => {
            let file_mode = scm_record::FileMode(mode::SYMLINK);
            let num_bytes = target.len().try_into().unwrap();
            Ok(FileInfo {
                file_mode,
                contents: FileContents::Text {
                    contents: target,
                    hash: Some(id.hex()),
                    num_bytes,
                },
            })
        }

        MaterializedTreeValue::Tree(tree_id) => {
            unreachable!("list of changed files included a tree: {tree_id:?}");
        }
        MaterializedTreeValue::GitSubmodule(id) => Err(BuiltinToolError::Unimplemented {
            item: "git submodule",
            id: id.hex(),
        }),
        MaterializedTreeValue::Conflict {
            id: _,
            contents,
            executable: _,
        } => {
            // TODO: Render the ID somehow?
            let contents = buf_to_file_contents(None, contents);
            Ok(FileInfo {
                file_mode: scm_record::FileMode(mode::NORMAL),
                contents,
            })
        }
    }
}

fn make_section_changed_lines(
    contents: &str,
    change_type: scm_record::ChangeType,
) -> Vec<scm_record::SectionChangedLine<'static>> {
    contents
        .split_inclusive('\n')
        .map(|line| scm_record::SectionChangedLine {
            is_checked: false,
            change_type,
            line: Cow::Owned(line.to_owned()),
        })
        .collect()
}

fn make_diff_sections(
    left_contents: &str,
    right_contents: &str,
) -> Result<Vec<scm_record::Section<'static>>, BuiltinToolError> {
    let diff = Diff::by_line([left_contents.as_bytes(), right_contents.as_bytes()]);
    let mut sections = Vec::new();
    for hunk in diff.hunks() {
        match hunk {
            DiffHunk::Matching(text) => {
                let text =
                    std::str::from_utf8(text).map_err(|err| BuiltinToolError::DecodeUtf8 {
                        source: err,
                        item: "matching text in diff hunk",
                    })?;
                sections.push(scm_record::Section::Unchanged {
                    lines: text
                        .split_inclusive('\n')
                        .map(|line| Cow::Owned(line.to_owned()))
                        .collect(),
                })
            }
            DiffHunk::Different(sides) => {
                assert_eq!(sides.len(), 2, "only two inputs were provided to the diff");
                let left_side =
                    std::str::from_utf8(sides[0]).map_err(|err| BuiltinToolError::DecodeUtf8 {
                        source: err,
                        item: "left side of diff hunk",
                    })?;
                let right_side =
                    std::str::from_utf8(sides[1]).map_err(|err| BuiltinToolError::DecodeUtf8 {
                        source: err,
                        item: "right side of diff hunk",
                    })?;
                sections.push(scm_record::Section::Changed {
                    lines: [
                        make_section_changed_lines(left_side, scm_record::ChangeType::Removed),
                        make_section_changed_lines(right_side, scm_record::ChangeType::Added),
                    ]
                    .concat(),
                })
            }
        }
    }
    Ok(sections)
}

fn should_render_mode_section(left: &FileInfo, right: &FileInfo) -> bool {
    match (left.is_empty(), right.is_empty()) {
        // The file only exists on one side, but it's not empty on the other
        // side. We'll render a section for the change in content, so the mode
        // change is implied and mandatory if the user selects the change in
        // content.
        (Some(false), None) | (None, Some(false)) => false,
        _ => left.file_mode != right.file_mode,
    }
}

pub fn make_diff_files(
    store: &Arc<Store>,
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    changed_files: &[RepoPathBuf],
) -> Result<Vec<scm_record::File<'static>>, BuiltinToolError> {
    let mut files = Vec::new();
    for changed_path in changed_files {
        let left_info = read_file_contents(store, left_tree, changed_path)?;
        let right_info = read_file_contents(store, right_tree, changed_path)?;
        let mut sections = Vec::new();

        if should_render_mode_section(&left_info, &right_info) {
            sections.push(scm_record::Section::FileMode {
                is_checked: false,
                before: left_info.file_mode,
                after: right_info.file_mode,
            });
        }

        match (left_info.contents, right_info.contents) {
            (FileContents::Absent, FileContents::Absent) => {}
            // In this context, `Absent` means the file doesn't exist. If it only
            // exists on one side, we will render a mode change section above.
            // The next two patterns are to avoid also rendering an empty
            // changed lines section that clutters the UI.
            (
                FileContents::Absent,
                FileContents::Text {
                    contents: _,
                    hash: _,
                    num_bytes: 0,
                },
            ) => {}
            (
                FileContents::Text {
                    contents: _,
                    hash: _,
                    num_bytes: 0,
                },
                FileContents::Absent,
            ) => {}
            (
                FileContents::Absent,
                FileContents::Text {
                    contents,
                    hash: _,
                    num_bytes: _,
                },
            ) => sections.push(scm_record::Section::Changed {
                lines: make_section_changed_lines(&contents, scm_record::ChangeType::Added),
            }),

            (FileContents::Absent, FileContents::Binary { hash, num_bytes }) => {
                sections.push(scm_record::Section::Binary {
                    is_checked: false,
                    old_description: None,
                    new_description: Some(Cow::Owned(describe_binary(hash.as_deref(), num_bytes))),
                })
            }

            (
                FileContents::Text {
                    contents,
                    hash: _,
                    num_bytes: _,
                },
                FileContents::Absent,
            ) => sections.push(scm_record::Section::Changed {
                lines: make_section_changed_lines(&contents, scm_record::ChangeType::Removed),
            }),

            (
                FileContents::Text {
                    contents: old_contents,
                    hash: _,
                    num_bytes: _,
                },
                FileContents::Text {
                    contents: new_contents,
                    hash: _,
                    num_bytes: _,
                },
            ) => {
                sections.extend(make_diff_sections(&old_contents, &new_contents)?);
            }

            (
                FileContents::Text {
                    contents: _,
                    hash: old_hash,
                    num_bytes: old_num_bytes,
                }
                | FileContents::Binary {
                    hash: old_hash,
                    num_bytes: old_num_bytes,
                },
                FileContents::Text {
                    contents: _,
                    hash: new_hash,
                    num_bytes: new_num_bytes,
                }
                | FileContents::Binary {
                    hash: new_hash,
                    num_bytes: new_num_bytes,
                },
            ) => sections.push(scm_record::Section::Binary {
                is_checked: false,
                old_description: Some(Cow::Owned(describe_binary(
                    old_hash.as_deref(),
                    old_num_bytes,
                ))),
                new_description: Some(Cow::Owned(describe_binary(
                    new_hash.as_deref(),
                    new_num_bytes,
                ))),
            }),

            (FileContents::Binary { hash, num_bytes }, FileContents::Absent) => {
                sections.push(scm_record::Section::Binary {
                    is_checked: false,
                    old_description: Some(Cow::Owned(describe_binary(hash.as_deref(), num_bytes))),
                    new_description: None,
                })
            }
        }

        files.push(scm_record::File {
            old_path: None,
            path: Cow::Owned(changed_path.to_fs_path(Path::new(""))),
            file_mode: Some(left_info.file_mode),
            sections,
        });
    }
    Ok(files)
}

pub fn apply_diff_builtin(
    store: &Arc<Store>,
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    changed_files: Vec<RepoPathBuf>,
    files: &[scm_record::File],
) -> BackendResult<MergedTreeId> {
    let mut tree_builder = MergedTreeBuilder::new(left_tree.id().clone());
    assert_eq!(
        changed_files.len(),
        files.len(),
        "result had a different number of files"
    );
    for (path, file) in changed_files.into_iter().zip(files) {
        let (selected, _unselected) = file.get_selected_contents();
        match selected {
            scm_record::SelectedContents::Absent => {
                // TODO(https://github.com/arxanas/scm-record/issues/26): This
                // is probably an upstream bug in scm-record. If the file exists
                // but is empty, `get_selected_contents` should probably return
                // `Unchanged` or `Present`. When this is fixed upstream we can
                // simplify this logic.

                // Currently, `Absent` means the file is either empty or
                // deleted. We need to disambiguate three cases:
                // 1. The file is new and empty.
                // 2. The file existed before, is empty, and nothing changed.
                // 3. The file does not exist (it's been deleted).
                let old_mode = file.file_mode;
                let new_mode = file.get_file_mode();
                let file_existed_previously =
                    old_mode.is_some() && old_mode != Some(scm_record::FileMode::absent());
                let file_exists_now =
                    new_mode.is_some() && new_mode != Some(scm_record::FileMode::absent());
                let new_empty_file = !file_existed_previously && file_exists_now;
                let file_deleted = file_existed_previously && !file_exists_now;

                if new_empty_file {
                    let value = right_tree.path_value(&path)?;
                    tree_builder.set_or_remove(path, value);
                } else if file_deleted {
                    tree_builder.set_or_remove(path, Merge::absent());
                }
                // Else: the file is empty and nothing changed.
            }
            scm_record::SelectedContents::Unchanged => {
                // Do nothing.
            }
            scm_record::SelectedContents::Binary {
                old_description: _,
                new_description: _,
            } => {
                let value = right_tree.path_value(&path)?;
                tree_builder.set_or_remove(path, value);
            }
            scm_record::SelectedContents::Present { contents } => {
                let file_id = store.write_file(&path, &mut contents.as_bytes())?;
                tree_builder.set_or_remove(
                    path,
                    Merge::normal(TreeValue::File {
                        id: file_id,
                        executable: file.get_file_mode()
                            == Some(scm_record::FileMode(mode::EXECUTABLE)),
                    }),
                )
            }
        }
    }

    let tree_id = tree_builder.write_tree(left_tree.store())?;
    Ok(tree_id)
}

pub fn edit_diff_builtin(
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    matcher: &dyn Matcher,
) -> Result<MergedTreeId, BuiltinToolError> {
    let store = left_tree.store().clone();
    // TODO: handle copy tracking
    let changed_files: Vec<_> = left_tree
        .diff_stream(right_tree, matcher)
        .map(|TreeDiffEntry { path, values }| values.map(|_| path))
        .try_collect()
        .block_on()?;
    let files = make_diff_files(&store, left_tree, right_tree, &changed_files)?;
    let mut input = scm_record::helpers::CrosstermInput;
    let recorder = scm_record::Recorder::new(
        scm_record::RecordState {
            is_read_only: false,
            files,
            commits: Default::default(),
        },
        &mut input,
    );
    let result = recorder.run().map_err(BuiltinToolError::Record)?;
    let tree_id = apply_diff_builtin(&store, left_tree, right_tree, changed_files, &result.files)
        .map_err(BuiltinToolError::BackendError)?;
    Ok(tree_id)
}

fn make_merge_sections(
    merge_result: MergeResult,
) -> Result<Vec<scm_record::Section<'static>>, BuiltinToolError> {
    let mut sections = Vec::new();
    match merge_result {
        MergeResult::Resolved(ContentHunk(buf)) => {
            let contents = buf_to_file_contents(None, buf);
            let section = match contents {
                FileContents::Absent => None,
                FileContents::Text {
                    contents,
                    hash: _,
                    num_bytes: _,
                } => Some(scm_record::Section::Unchanged {
                    lines: contents
                        .split_inclusive('\n')
                        .map(|line| Cow::Owned(line.to_owned()))
                        .collect(),
                }),
                FileContents::Binary { hash, num_bytes } => Some(scm_record::Section::Binary {
                    is_checked: false,
                    old_description: None,
                    new_description: Some(Cow::Owned(describe_binary(hash.as_deref(), num_bytes))),
                }),
            };
            if let Some(section) = section {
                sections.push(section);
            }
        }
        MergeResult::Conflict(hunks) => {
            for hunk in hunks {
                let section = match hunk.into_resolved() {
                    Ok(ContentHunk(contents)) => {
                        let contents = std::str::from_utf8(&contents).map_err(|err| {
                            BuiltinToolError::DecodeUtf8 {
                                source: err,
                                item: "unchanged hunk",
                            }
                        })?;
                        scm_record::Section::Unchanged {
                            lines: contents
                                .split_inclusive('\n')
                                .map(|line| Cow::Owned(line.to_owned()))
                                .collect(),
                        }
                    }
                    Err(merge) => {
                        let lines: Vec<scm_record::SectionChangedLine> = merge
                            .iter()
                            .zip(
                                [
                                    scm_record::ChangeType::Added,
                                    scm_record::ChangeType::Removed,
                                ]
                                .into_iter()
                                .cycle(),
                            )
                            .map(|(contents, change_type)| -> Result<_, BuiltinToolError> {
                                let ContentHunk(contents) = contents;
                                let contents = std::str::from_utf8(contents).map_err(|err| {
                                    BuiltinToolError::DecodeUtf8 {
                                        source: err,
                                        item: "conflicting hunk",
                                    }
                                })?;
                                let changed_lines =
                                    make_section_changed_lines(contents, change_type);
                                Ok(changed_lines)
                            })
                            .flatten_ok()
                            .try_collect()?;
                        scm_record::Section::Changed { lines }
                    }
                };
                sections.push(section);
            }
        }
    }
    Ok(sections)
}

pub fn edit_merge_builtin(
    tree: &MergedTree,
    path: &RepoPath,
    content: Merge<ContentHunk>,
) -> Result<MergedTreeId, BuiltinToolError> {
    let merge_result = files::merge(&content);
    let sections = make_merge_sections(merge_result)?;
    let mut input = scm_record::helpers::CrosstermInput;
    let recorder = scm_record::Recorder::new(
        scm_record::RecordState {
            is_read_only: false,
            files: vec![scm_record::File {
                old_path: None,
                path: Cow::Owned(path.to_fs_path(Path::new(""))),
                file_mode: None,
                sections,
            }],
            commits: Default::default(),
        },
        &mut input,
    );
    let state = recorder.run()?;

    let file = state.files.into_iter().exactly_one().unwrap();
    apply_diff_builtin(tree.store(), tree, tree, vec![path.to_owned()], &[file])
        .map_err(BuiltinToolError::BackendError)
}

#[cfg(test)]
mod tests {
    use jj_lib::conflicts::extract_as_single_hunk;
    use jj_lib::merge::MergedTreeValue;
    use jj_lib::repo::Repo;
    use testutils::TestRepo;

    use super::*;

    #[test]
    fn test_edit_diff_builtin() {
        let test_repo = TestRepo::init();
        let store = test_repo.repo.store();

        let unused_path = RepoPath::from_internal_string("unused");
        let unchanged = RepoPath::from_internal_string("unchanged");
        let changed_path = RepoPath::from_internal_string("changed");
        let added_path = RepoPath::from_internal_string("added");
        let left_tree = testutils::create_tree(
            &test_repo.repo,
            &[
                (unused_path, "unused\n"),
                (unchanged, "unchanged\n"),
                (changed_path, "line1\nline2\nline3\n"),
            ],
        );
        let right_tree = testutils::create_tree(
            &test_repo.repo,
            &[
                (unused_path, "unused\n"),
                (unchanged, "unchanged\n"),
                (changed_path, "line1\nchanged1\nchanged2\nline3\nadded1\n"),
                (added_path, "added\n"),
            ],
        );

        let changed_files = vec![
            unchanged.to_owned(),
            changed_path.to_owned(),
            added_path.to_owned(),
        ];
        let files = make_diff_files(store, &left_tree, &right_tree, &changed_files).unwrap();
        insta::assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: None,
                path: "unchanged",
                file_mode: Some(
                    FileMode(
                        33188,
                    ),
                ),
                sections: [
                    Unchanged {
                        lines: [
                            "unchanged\n",
                        ],
                    },
                ],
            },
            File {
                old_path: None,
                path: "changed",
                file_mode: Some(
                    FileMode(
                        33188,
                    ),
                ),
                sections: [
                    Unchanged {
                        lines: [
                            "line1\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "line2\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "changed1\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "changed2\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "line3\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "added1\n",
                            },
                        ],
                    },
                ],
            },
            File {
                old_path: None,
                path: "added",
                file_mode: Some(
                    FileMode(
                        0,
                    ),
                ),
                sections: [
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "added\n",
                            },
                        ],
                    },
                ],
            },
        ]
        "###);

        let no_changes_tree_id = apply_diff_builtin(
            store,
            &left_tree,
            &right_tree,
            changed_files.clone(),
            &files,
        )
        .unwrap();
        let no_changes_tree = store.get_root_tree(&no_changes_tree_id).unwrap();
        assert_eq!(
            no_changes_tree.id(),
            left_tree.id(),
            "no-changes tree was different",
        );

        let mut files = files;
        for file in files.iter_mut() {
            file.toggle_all();
        }
        let all_changes_tree_id =
            apply_diff_builtin(store, &left_tree, &right_tree, changed_files, &files).unwrap();
        let all_changes_tree = store.get_root_tree(&all_changes_tree_id).unwrap();
        assert_eq!(
            all_changes_tree.id(),
            right_tree.id(),
            "all-changes tree was different",
        );
    }

    #[test]
    fn test_edit_diff_builtin_add_empty_file() {
        let test_repo = TestRepo::init();
        let store = test_repo.repo.store();

        let added_empty_file_path = RepoPath::from_internal_string("empty_file");
        let left_tree = testutils::create_tree(&test_repo.repo, &[]);
        let right_tree = testutils::create_tree(&test_repo.repo, &[(added_empty_file_path, "")]);

        let changed_files = vec![added_empty_file_path.to_owned()];
        let files = make_diff_files(store, &left_tree, &right_tree, &changed_files).unwrap();
        insta::assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: None,
                path: "empty_file",
                file_mode: Some(
                    FileMode(
                        0,
                    ),
                ),
                sections: [
                    FileMode {
                        is_checked: false,
                        before: FileMode(
                            0,
                        ),
                        after: FileMode(
                            33188,
                        ),
                    },
                ],
            },
        ]
        "###);
        let no_changes_tree_id = apply_diff_builtin(
            store,
            &left_tree,
            &right_tree,
            changed_files.clone(),
            &files,
        )
        .unwrap();
        let no_changes_tree = store.get_root_tree(&no_changes_tree_id).unwrap();
        assert_eq!(
            no_changes_tree.id(),
            left_tree.id(),
            "no-changes tree was different",
        );

        let mut files = files;
        for file in files.iter_mut() {
            file.toggle_all();
        }
        let all_changes_tree_id =
            apply_diff_builtin(store, &left_tree, &right_tree, changed_files, &files).unwrap();
        let all_changes_tree = store.get_root_tree(&all_changes_tree_id).unwrap();
        assert_eq!(
            all_changes_tree.id(),
            right_tree.id(),
            "all-changes tree was different",
        );
    }

    #[test]
    fn test_edit_diff_builtin_delete_empty_file() {
        let test_repo = TestRepo::init();
        let store = test_repo.repo.store();

        let added_empty_file_path = RepoPath::from_internal_string("empty_file");
        let left_tree = testutils::create_tree(&test_repo.repo, &[(added_empty_file_path, "")]);
        let right_tree = testutils::create_tree(&test_repo.repo, &[]);

        let changed_files = vec![added_empty_file_path.to_owned()];
        let files = make_diff_files(store, &left_tree, &right_tree, &changed_files).unwrap();
        insta::assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: None,
                path: "empty_file",
                file_mode: Some(
                    FileMode(
                        33188,
                    ),
                ),
                sections: [
                    FileMode {
                        is_checked: false,
                        before: FileMode(
                            33188,
                        ),
                        after: FileMode(
                            0,
                        ),
                    },
                ],
            },
        ]
        "###);
        let no_changes_tree_id = apply_diff_builtin(
            store,
            &left_tree,
            &right_tree,
            changed_files.clone(),
            &files,
        )
        .unwrap();
        let no_changes_tree = store.get_root_tree(&no_changes_tree_id).unwrap();
        assert_eq!(
            no_changes_tree.id(),
            left_tree.id(),
            "no-changes tree was different",
        );

        let mut files = files;
        for file in files.iter_mut() {
            file.toggle_all();
        }
        let all_changes_tree_id =
            apply_diff_builtin(store, &left_tree, &right_tree, changed_files, &files).unwrap();
        let all_changes_tree = store.get_root_tree(&all_changes_tree_id).unwrap();
        assert_eq!(
            all_changes_tree.id(),
            right_tree.id(),
            "all-changes tree was different",
        );
    }

    #[test]
    fn test_edit_diff_builtin_modify_empty_file() {
        let test_repo = TestRepo::init();
        let store = test_repo.repo.store();

        let empty_file_path = RepoPath::from_internal_string("empty_file");
        let left_tree = testutils::create_tree(&test_repo.repo, &[(empty_file_path, "")]);
        let right_tree =
            testutils::create_tree(&test_repo.repo, &[(empty_file_path, "modified\n")]);

        let changed_files = vec![empty_file_path.to_owned()];
        let files = make_diff_files(store, &left_tree, &right_tree, &changed_files).unwrap();
        insta::assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: None,
                path: "empty_file",
                file_mode: Some(
                    FileMode(
                        33188,
                    ),
                ),
                sections: [
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "modified\n",
                            },
                        ],
                    },
                ],
            },
        ]
        "###);
        let no_changes_tree_id = apply_diff_builtin(
            store,
            &left_tree,
            &right_tree,
            changed_files.clone(),
            &files,
        )
        .unwrap();
        let no_changes_tree = store.get_root_tree(&no_changes_tree_id).unwrap();
        assert_eq!(
            no_changes_tree.id(),
            left_tree.id(),
            "no-changes tree was different",
        );

        let mut files = files;
        for file in files.iter_mut() {
            file.toggle_all();
        }
        let all_changes_tree_id =
            apply_diff_builtin(store, &left_tree, &right_tree, changed_files, &files).unwrap();
        let all_changes_tree = store.get_root_tree(&all_changes_tree_id).unwrap();
        assert_eq!(
            all_changes_tree.id(),
            right_tree.id(),
            "all-changes tree was different",
        );
    }

    #[test]
    fn test_make_merge_sections() {
        let test_repo = TestRepo::init();
        let store = test_repo.repo.store();

        let path = RepoPath::from_internal_string("file");
        let base_tree = testutils::create_tree(
            &test_repo.repo,
            &[(path, "base 1\nbase 2\nbase 3\nbase 4\nbase 5\n")],
        );
        let left_tree = testutils::create_tree(
            &test_repo.repo,
            &[(path, "left 1\nbase 2\nbase 3\nbase 4\nleft 5\n")],
        );
        let right_tree = testutils::create_tree(
            &test_repo.repo,
            &[(path, "right 1\nbase 2\nbase 3\nbase 4\nright 5\n")],
        );

        fn to_file_id(tree_value: MergedTreeValue) -> Option<FileId> {
            match tree_value.into_resolved() {
                Ok(Some(TreeValue::File { id, executable: _ })) => Some(id.clone()),
                other => {
                    panic!("merge should have been a FileId: {other:?}")
                }
            }
        }
        let merge = Merge::from_vec(vec![
            to_file_id(left_tree.path_value(path).unwrap()),
            to_file_id(base_tree.path_value(path).unwrap()),
            to_file_id(right_tree.path_value(path).unwrap()),
        ]);
        let content = extract_as_single_hunk(&merge, store, path)
            .block_on()
            .unwrap();
        let merge_result = files::merge(&content);
        let sections = make_merge_sections(merge_result).unwrap();
        insta::assert_debug_snapshot!(sections, @r###"
        [
            Changed {
                lines: [
                    SectionChangedLine {
                        is_checked: false,
                        change_type: Added,
                        line: "left 1\n",
                    },
                    SectionChangedLine {
                        is_checked: false,
                        change_type: Removed,
                        line: "base 1\n",
                    },
                    SectionChangedLine {
                        is_checked: false,
                        change_type: Added,
                        line: "right 1\n",
                    },
                ],
            },
            Unchanged {
                lines: [
                    "base 2\n",
                    "base 3\n",
                    "base 4\n",
                ],
            },
            Changed {
                lines: [
                    SectionChangedLine {
                        is_checked: false,
                        change_type: Added,
                        line: "left 5\n",
                    },
                    SectionChangedLine {
                        is_checked: false,
                        change_type: Removed,
                        line: "base 5\n",
                    },
                    SectionChangedLine {
                        is_checked: false,
                        change_type: Added,
                        line: "right 5\n",
                    },
                ],
            },
        ]
        "###);
    }
}
