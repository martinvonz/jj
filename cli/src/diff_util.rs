// Copyright 2020-2022 The Jujutsu Authors
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

use std::cmp::max;
use std::collections::VecDeque;
use std::io;
use std::ops::Range;

use futures::{try_join, Stream, StreamExt};
use itertools::Itertools;
use jj_lib::backend::{BackendResult, TreeValue};
use jj_lib::commit::Commit;
use jj_lib::conflicts::{materialize_tree_value, MaterializedTreeValue};
use jj_lib::diff::{Diff, DiffHunk};
use jj_lib::files::DiffLine;
use jj_lib::matchers::Matcher;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::{MergedTree, TreeDiffStream};
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::store::Store;
use jj_lib::{diff, files, rewrite};
use pollster::FutureExt;
use tracing::instrument;
use unicode_width::UnicodeWidthStr as _;

use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::config::CommandNameAndArgs;
use crate::formatter::Formatter;
use crate::merge_tools::{self, ExternalMergeTool};
use crate::text_util;
use crate::ui::Ui;

const DEFAULT_CONTEXT_LINES: usize = 3;

#[derive(clap::Args, Clone, Debug)]
#[command(next_help_heading = "Diff Formatting Options")]
#[command(group(clap::ArgGroup::new("short-format").args(&["summary", "stat", "types"])))]
#[command(group(clap::ArgGroup::new("long-format").args(&["git", "color_words", "tool"])))]
pub struct DiffFormatArgs {
    /// For each path, show only whether it was modified, added, or deleted
    #[arg(long, short)]
    pub summary: bool,
    /// Show a histogram of the changes
    #[arg(long)]
    pub stat: bool,
    /// For each path, show only its type before and after
    ///
    /// The diff is shown as two letters. The first letter indicates the type
    /// before and the second letter indicates the type after. '-' indicates
    /// that the path was not present, 'F' represents a regular file, `L'
    /// represents a symlink, 'C' represents a conflict, and 'G' represents a
    /// Git submodule.
    #[arg(long)]
    pub types: bool,
    /// Show a Git-format diff
    #[arg(long)]
    pub git: bool,
    /// Show a word-level diff with changes indicated only by color
    #[arg(long)]
    pub color_words: bool,
    /// Generate diff by external command
    #[arg(long)]
    pub tool: Option<String>,
    /// Number of lines of context to show
    #[arg(long)]
    context: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffFormat {
    Summary,
    Stat,
    Types,
    Git { context: usize },
    ColorWords { context: usize },
    Tool(Box<ExternalMergeTool>),
}

/// Returns a list of requested diff formats, which will never be empty.
pub fn diff_formats_for(
    settings: &UserSettings,
    args: &DiffFormatArgs,
) -> Result<Vec<DiffFormat>, config::ConfigError> {
    let formats = diff_formats_from_args(settings, args)?;
    if formats.is_empty() {
        Ok(vec![default_diff_format(settings, args.context)?])
    } else {
        Ok(formats)
    }
}

/// Returns a list of requested diff formats for log-like commands, which may be
/// empty.
pub fn diff_formats_for_log(
    settings: &UserSettings,
    args: &DiffFormatArgs,
    patch: bool,
) -> Result<Vec<DiffFormat>, config::ConfigError> {
    let mut formats = diff_formats_from_args(settings, args)?;
    // --patch implies default if no format other than --summary is specified
    if patch && matches!(formats.as_slice(), [] | [DiffFormat::Summary]) {
        formats.push(default_diff_format(settings, args.context)?);
        formats.dedup();
    }
    Ok(formats)
}

fn diff_formats_from_args(
    settings: &UserSettings,
    args: &DiffFormatArgs,
) -> Result<Vec<DiffFormat>, config::ConfigError> {
    let mut formats = [
        (args.summary, DiffFormat::Summary),
        (args.types, DiffFormat::Types),
        (
            args.git,
            DiffFormat::Git {
                context: args.context.unwrap_or(DEFAULT_CONTEXT_LINES),
            },
        ),
        (
            args.color_words,
            DiffFormat::ColorWords {
                context: args.context.unwrap_or(DEFAULT_CONTEXT_LINES),
            },
        ),
        (args.stat, DiffFormat::Stat),
    ]
    .into_iter()
    .filter_map(|(arg, format)| arg.then_some(format))
    .collect_vec();
    if let Some(name) = &args.tool {
        let tool = merge_tools::get_external_tool_config(settings, name)?
            .unwrap_or_else(|| ExternalMergeTool::with_program(name));
        formats.push(DiffFormat::Tool(Box::new(tool)));
    }
    Ok(formats)
}

fn default_diff_format(
    settings: &UserSettings,
    num_context_lines: Option<usize>,
) -> Result<DiffFormat, config::ConfigError> {
    let config = settings.config();
    if let Some(args) = config.get("ui.diff.tool").optional()? {
        // External "tool" overrides the internal "format" option.
        let tool = if let CommandNameAndArgs::String(name) = &args {
            merge_tools::get_external_tool_config(settings, name)?
        } else {
            None
        }
        .unwrap_or_else(|| ExternalMergeTool::with_diff_args(&args));
        return Ok(DiffFormat::Tool(Box::new(tool)));
    }
    let name = if let Some(name) = config.get_string("ui.diff.format").optional()? {
        name
    } else if let Some(name) = config.get_string("diff.format").optional()? {
        name // old config name
    } else {
        "color-words".to_owned()
    };
    match name.as_ref() {
        "summary" => Ok(DiffFormat::Summary),
        "types" => Ok(DiffFormat::Types),
        "git" => Ok(DiffFormat::Git {
            context: num_context_lines.unwrap_or(DEFAULT_CONTEXT_LINES),
        }),
        "color-words" => Ok(DiffFormat::ColorWords {
            context: num_context_lines.unwrap_or(DEFAULT_CONTEXT_LINES),
        }),
        "stat" => Ok(DiffFormat::Stat),
        _ => Err(config::ConfigError::Message(format!(
            "invalid diff format: {name}"
        ))),
    }
}

pub fn show_diff(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    from_tree: &MergedTree,
    to_tree: &MergedTree,
    matcher: &dyn Matcher,
    formats: &[DiffFormat],
) -> Result<(), CommandError> {
    for format in formats {
        match format {
            DiffFormat::Summary => {
                let tree_diff = from_tree.diff_stream(to_tree, matcher);
                show_diff_summary(formatter, workspace_command, tree_diff)?;
            }
            DiffFormat::Stat => {
                let tree_diff = from_tree.diff_stream(to_tree, matcher);
                show_diff_stat(ui, formatter, workspace_command, tree_diff)?;
            }
            DiffFormat::Types => {
                let tree_diff = from_tree.diff_stream(to_tree, matcher);
                show_types(formatter, workspace_command, tree_diff)?;
            }
            DiffFormat::Git { context } => {
                let tree_diff = from_tree.diff_stream(to_tree, matcher);
                show_git_diff(formatter, workspace_command, *context, tree_diff)?;
            }
            DiffFormat::ColorWords { context } => {
                let tree_diff = from_tree.diff_stream(to_tree, matcher);
                show_color_words_diff(formatter, workspace_command, *context, tree_diff)?;
            }
            DiffFormat::Tool(tool) => {
                merge_tools::generate_diff(ui, formatter.raw(), from_tree, to_tree, matcher, tool)?;
            }
        }
    }
    Ok(())
}

pub fn show_patch(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    matcher: &dyn Matcher,
    formats: &[DiffFormat],
) -> Result<(), CommandError> {
    let parents = commit.parents();
    let from_tree = rewrite::merge_commit_trees(workspace_command.repo().as_ref(), &parents)?;
    let to_tree = commit.tree()?;
    show_diff(
        ui,
        formatter,
        workspace_command,
        &from_tree,
        &to_tree,
        matcher,
        formats,
    )
}

fn show_color_words_diff_hunks(
    left: &[u8],
    right: &[u8],
    num_context_lines: usize,
    formatter: &mut dyn Formatter,
) -> io::Result<()> {
    const SKIPPED_CONTEXT_LINE: &str = "    ...\n";
    let mut context = VecDeque::new();
    // Have we printed "..." for any skipped context?
    let mut skipped_context = false;
    // Are the lines in `context` to be printed before the next modified line?
    let mut context_before = true;
    for diff_line in files::diff(left, right) {
        if diff_line.is_unmodified() {
            context.push_back(diff_line.clone());
            let mut start_skipping_context = false;
            if context_before {
                if skipped_context && context.len() > num_context_lines {
                    context.pop_front();
                } else if !skipped_context && context.len() > num_context_lines + 1 {
                    start_skipping_context = true;
                }
            } else if context.len() > num_context_lines * 2 + 1 {
                for line in context.drain(..num_context_lines) {
                    show_color_words_diff_line(formatter, &line)?;
                }
                start_skipping_context = true;
            }
            if start_skipping_context {
                context.drain(..2);
                write!(formatter, "{SKIPPED_CONTEXT_LINE}")?;
                skipped_context = true;
                context_before = true;
            }
        } else {
            for line in &context {
                show_color_words_diff_line(formatter, line)?;
            }
            context.clear();
            show_color_words_diff_line(formatter, &diff_line)?;
            context_before = false;
            skipped_context = false;
        }
    }
    if !context_before {
        if context.len() > num_context_lines + 1 {
            context.truncate(num_context_lines);
            skipped_context = true;
            context_before = true;
        }
        for line in &context {
            show_color_words_diff_line(formatter, line)?;
        }
        if context_before {
            write!(formatter, "{SKIPPED_CONTEXT_LINE}")?;
        }
    }

    // If the last diff line doesn't end with newline, add it.
    let no_hunk = left.is_empty() && right.is_empty();
    let any_last_newline = left.ends_with(b"\n") || right.ends_with(b"\n");
    if !skipped_context && !no_hunk && !any_last_newline {
        writeln!(formatter)?;
    }

    Ok(())
}

fn show_color_words_diff_line(
    formatter: &mut dyn Formatter,
    diff_line: &DiffLine,
) -> io::Result<()> {
    if diff_line.has_left_content {
        write!(
            formatter.labeled("removed"),
            "{:>4}",
            diff_line.left_line_number
        )?;
        write!(formatter, " ")?;
    } else {
        write!(formatter, "     ")?;
    }
    if diff_line.has_right_content {
        write!(
            formatter.labeled("added"),
            "{:>4}",
            diff_line.right_line_number
        )?;
        write!(formatter, ": ")?;
    } else {
        write!(formatter, "    : ")?;
    }
    for hunk in &diff_line.hunks {
        match hunk {
            DiffHunk::Matching(data) => {
                formatter.write_all(data)?;
            }
            DiffHunk::Different(data) => {
                let before = data[0];
                let after = data[1];
                if !before.is_empty() {
                    formatter.with_label("removed", |formatter| formatter.write_all(before))?;
                }
                if !after.is_empty() {
                    formatter.with_label("added", |formatter| formatter.write_all(after))?;
                }
            }
        }
    }

    Ok(())
}

struct FileContent {
    /// false if this file is likely text; true if it is likely binary.
    is_binary: bool,
    contents: Vec<u8>,
}

impl FileContent {
    fn empty() -> Self {
        Self {
            is_binary: false,
            contents: vec![],
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.contents.is_empty()
    }
}

fn file_content_for_diff(reader: &mut dyn io::Read) -> io::Result<FileContent> {
    // If this is a binary file, don't show the full contents.
    // Determine whether it's binary by whether the first 8k bytes contain a null
    // character; this is the same heuristic used by git as of writing: https://github.com/git/git/blob/eea0e59ffbed6e33d171ace5be13cde9faa41639/xdiff-interface.c#L192-L198
    const PEEK_SIZE: usize = 8000;
    // TODO: currently we look at the whole file, even though for binary files we
    // only need to know the file size. To change that we'd have to extend all
    // the data backends to support getting the length.
    let mut contents = vec![];
    reader.read_to_end(&mut contents)?;

    let start = &contents[..PEEK_SIZE.min(contents.len())];
    Ok(FileContent {
        is_binary: start.contains(&b'\0'),
        contents,
    })
}

fn diff_content(
    path: &RepoPath,
    value: MaterializedTreeValue,
) -> Result<FileContent, CommandError> {
    match value {
        MaterializedTreeValue::Absent => Ok(FileContent::empty()),
        MaterializedTreeValue::File { mut reader, .. } => {
            file_content_for_diff(&mut reader).map_err(Into::into)
        }
        MaterializedTreeValue::Symlink { id: _, target } => Ok(FileContent {
            // Unix file paths can't contain null bytes.
            is_binary: false,
            contents: target.into_bytes(),
        }),
        MaterializedTreeValue::GitSubmodule(id) => Ok(FileContent {
            is_binary: false,
            contents: format!("Git submodule checked out at {}", id.hex()).into_bytes(),
        }),
        // TODO: are we sure this is never binary?
        MaterializedTreeValue::Conflict { id: _, contents } => Ok(FileContent {
            is_binary: false,
            contents,
        }),
        MaterializedTreeValue::Tree(id) => {
            panic!("Unexpected tree with id {id:?} in diff at path {path:?}");
        }
    }
}

fn basic_diff_file_type(value: &MaterializedTreeValue) -> &'static str {
    match value {
        MaterializedTreeValue::Absent => {
            panic!("absent path in diff");
        }
        MaterializedTreeValue::File { executable, .. } => {
            if *executable {
                "executable file"
            } else {
                "regular file"
            }
        }
        MaterializedTreeValue::Symlink { .. } => "symlink",
        MaterializedTreeValue::Tree(_) => "tree",
        MaterializedTreeValue::GitSubmodule(_) => "Git submodule",
        MaterializedTreeValue::Conflict { .. } => "conflict",
    }
}

pub fn show_color_words_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    num_context_lines: usize,
    tree_diff: TreeDiffStream,
) -> Result<(), CommandError> {
    formatter.push_label("diff")?;
    let mut diff_stream = materialized_diff_stream(workspace_command.repo().store(), tree_diff);
    async {
        while let Some((path, diff)) = diff_stream.next().await {
            let ui_path = workspace_command.format_file_path(&path);
            let (left_value, right_value) = diff?;
            if left_value.is_absent() {
                let description = basic_diff_file_type(&right_value);
                writeln!(
                    formatter.labeled("header"),
                    "Added {description} {ui_path}:"
                )?;
                let right_content = diff_content(&path, right_value)?;
                if right_content.is_empty() {
                    writeln!(formatter.labeled("empty"), "    (empty)")?;
                } else if right_content.is_binary {
                    writeln!(formatter.labeled("binary"), "    (binary)")?;
                } else {
                    show_color_words_diff_hunks(
                        &[],
                        &right_content.contents,
                        num_context_lines,
                        formatter,
                    )?;
                }
            } else if right_value.is_present() {
                let description = match (&left_value, &right_value) {
                    (
                        MaterializedTreeValue::File {
                            executable: left_executable,
                            ..
                        },
                        MaterializedTreeValue::File {
                            executable: right_executable,
                            ..
                        },
                    ) => {
                        if *left_executable && *right_executable {
                            "Modified executable file".to_string()
                        } else if *left_executable {
                            "Executable file became non-executable at".to_string()
                        } else if *right_executable {
                            "Non-executable file became executable at".to_string()
                        } else {
                            "Modified regular file".to_string()
                        }
                    }
                    (
                        MaterializedTreeValue::Conflict { .. },
                        MaterializedTreeValue::Conflict { .. },
                    ) => "Modified conflict in".to_string(),
                    (MaterializedTreeValue::Conflict { .. }, _) => {
                        "Resolved conflict in".to_string()
                    }
                    (_, MaterializedTreeValue::Conflict { .. }) => {
                        "Created conflict in".to_string()
                    }
                    (
                        MaterializedTreeValue::Symlink { .. },
                        MaterializedTreeValue::Symlink { .. },
                    ) => "Symlink target changed at".to_string(),
                    (_, _) => {
                        let left_type = basic_diff_file_type(&left_value);
                        let right_type = basic_diff_file_type(&right_value);
                        let (first, rest) = left_type.split_at(1);
                        format!(
                            "{}{} became {} at",
                            first.to_ascii_uppercase(),
                            rest,
                            right_type
                        )
                    }
                };
                let left_content = diff_content(&path, left_value)?;
                let right_content = diff_content(&path, right_value)?;
                writeln!(formatter.labeled("header"), "{description} {ui_path}:")?;
                if left_content.is_binary || right_content.is_binary {
                    writeln!(formatter.labeled("binary"), "    (binary)")?;
                } else {
                    show_color_words_diff_hunks(
                        &left_content.contents,
                        &right_content.contents,
                        num_context_lines,
                        formatter,
                    )?;
                }
            } else {
                let description = basic_diff_file_type(&left_value);
                writeln!(
                    formatter.labeled("header"),
                    "Removed {description} {ui_path}:"
                )?;
                let left_content = diff_content(&path, left_value)?;
                if left_content.is_empty() {
                    writeln!(formatter.labeled("empty"), "    (empty)")?;
                } else if left_content.is_binary {
                    writeln!(formatter.labeled("binary"), "    (binary)")?;
                } else {
                    show_color_words_diff_hunks(
                        &left_content.contents,
                        &[],
                        num_context_lines,
                        formatter,
                    )?;
                }
            }
        }
        Ok::<(), CommandError>(())
    }
    .block_on()?;
    formatter.pop_label()?;
    Ok(())
}

