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

mod external;
mod internal;

use std::sync::Arc;

use config::ConfigError;
use jj_lib::backend::{TreeId, TreeValue};
use jj_lib::conflicts::extract_as_single_hunk;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::Matcher;
use jj_lib::repo_path::RepoPath;
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::tree::Tree;
use jj_lib::working_copy::SnapshotError;
use thiserror::Error;

use self::external::{edit_diff_external, DiffCheckoutError, ExternalToolError};
pub use self::external::{generate_diff, ExternalMergeTool};
use self::internal::{edit_diff_internal, edit_merge_internal, InternalToolError};
use crate::config::CommandNameAndArgs;
use crate::ui::Ui;

const BUILTIN_EDITOR_NAME: &str = ":builtin";

#[derive(Debug, Error)]
pub enum DiffEditError {
    #[error(transparent)]
    InternalTool(#[from] Box<InternalToolError>),
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error(transparent)]
    DiffCheckoutError(#[from] DiffCheckoutError),
    #[error("Failed to snapshot changes: {0}")]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Config(#[from] config::ConfigError),
}

#[derive(Debug, Error)]
pub enum DiffGenerateError {
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error(transparent)]
    DiffCheckoutError(#[from] DiffCheckoutError),
}

#[derive(Debug, Error)]
pub enum ConflictResolveError {
    #[error(transparent)]
    InternalTool(#[from] Box<InternalToolError>),
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error("Couldn't find the path {0:?} in this revision")]
    PathNotFound(RepoPath),
    #[error("Couldn't find any conflicts at {0:?} in this revision")]
    NotAConflict(RepoPath),
    #[error(
        "Only conflicts that involve normal files (not symlinks, not executable, etc.) are \
         supported. Conflict summary for {0:?}:\n{1}"
    )]
    NotNormalFiles(RepoPath, String),
    #[error("The conflict at {path:?} has {sides} sides. At most 2 sides are supported.")]
    ConflictTooComplicated { path: RepoPath, sides: usize },
    #[error(
        "The output file is either unchanged or empty after the editor quit (run with --verbose \
         to see the exact invocation)."
    )]
    EmptyOrUnchanged,
    #[error("Backend error: {0}")]
    Backend(#[from] jj_lib::backend::BackendError),
}

pub fn run_mergetool(
    ui: &Ui,
    tree: &Tree,
    repo_path: &RepoPath,
    settings: &UserSettings,
) -> Result<TreeId, ConflictResolveError> {
    let conflict_id = match tree.path_value(repo_path) {
        Some(TreeValue::Conflict(id)) => id,
        Some(_) => return Err(ConflictResolveError::NotAConflict(repo_path.clone())),
        None => return Err(ConflictResolveError::PathNotFound(repo_path.clone())),
    };
    let conflict = tree.store().read_conflict(repo_path, &conflict_id)?;
    let file_merge = conflict.to_file_merge().ok_or_else(|| {
        let mut summary_bytes: Vec<u8> = vec![];
        conflict
            .describe(&mut summary_bytes)
            .expect("Writing to an in-memory buffer should never fail");
        ConflictResolveError::NotNormalFiles(
            repo_path.clone(),
            String::from_utf8_lossy(summary_bytes.as_slice()).to_string(),
        )
    })?;
    // We only support conflicts with 2 sides (3-way conflicts)
    if file_merge.adds().len() > 2 {
        return Err(ConflictResolveError::ConflictTooComplicated {
            path: repo_path.clone(),
            sides: file_merge.adds().len(),
        });
    };
    let content = extract_as_single_hunk(&file_merge, tree.store(), repo_path);

    let editor = get_merge_tool_from_settings(ui, settings)?;
    match editor {
        MergeTool::Internal => {
            let tree_id =
                edit_merge_internal(tree, repo_path, file_merge, content).map_err(Box::new)?;
            Ok(tree_id)
        }
        MergeTool::External(editor) => external::run_mergetool_external(
            &editor, file_merge, content, repo_path, conflict, tree,
        ),
    }
}

