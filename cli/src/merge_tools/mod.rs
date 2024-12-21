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

mod builtin;
mod diff_working_copies;
mod external;

use std::sync::Arc;

use bstr::BString;
use jj_lib::backend::FileId;
use jj_lib::backend::MergedTreeId;
use jj_lib::config::ConfigGetError;
use jj_lib::config::ConfigGetResultExt as _;
use jj_lib::config::ConfigNamePathBuf;
use jj_lib::conflicts::extract_as_single_hunk;
use jj_lib::conflicts::ConflictMarkerStyle;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::Matcher;
use jj_lib::merge::Merge;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo_path::InvalidRepoPathError;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::settings::UserSettings;
use jj_lib::working_copy::SnapshotError;
use pollster::FutureExt;
use thiserror::Error;

use self::builtin::edit_diff_builtin;
use self::builtin::edit_merge_builtin;
use self::builtin::BuiltinToolError;
pub(crate) use self::diff_working_copies::new_utf8_temp_dir;
use self::diff_working_copies::DiffCheckoutError;
use self::external::edit_diff_external;
pub use self::external::generate_diff;
pub use self::external::invoke_external_diff;
pub use self::external::DiffToolMode;
pub use self::external::ExternalMergeTool;
use self::external::ExternalToolError;
use crate::config::CommandNameAndArgs;
use crate::ui::Ui;

const BUILTIN_EDITOR_NAME: &str = ":builtin";