struct GitDiffPart {
    mode: String,
    hash: String,
    content: Vec<u8>,
}

fn git_diff_part(
    path: &RepoPath,
    value: MaterializedTreeValue,
) -> Result<GitDiffPart, CommandError> {
    let mode;
    let hash;
    let mut contents: Vec<u8>;
    match value {
        MaterializedTreeValue::Absent => {
            panic!("Absent path {path:?} in diff should have been handled by caller");
        }
        MaterializedTreeValue::File {
            id,
            executable,
            mut reader,
        } => {
            mode = if executable {
                "100755".to_string()
            } else {
                "100644".to_string()
            };
            hash = id.hex();
            // TODO: use `file_content_for_diff` instead of showing binary
            contents = vec![];
            reader.read_to_end(&mut contents)?;
        }
        MaterializedTreeValue::Symlink { id, target } => {
            mode = "120000".to_string();
            hash = id.hex();
            contents = target.into_bytes();
        }
        MaterializedTreeValue::GitSubmodule(id) => {
            // TODO: What should we actually do here?
            mode = "040000".to_string();
            hash = id.hex();
            contents = vec![];
        }
        MaterializedTreeValue::Conflict {
            id: _,
            contents: conflict_data,
        } => {
            mode = "100644".to_string();
            hash = "0000000000".to_string();
            contents = conflict_data
        }
        MaterializedTreeValue::Tree(_) => {
            panic!("Unexpected tree in diff at path {path:?}");
        }
    }
    let hash = hash[0..10].to_string();
    Ok(GitDiffPart {
        mode,
        hash,
        content: contents,
    })
}