pub fn edit_diff(
    ui: &Ui,
    left_tree: &Tree,
    right_tree: &Tree,
    matcher: &dyn Matcher,
    instructions: &str,
    base_ignores: Arc<GitIgnoreFile>,
    settings: &UserSettings,
) -> Result<TreeId, DiffEditError> {
    // Start a diff editor on the two directories.
    let editor = get_diff_editor_from_settings(ui, settings)?;
    match editor {
        MergeTool::Internal => {
            let tree_id = edit_diff_internal(left_tree, right_tree, matcher).map_err(Box::new)?;
            Ok(tree_id)
        }
        MergeTool::External(editor) => edit_diff_external(
            editor,
            left_tree,
            right_tree,
            matcher,
            instructions,
            base_ignores,
            settings,
        ),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeTool {
    Internal,
    External(ExternalMergeTool),
}

/// Loads merge tool options from `[merge-tools.<name>]`.
pub fn get_tool_config(
    settings: &UserSettings,
    name: &str,
) -> Result<Option<MergeTool>, ConfigError> {
    if name == BUILTIN_EDITOR_NAME {
        return Ok(Some(MergeTool::Internal));
    }

    const TABLE_KEY: &str = "merge-tools";
    let tools_table = settings.config().get_table(TABLE_KEY)?;
    if let Some(v) = tools_table.get(name) {
        let mut result: ExternalMergeTool = v
            .clone()
            .try_deserialize()
            // add config key, deserialize error is otherwise unclear
            .map_err(|e| ConfigError::Message(format!("{TABLE_KEY}.{name}: {e}")))?;

        if result.program.is_empty() {
            result.program.clone_from(&name.to_string());
        };
        Ok(Some(MergeTool::External(result)))
    } else {
        Ok(None)
    }
}

/// Finds the appropriate tool for diff editing or merges
fn editor_args_from_settings(
    ui: &Ui,
    settings: &UserSettings,
    key: &str,
) -> Result<CommandNameAndArgs, ExternalToolError> {
    // TODO: Make this configuration have a table of possible editors and detect the
    // best one here.
    if let Some(args) = settings.config().get(key).optional()? {
        Ok(args)
    } else {
        let default_editor = BUILTIN_EDITOR_NAME;
        writeln!(
            ui.hint(),
            "Using default editor '{default_editor}'; you can change this by setting {key}"
        )
        .map_err(ExternalToolError::Io)?;
        Ok(default_editor.into())
    }
}

/// Loads merge tool options from `[merge-tools.<name>]` if `args` is of
/// unstructured string type.
pub fn get_tool_config_from_args(
    settings: &UserSettings,
    args: &CommandNameAndArgs,
) -> Result<Option<MergeTool>, ConfigError> {
    match args {
        CommandNameAndArgs::String(name) => get_tool_config(settings, name),
        CommandNameAndArgs::Vec(_) | CommandNameAndArgs::Structured { .. } => Ok(None),
    }
}

fn get_diff_editor_from_settings(
    ui: &Ui,
    settings: &UserSettings,
) -> Result<MergeTool, ExternalToolError> {
    let args = editor_args_from_settings(ui, settings, "ui.diff-editor")?;
    let editor = get_tool_config_from_args(settings, &args)?
        .unwrap_or_else(|| MergeTool::External(ExternalMergeTool::with_edit_args(&args)));
    Ok(editor)
}

fn get_merge_tool_from_settings(
    ui: &Ui,
    settings: &UserSettings,
) -> Result<MergeTool, ExternalToolError> {
    let args = editor_args_from_settings(ui, settings, "ui.merge-editor")?;
    let mergetool = get_tool_config_from_args(settings, &args)?
        .unwrap_or_else(|| MergeTool::External(ExternalMergeTool::with_merge_args(&args)));
    match mergetool {
        MergeTool::External(mergetool) if mergetool.merge_args.is_empty() => {
            Err(ExternalToolError::MergeArgsNotConfigured {
                tool_name: args.to_string(),
            })
        }
        mergetool => Ok(mergetool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from_string(text: &str) -> config::Config {
        config::Config::builder()
            // Load defaults to test the default args lookup
            .add_source(crate::config::default_config())
            .add_source(config::File::from_str(text, config::FileFormat::Toml))
            .build()
            .unwrap()
    }

    #[test]
    fn test_get_diff_editor() {
        let get = |text| {
            let config = config_from_string(text);
            let ui = Ui::with_config(&config).unwrap();
            let settings = UserSettings::from_config(config);
            get_diff_editor_from_settings(&ui, &settings)
        };

        // Default
        insta::assert_debug_snapshot!(get("").unwrap(), @"Internal");

        // Just program name, edit_args are filled by default
        insta::assert_debug_snapshot!(get(r#"ui.diff-editor = "my-diff""#).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my-diff",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // String args (with interpolation variables)
        insta::assert_debug_snapshot!(
            get(r#"ui.diff-editor = "my-diff -l $left -r $right""#).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my-diff",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "-l",
                    "$left",
                    "-r",
                    "$right",
                ],
                merge_args: [],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // List args (with interpolation variables)
        insta::assert_debug_snapshot!(
            get(r#"ui.diff-editor = ["my-diff", "--diff", "$left", "$right"]"#).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my-diff",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "--diff",
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // Pick from merge-tools
        insta::assert_debug_snapshot!(get(
        r#"
        ui.diff-editor = "foo bar"
        [merge-tools."foo bar"]
        edit-args = ["--edit", "args", "$left", "$right"]
        "#,
        ).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "foo bar",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "--edit",
                    "args",
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // Pick from merge-tools, but no edit-args specified
        insta::assert_debug_snapshot!(get(
        r#"
        ui.diff-editor = "my-diff"
        [merge-tools.my-diff]
        program = "MyDiff"
        "#,
        ).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "MyDiff",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // List args should never be a merge-tools key, edit_args are filled by default
        insta::assert_debug_snapshot!(get(r#"ui.diff-editor = ["meld"]"#).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "meld",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // Invalid type
        assert!(get(r#"ui.diff-editor.k = 0"#).is_err());
    }

    #[test]
    fn test_get_merge_tool() {
        let get = |text| {
            let config = config_from_string(text);
            let ui = Ui::with_config(&config).unwrap();
            let settings = UserSettings::from_config(config);
            get_merge_tool_from_settings(&ui, &settings)
        };

        // Default
        insta::assert_debug_snapshot!(get("").unwrap(), @"Internal");

        // Just program name
        insta::assert_debug_snapshot!(get(r#"ui.merge-editor = "my-merge""#).unwrap_err(), @r###"
        MergeArgsNotConfigured {
            tool_name: "my-merge",
        }
        "###);

        // String args
        insta::assert_debug_snapshot!(
            get(r#"ui.merge-editor = "my-merge $left $base $right $output""#).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my-merge",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [
                    "$left",
                    "$base",
                    "$right",
                    "$output",
                ],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // List args
        insta::assert_debug_snapshot!(
            get(
                r#"ui.merge-editor = ["my-merge", "$left", "$base", "$right", "$output"]"#,
            ).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my-merge",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [
                    "$left",
                    "$base",
                    "$right",
                    "$output",
                ],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // Pick from merge-tools
        insta::assert_debug_snapshot!(get(
        r#"
        ui.merge-editor = "foo bar"
        [merge-tools."foo bar"]
        merge-args = ["$base", "$left", "$right", "$output"]
        "#,
        ).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "foo bar",
                diff_args: [
                    "$left",
                    "$right",
                ],
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [
                    "$base",
                    "$left",
                    "$right",
                    "$output",
                ],
                merge_tool_edits_conflict_markers: false,
            },
        )
        "###);

        // List args should never be a merge-tools key
        insta::assert_debug_snapshot!(
            get(r#"ui.merge-editor = ["meld"]"#).unwrap_err(), @r###"
        MergeArgsNotConfigured {
            tool_name: "meld",
        }
        "###);

        // Invalid type
        assert!(get(r#"ui.merge-editor.k = 0"#).is_err());
    }
}