#[derive(Debug, Error)]
pub enum DiffEditError {
    #[error(transparent)]
    InternalTool(#[from] Box<BuiltinToolError>),
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error(transparent)]
    DiffCheckoutError(#[from] DiffCheckoutError),
    #[error("Failed to snapshot changes")]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Config(#[from] ConfigGetError),
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
    InternalTool(#[from] Box<BuiltinToolError>),
    #[error(transparent)]
    ExternalTool(#[from] ExternalToolError),
    #[error(transparent)]
    InvalidRepoPath(#[from] InvalidRepoPathError),
    #[error("Couldn't find the path {0:?} in this revision")]
    PathNotFound(RepoPathBuf),
    #[error("Couldn't find any conflicts at {0:?} in this revision")]
    NotAConflict(RepoPathBuf),
    #[error(
        "Only conflicts that involve normal files (not symlinks, not executable, etc.) are \
         supported. Conflict summary for {0:?}:\n{1}"
    )]
    NotNormalFiles(RepoPathBuf, String),
    #[error("The conflict at {path:?} has {sides} sides. At most 2 sides are supported.")]
    ConflictTooComplicated { path: RepoPathBuf, sides: usize },
    #[error(
        "The output file is either unchanged or empty after the editor quit (run with --debug to \
         see the exact invocation)."
    )]
    EmptyOrUnchanged,
    #[error("Backend error")]
    Backend(#[from] jj_lib::backend::BackendError),
}

#[derive(Debug, Error)]
pub enum MergeToolConfigError {
    #[error(transparent)]
    Config(#[from] ConfigGetError),
    #[error("The tool `{tool_name}` cannot be used as a merge tool with `jj resolve`")]
    MergeArgsNotConfigured { tool_name: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeTool {
    Builtin,
    // Boxed because ExternalMergeTool is big compared to the Builtin variant.
    External(Box<ExternalMergeTool>),
}

impl MergeTool {
    fn external(tool: ExternalMergeTool) -> Self {
        MergeTool::External(Box::new(tool))
    }
}

/// Finds the appropriate tool for diff editing or merges
fn editor_args_from_settings(
    ui: &Ui,
    settings: &UserSettings,
    key: &'static str,
) -> Result<CommandNameAndArgs, ConfigGetError> {
    // TODO: Make this configuration have a table of possible editors and detect the
    // best one here.
    if let Some(args) = settings.get(key).optional()? {
        Ok(args)
    } else {
        let default_editor = BUILTIN_EDITOR_NAME;
        writeln!(
            ui.hint_default(),
            "Using default editor '{default_editor}'; run `jj config set --user {key} :builtin` \
             to disable this message."
        )
        .ok();
        Ok(default_editor.into())
    }
}

/// Resolves builtin merge tool name or loads external tool options from
/// `[merge-tools.<name>]`.
fn get_tool_config(
    settings: &UserSettings,
    name: &str,
) -> Result<Option<MergeTool>, ConfigGetError> {
    if name == BUILTIN_EDITOR_NAME {
        Ok(Some(MergeTool::Builtin))
    } else {
        Ok(get_external_tool_config(settings, name)?.map(MergeTool::external))
    }
}

/// Loads external diff/merge tool options from `[merge-tools.<name>]`.
pub fn get_external_tool_config(
    settings: &UserSettings,
    name: &str,
) -> Result<Option<ExternalMergeTool>, ConfigGetError> {
    let full_name = ConfigNamePathBuf::from_iter(["merge-tools", name]);
    let Some(mut tool) = settings.get::<ExternalMergeTool>(&full_name).optional()? else {
        return Ok(None);
    };
    if tool.program.is_empty() {
        tool.program = name.to_owned();
    };
    Ok(Some(tool))
}

/// Configured diff editor.
#[derive(Clone, Debug)]
pub struct DiffEditor {
    tool: MergeTool,
    base_ignores: Arc<GitIgnoreFile>,
    use_instructions: bool,
    conflict_marker_style: ConflictMarkerStyle,
}

impl DiffEditor {
    /// Creates diff editor of the given name, and loads parameters from the
    /// settings.
    pub fn with_name(
        name: &str,
        settings: &UserSettings,
        base_ignores: Arc<GitIgnoreFile>,
        conflict_marker_style: ConflictMarkerStyle,
    ) -> Result<Self, MergeToolConfigError> {
        let tool = get_tool_config(settings, name)?
            .unwrap_or_else(|| MergeTool::external(ExternalMergeTool::with_program(name)));
        Self::new_inner(tool, settings, base_ignores, conflict_marker_style)
    }

    /// Loads the default diff editor from the settings.
    pub fn from_settings(
        ui: &Ui,
        settings: &UserSettings,
        base_ignores: Arc<GitIgnoreFile>,
        conflict_marker_style: ConflictMarkerStyle,
    ) -> Result<Self, MergeToolConfigError> {
        let args = editor_args_from_settings(ui, settings, "ui.diff-editor")?;
        let tool = if let CommandNameAndArgs::String(name) = &args {
            get_tool_config(settings, name)?
        } else {
            None
        }
        .unwrap_or_else(|| MergeTool::external(ExternalMergeTool::with_edit_args(&args)));
        Self::new_inner(tool, settings, base_ignores, conflict_marker_style)
    }

    fn new_inner(
        tool: MergeTool,
        settings: &UserSettings,
        base_ignores: Arc<GitIgnoreFile>,
        conflict_marker_style: ConflictMarkerStyle,
    ) -> Result<Self, MergeToolConfigError> {
        Ok(DiffEditor {
            tool,
            base_ignores,
            use_instructions: settings.get_bool("ui.diff-instructions")?,
            conflict_marker_style,
        })
    }

    /// Starts a diff editor on the two directories.
    pub fn edit(
        &self,
        left_tree: &MergedTree,
        right_tree: &MergedTree,
        matcher: &dyn Matcher,
        format_instructions: impl FnOnce() -> String,
    ) -> Result<MergedTreeId, DiffEditError> {
        match &self.tool {
            MergeTool::Builtin => {
                Ok(
                    edit_diff_builtin(left_tree, right_tree, matcher, self.conflict_marker_style)
                        .map_err(Box::new)?,
                )
            }
            MergeTool::External(editor) => {
                let instructions = self.use_instructions.then(format_instructions);
                edit_diff_external(
                    editor,
                    left_tree,
                    right_tree,
                    matcher,
                    instructions.as_deref(),
                    self.base_ignores.clone(),
                    self.conflict_marker_style,
                )
            }
        }
    }
}

/// A file to be merged by a merge tool.
struct MergeToolFile {
    repo_path: RepoPathBuf,
    conflict: MergedTreeValue,
    file_merge: Merge<Option<FileId>>,
    content: Merge<BString>,
}

/// Configured 3-way merge editor.
#[derive(Clone, Debug)]
pub struct MergeEditor {
    tool: MergeTool,
    conflict_marker_style: ConflictMarkerStyle,
}

impl MergeEditor {
    /// Creates 3-way merge editor of the given name, and loads parameters from
    /// the settings.
    pub fn with_name(
        name: &str,
        settings: &UserSettings,
        conflict_marker_style: ConflictMarkerStyle,
    ) -> Result<Self, MergeToolConfigError> {
        let tool = get_tool_config(settings, name)?
            .unwrap_or_else(|| MergeTool::external(ExternalMergeTool::with_program(name)));
        Self::new_inner(name, tool, conflict_marker_style)
    }

    /// Loads the default 3-way merge editor from the settings.
    pub fn from_settings(
        ui: &Ui,
        settings: &UserSettings,
        conflict_marker_style: ConflictMarkerStyle,
    ) -> Result<Self, MergeToolConfigError> {
        let args = editor_args_from_settings(ui, settings, "ui.merge-editor")?;
        let tool = if let CommandNameAndArgs::String(name) = &args {
            get_tool_config(settings, name)?
        } else {
            None
        }
        .unwrap_or_else(|| MergeTool::external(ExternalMergeTool::with_merge_args(&args)));
        Self::new_inner(&args, tool, conflict_marker_style)
    }

    fn new_inner(
        name: impl ToString,
        tool: MergeTool,
        conflict_marker_style: ConflictMarkerStyle,
    ) -> Result<Self, MergeToolConfigError> {
        if matches!(&tool, MergeTool::External(mergetool) if mergetool.merge_args.is_empty()) {
            return Err(MergeToolConfigError::MergeArgsNotConfigured {
                tool_name: name.to_string(),
            });
        }
        Ok(MergeEditor {
            tool,
            conflict_marker_style,
        })
    }

    /// Starts a merge editor for the specified file.
    pub fn edit_file(
        &self,
        tree: &MergedTree,
        repo_path: &RepoPath,
    ) -> Result<MergedTreeId, ConflictResolveError> {
        let conflict = match tree.path_value(repo_path)?.into_resolved() {
            Err(conflict) => conflict,
            Ok(Some(_)) => return Err(ConflictResolveError::NotAConflict(repo_path.to_owned())),
            Ok(None) => return Err(ConflictResolveError::PathNotFound(repo_path.to_owned())),
        };
        let file_merge = conflict.to_file_merge().ok_or_else(|| {
            let summary = conflict.describe();
            ConflictResolveError::NotNormalFiles(repo_path.to_owned(), summary)
        })?;
        let simplified_file_merge = file_merge.clone().simplify();
        // We only support conflicts with 2 sides (3-way conflicts)
        if simplified_file_merge.num_sides() > 2 {
            return Err(ConflictResolveError::ConflictTooComplicated {
                path: repo_path.to_owned(),
                sides: simplified_file_merge.num_sides(),
            });
        };
        let content =
            extract_as_single_hunk(&simplified_file_merge, tree.store(), repo_path).block_on()?;
        let merge_tool_file = MergeToolFile {
            repo_path: repo_path.to_owned(),
            conflict,
            file_merge,
            content,
        };

        match &self.tool {
            MergeTool::Builtin => {
                let tree_id = edit_merge_builtin(tree, &merge_tool_file).map_err(Box::new)?;
                Ok(tree_id)
            }
            MergeTool::External(editor) => external::run_mergetool_external(
                editor,
                tree,
                &merge_tool_file,
                self.conflict_marker_style,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use jj_lib::config::ConfigLayer;
    use jj_lib::config::ConfigSource;
    use jj_lib::config::StackedConfig;

    use super::*;

    fn config_from_string(text: &str) -> StackedConfig {
        let mut config = StackedConfig::empty();
        // Load defaults to test the default args lookup
        config.extend_layers(crate::config::default_config_layers());
        config.add_layer(ConfigLayer::parse(ConfigSource::User, text).unwrap());
        config
    }

    #[test]
    fn test_get_diff_editor_with_name() {
        let get = |name, config_text| {
            let config = config_from_string(config_text);
            let settings = UserSettings::from_config(config).unwrap();
            DiffEditor::with_name(
                name,
                &settings,
                GitIgnoreFile::empty(),
                ConflictMarkerStyle::Diff,
            )
            .map(|editor| editor.tool)
        };

        insta::assert_debug_snapshot!(get(":builtin", "").unwrap(), @"Builtin");

        // Just program name, edit_args are filled by default
        insta::assert_debug_snapshot!(get("my diff", "").unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my diff",
                diff_args: [
                    "$left",
                    "$right",
                ],
                diff_invocation_mode: Dir,
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
            },
        )
        "###);

        // Pick from merge-tools
        insta::assert_debug_snapshot!(get(
            "foo bar", r#"
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
                diff_invocation_mode: Dir,
                edit_args: [
                    "--edit",
                    "args",
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
            },
        )
        "###);
    }

    #[test]
    fn test_get_diff_editor_from_settings() {
        let get = |text| {
            let config = config_from_string(text);
            let ui = Ui::with_config(&config).unwrap();
            let settings = UserSettings::from_config(config).unwrap();
            DiffEditor::from_settings(
                &ui,
                &settings,
                GitIgnoreFile::empty(),
                ConflictMarkerStyle::Diff,
            )
            .map(|editor| editor.tool)
        };

        // Default
        insta::assert_debug_snapshot!(get("").unwrap(), @"Builtin");

        // Just program name, edit_args are filled by default
        insta::assert_debug_snapshot!(get(r#"ui.diff-editor = "my-diff""#).unwrap(), @r###"
        External(
            ExternalMergeTool {
                program: "my-diff",
                diff_args: [
                    "$left",
                    "$right",
                ],
                diff_invocation_mode: Dir,
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
                edit_args: [
                    "-l",
                    "$left",
                    "-r",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
                edit_args: [
                    "--diff",
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
                edit_args: [
                    "--edit",
                    "args",
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
                edit_args: [
                    "$left",
                    "$right",
                ],
                merge_args: [],
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
            },
        )
        "###);

        // Invalid type
        assert!(get(r#"ui.diff-editor.k = 0"#).is_err());
    }

    #[test]
    fn test_get_merge_editor_with_name() {
        let get = |name, config_text| {
            let config = config_from_string(config_text);
            let settings = UserSettings::from_config(config).unwrap();
            MergeEditor::with_name(name, &settings, ConflictMarkerStyle::Diff)
                .map(|editor| editor.tool)
        };

        insta::assert_debug_snapshot!(get(":builtin", "").unwrap(), @"Builtin");

        // Just program name
        insta::assert_debug_snapshot!(get("my diff", "").unwrap_err(), @r###"
        MergeArgsNotConfigured {
            tool_name: "my diff",
        }
        "###);

        // Pick from merge-tools
        insta::assert_debug_snapshot!(get(
            "foo bar", r#"
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
                diff_invocation_mode: Dir,
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
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
            },
        )
        "###);
    }

    #[test]
    fn test_get_merge_editor_from_settings() {
        let get = |text| {
            let config = config_from_string(text);
            let ui = Ui::with_config(&config).unwrap();
            let settings = UserSettings::from_config(config).unwrap();
            MergeEditor::from_settings(&ui, &settings, ConflictMarkerStyle::Diff)
                .map(|editor| editor.tool)
        };

        // Default
        insta::assert_debug_snapshot!(get("").unwrap(), @"Builtin");

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
                diff_invocation_mode: Dir,
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
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
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
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
                diff_invocation_mode: Dir,
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
                merge_conflict_exit_codes: [],
                merge_tool_edits_conflict_markers: false,
                conflict_marker_style: None,
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