#[derive(PartialEq)]
enum DiffLineType {
    Context,
    Removed,
    Added,
}

struct UnifiedDiffHunk<'content> {
    left_line_range: Range<usize>,
    right_line_range: Range<usize>,
    lines: Vec<(DiffLineType, &'content [u8])>,
}

fn unified_diff_hunks<'content>(
    left_content: &'content [u8],
    right_content: &'content [u8],
    num_context_lines: usize,
) -> Vec<UnifiedDiffHunk<'content>> {
    let mut hunks = vec![];
    let mut current_hunk = UnifiedDiffHunk {
        left_line_range: 1..1,
        right_line_range: 1..1,
        lines: vec![],
    };
    let mut show_context_after = false;
    let diff = Diff::for_tokenizer(&[left_content, right_content], &diff::find_line_ranges);
    for hunk in diff.hunks() {
        match hunk {
            DiffHunk::Matching(content) => {
                let lines = content.split_inclusive(|b| *b == b'\n').collect_vec();
                // Number of context lines to print after the previous non-matching hunk.
                let num_after_lines = lines.len().min(if show_context_after {
                    num_context_lines
                } else {
                    0
                });
                current_hunk.left_line_range.end += num_after_lines;
                current_hunk.right_line_range.end += num_after_lines;
                for line in lines.iter().take(num_after_lines) {
                    current_hunk.lines.push((DiffLineType::Context, line));
                }
                let num_skip_lines = lines
                    .len()
                    .saturating_sub(num_after_lines)
                    .saturating_sub(num_context_lines);
                if num_skip_lines > 0 {
                    let left_start = current_hunk.left_line_range.end + num_skip_lines;
                    let right_start = current_hunk.right_line_range.end + num_skip_lines;
                    if !current_hunk.lines.is_empty() {
                        hunks.push(current_hunk);
                    }
                    current_hunk = UnifiedDiffHunk {
                        left_line_range: left_start..left_start,
                        right_line_range: right_start..right_start,
                        lines: vec![],
                    };
                }
                let num_before_lines = lines.len() - num_after_lines - num_skip_lines;
                current_hunk.left_line_range.end += num_before_lines;
                current_hunk.right_line_range.end += num_before_lines;
                for line in lines.iter().skip(num_after_lines + num_skip_lines) {
                    current_hunk.lines.push((DiffLineType::Context, line));
                }
            }
            DiffHunk::Different(content) => {
                show_context_after = true;
                let left_lines = content[0].split_inclusive(|b| *b == b'\n').collect_vec();
                let right_lines = content[1].split_inclusive(|b| *b == b'\n').collect_vec();
                if !left_lines.is_empty() {
                    current_hunk.left_line_range.end += left_lines.len();
                    for line in left_lines {
                        current_hunk.lines.push((DiffLineType::Removed, line));
                    }
                }
                if !right_lines.is_empty() {
                    current_hunk.right_line_range.end += right_lines.len();
                    for line in right_lines {
                        current_hunk.lines.push((DiffLineType::Added, line));
                    }
                }
            }
        }
    }
    if !current_hunk
        .lines
        .iter()
        .all(|(diff_type, _line)| *diff_type == DiffLineType::Context)
    {
        hunks.push(current_hunk);
    }
    hunks
}

fn show_unified_diff_hunks(
    formatter: &mut dyn Formatter,
    left_content: &[u8],
    right_content: &[u8],
    num_context_lines: usize,
) -> Result<(), CommandError> {
    for hunk in unified_diff_hunks(left_content, right_content, num_context_lines) {
        writeln!(
            formatter.labeled("hunk_header"),
            "@@ -{},{} +{},{} @@",
            hunk.left_line_range.start,
            hunk.left_line_range.len(),
            hunk.right_line_range.start,
            hunk.right_line_range.len()
        )?;
        for (line_type, content) in hunk.lines {
            match line_type {
                DiffLineType::Context => {
                    formatter.with_label("context", |formatter| {
                        write!(formatter, " ")?;
                        formatter.write_all(content)
                    })?;
                }
                DiffLineType::Removed => {
                    formatter.with_label("removed", |formatter| {
                        write!(formatter, "-")?;
                        formatter.write_all(content)
                    })?;
                }
                DiffLineType::Added => {
                    formatter.with_label("added", |formatter| {
                        write!(formatter, "+")?;
                        formatter.write_all(content)
                    })?;
                }
            }
            if !content.ends_with(b"\n") {
                write!(formatter, "\n\\ No newline at end of file\n")?;
            }
        }
    }
    Ok(())
}

fn materialized_diff_stream<'a>(
    store: &'a Store,
    tree_diff: TreeDiffStream<'a>,
) -> impl Stream<
    Item = (
        RepoPathBuf,
        BackendResult<(MaterializedTreeValue, MaterializedTreeValue)>,
    ),
> + 'a {
    tree_diff
        .map(|(path, diff)| async {
            match diff {
                Err(err) => (path, Err(err)),
                Ok((before, after)) => {
                    let before_future = materialize_tree_value(store, &path, before);
                    let after_future = materialize_tree_value(store, &path, after);
                    let values = try_join!(before_future, after_future);
                    (path, values)
                }
            }
        })
        .buffered((store.concurrency() / 2).max(1))
}

pub fn show_git_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    num_context_lines: usize,
    tree_diff: TreeDiffStream,
) -> Result<(), CommandError> {
    formatter.push_label("diff")?;

    let mut diff_stream = materialized_diff_stream(workspace_command.repo().store(), tree_diff);
    async {
        while let Some((path, diff)) = diff_stream.next().await {
            let path_string = path.as_internal_file_string();
            let (left_value, right_value) = diff?;
            if left_value.is_absent() {
                let right_part = git_diff_part(&path, right_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{path_string} b/{path_string}")?;
                    writeln!(formatter, "new file mode {}", &right_part.mode)?;
                    writeln!(formatter, "index 0000000000..{}", &right_part.hash)?;
                    writeln!(formatter, "--- /dev/null")?;
                    writeln!(formatter, "+++ b/{path_string}")
                })?;
                show_unified_diff_hunks(formatter, &[], &right_part.content, num_context_lines)?;
            } else if right_value.is_present() {
                let left_part = git_diff_part(&path, left_value)?;
                let right_part = git_diff_part(&path, right_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{path_string} b/{path_string}")?;
                    if left_part.mode != right_part.mode {
                        writeln!(formatter, "old mode {}", &left_part.mode)?;
                        writeln!(formatter, "new mode {}", &right_part.mode)?;
                        if left_part.hash != right_part.hash {
                            writeln!(formatter, "index {}...{}", &left_part.hash, right_part.hash)?;
                        }
                    } else if left_part.hash != right_part.hash {
                        writeln!(
                            formatter,
                            "index {}...{} {}",
                            &left_part.hash, right_part.hash, left_part.mode
                        )?;
                    }
                    if left_part.content != right_part.content {
                        writeln!(formatter, "--- a/{path_string}")?;
                        writeln!(formatter, "+++ b/{path_string}")?;
                    }
                    Ok(())
                })?;
                show_unified_diff_hunks(
                    formatter,
                    &left_part.content,
                    &right_part.content,
                    num_context_lines,
                )?;
            } else {
                let left_part = git_diff_part(&path, left_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{path_string} b/{path_string}")?;
                    writeln!(formatter, "deleted file mode {}", &left_part.mode)?;
                    writeln!(formatter, "index {}..0000000000", &left_part.hash)?;
                    writeln!(formatter, "--- a/{path_string}")?;
                    writeln!(formatter, "+++ /dev/null")
                })?;
                show_unified_diff_hunks(formatter, &left_part.content, &[], num_context_lines)?;
            }
        }
        Ok::<(), CommandError>(())
    }
    .block_on()?;
    formatter.pop_label()?;
    Ok(())
}

#[instrument(skip_all)]
pub fn show_diff_summary(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    mut tree_diff: TreeDiffStream,
) -> io::Result<()> {
    formatter.with_label("diff", |formatter| -> io::Result<()> {
        async {
            while let Some((repo_path, diff)) = tree_diff.next().await {
                let (before, after) = diff.unwrap();
                if before.is_present() && after.is_present() {
                    writeln!(
                        formatter.labeled("modified"),
                        "M {}",
                        workspace_command.format_file_path(&repo_path)
                    )?;
                } else if before.is_absent() {
                    writeln!(
                        formatter.labeled("added"),
                        "A {}",
                        workspace_command.format_file_path(&repo_path)
                    )?;
                } else {
                    writeln!(
                        formatter.labeled("removed"),
                        "D {}", // `R` could be interpreted as "renamed"
                        workspace_command.format_file_path(&repo_path)
                    )?;
                }
            }
            Ok(())
        }
        .block_on()
    })
}

struct DiffStat {
    path: String,
    added: usize,
    removed: usize,
}

fn get_diff_stat(
    path: String,
    left_content: &FileContent,
    right_content: &FileContent,
) -> DiffStat {
    // TODO: this matches git's behavior, which is to count the number of newlines
    // in the file. but that behavior seems unhelpful; no one really cares how
    // many `0xa0` characters are in an image.
    let hunks = unified_diff_hunks(&left_content.contents, &right_content.contents, 0);
    let mut added = 0;
    let mut removed = 0;
    for hunk in hunks {
        for (line_type, _content) in hunk.lines {
            match line_type {
                DiffLineType::Context => {}
                DiffLineType::Removed => removed += 1,
                DiffLineType::Added => added += 1,
            }
        }
    }
    DiffStat {
        path,
        added,
        removed,
    }
}

pub fn show_diff_stat(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffStream,
) -> Result<(), CommandError> {
    let mut stats: Vec<DiffStat> = vec![];
    let mut max_path_width = 0;
    let mut max_diffs = 0;

    let mut diff_stream = materialized_diff_stream(workspace_command.repo().store(), tree_diff);
    async {
        while let Some((repo_path, diff)) = diff_stream.next().await {
            let (left, right) = diff?;
            let path = workspace_command.format_file_path(&repo_path);
            let left_content = diff_content(&repo_path, left)?;
            let right_content = diff_content(&repo_path, right)?;
            max_path_width = max(max_path_width, path.width());
            let stat = get_diff_stat(path, &left_content, &right_content);
            max_diffs = max(max_diffs, stat.added + stat.removed);
            stats.push(stat);
        }
        Ok::<(), CommandError>(())
    }
    .block_on()?;

    let number_padding = max_diffs.to_string().len();
    // 4 characters padding for the graph
    let available_width =
        usize::from(ui.term_width().unwrap_or(80)).saturating_sub(4 + " | ".len() + number_padding);
    // Always give at least a tiny bit of room
    let available_width = max(available_width, 5);
    let max_path_width = max_path_width.clamp(3, (0.7 * available_width as f64) as usize);
    let max_bar_length = available_width.saturating_sub(max_path_width);
    let factor = if max_diffs < max_bar_length {
        1.0
    } else {
        max_bar_length as f64 / max_diffs as f64
    };

    formatter.with_label("diff", |formatter| {
        let mut total_added = 0;
        let mut total_removed = 0;
        let total_files = stats.len();
        for stat in &stats {
            total_added += stat.added;
            total_removed += stat.removed;
            let bar_added = (stat.added as f64 * factor).ceil() as usize;
            let bar_removed = (stat.removed as f64 * factor).ceil() as usize;
            // replace start of path with ellipsis if the path is too long
            let (path, path_width) = text_util::elide_start(&stat.path, "...", max_path_width);
            let path_pad_width = max_path_width - path_width;
            write!(
                formatter,
                "{path}{:path_pad_width$} | {:>number_padding$}{}",
                "", // pad to max_path_width
                stat.added + stat.removed,
                if bar_added + bar_removed > 0 { " " } else { "" },
            )?;
            write!(formatter.labeled("added"), "{}", "+".repeat(bar_added))?;
            writeln!(formatter.labeled("removed"), "{}", "-".repeat(bar_removed))?;
        }
        writeln!(
            formatter.labeled("stat-summary"),
            "{} file{} changed, {} insertion{}(+), {} deletion{}(-)",
            total_files,
            if total_files == 1 { "" } else { "s" },
            total_added,
            if total_added == 1 { "" } else { "s" },
            total_removed,
            if total_removed == 1 { "" } else { "s" },
        )?;
        Ok(())
    })?;
    Ok(())
}

pub fn show_types(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    mut tree_diff: TreeDiffStream,
) -> io::Result<()> {
    formatter.with_label("diff", |formatter| {
        async {
            while let Some((repo_path, diff)) = tree_diff.next().await {
                let (before, after) = diff.unwrap();
                writeln!(
                    formatter.labeled("modified"),
                    "{}{} {}",
                    diff_summary_char(&before),
                    diff_summary_char(&after),
                    workspace_command.format_file_path(&repo_path)
                )?;
            }
            Ok(())
        }
        .block_on()
    })
}

fn diff_summary_char(value: &MergedTreeValue) -> char {
    match value.as_resolved() {
        Some(None) => '-',
        Some(Some(TreeValue::File { .. })) => 'F',
        Some(Some(TreeValue::Symlink(_))) => 'L',
        Some(Some(TreeValue::GitSubmodule(_))) => 'G',
        None => 'C',
        Some(Some(TreeValue::Tree(_))) | Some(Some(TreeValue::Conflict(_))) => {
            panic!("Unexpected {value:?} in diff")
        }
    }
}
