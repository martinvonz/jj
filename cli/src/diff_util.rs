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

use std::borrow::Borrow;
use std::cmp::max;
use std::collections::HashSet;
use std::io;
use std::mem;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;

use bstr::BStr;
use futures::executor::block_on_stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use itertools::Itertools;
use jj_lib::backend::BackendError;
use jj_lib::backend::BackendResult;
use jj_lib::backend::CommitId;
use jj_lib::backend::CopyRecord;
use jj_lib::backend::TreeValue;
use jj_lib::commit::Commit;
use jj_lib::config::ConfigGetError;
use jj_lib::config::ConfigGetResultExt as _;
use jj_lib::conflicts::materialize_merge_result_to_bytes;
use jj_lib::conflicts::materialized_diff_stream;
use jj_lib::conflicts::ConflictMarkerStyle;
use jj_lib::conflicts::MaterializedTreeDiffEntry;
use jj_lib::conflicts::MaterializedTreeValue;
use jj_lib::copies::CopiesTreeDiffEntry;
use jj_lib::copies::CopyOperation;
use jj_lib::copies::CopyRecords;
use jj_lib::diff::find_line_ranges;
use jj_lib::diff::CompareBytesExactly;
use jj_lib::diff::CompareBytesIgnoreAllWhitespace;
use jj_lib::diff::CompareBytesIgnoreWhitespaceAmount;
use jj_lib::diff::Diff;
use jj_lib::diff::DiffHunk;
use jj_lib::diff::DiffHunkContentVec;
use jj_lib::diff::DiffHunkKind;
use jj_lib::files::DiffLineHunkSide;
use jj_lib::files::DiffLineIterator;
use jj_lib::files::DiffLineNumber;
use jj_lib::matchers::Matcher;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::repo_path::InvalidRepoPathError;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathUiConverter;
use jj_lib::rewrite::rebase_to_dest_parent;
use jj_lib::settings::UserSettings;
use jj_lib::store::Store;
use pollster::FutureExt;
use thiserror::Error;
use tracing::instrument;
use unicode_width::UnicodeWidthStr as _;

use crate::config::CommandNameAndArgs;
use crate::formatter::Formatter;
use crate::merge_tools;
use crate::merge_tools::generate_diff;
use crate::merge_tools::invoke_external_diff;
use crate::merge_tools::new_utf8_temp_dir;
use crate::merge_tools::DiffGenerateError;
use crate::merge_tools::DiffToolMode;
use crate::merge_tools::ExternalMergeTool;
use crate::text_util;
use crate::ui::Ui;

pub const DEFAULT_CONTEXT_LINES: usize = 3;

#[derive(clap::Args, Clone, Debug)]
#[command(next_help_heading = "Diff Formatting Options")]
#[command(group(clap::ArgGroup::new("short-format").args(&["summary", "stat", "types", "name_only"])))]
#[command(group(clap::ArgGroup::new("long-format").args(&["git", "color_words", "tool"])))]
pub struct DiffFormatArgs {
    /// For each path, show only whether it was modified, added, or deleted
    #[arg(long, short)]
    pub summary: bool,
    /// Show a histogram of the changes
    #[arg(long, short = 'S')]
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
    /// For each path, show only its path
    ///
    /// Typically useful for shell commands like:
    ///    `jj diff -r @- --name_only | xargs perl -pi -e's/OLD/NEW/g`
    #[arg(long)]
    pub name_only: bool,
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

    // Short flags are set by command to avoid future conflicts.
    /// Ignore whitespace when comparing lines.
    #[arg(long)] // short = 'w'
    ignore_all_space: bool,
    /// Ignore changes in amount of whitespace when comparing lines.
    #[arg(long, conflicts_with = "ignore_all_space")] // short = 'b'
    ignore_space_change: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffFormat {
    // Non-trivial parameters are boxed in order to keep the variants small
    Summary,
    Stat(Box<DiffStatOptions>),
    Types,
    NameOnly,
    Git(Box<UnifiedDiffOptions>),
    ColorWords(Box<ColorWordsDiffOptions>),
    Tool(Box<ExternalMergeTool>),
}

/// Returns a list of requested diff formats, which will never be empty.
pub fn diff_formats_for(
    settings: &UserSettings,
    args: &DiffFormatArgs,
) -> Result<Vec<DiffFormat>, ConfigGetError> {
    let formats = diff_formats_from_args(settings, args)?;
    if formats.is_empty() {
        Ok(vec![default_diff_format(settings, args)?])
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
) -> Result<Vec<DiffFormat>, ConfigGetError> {
    let mut formats = diff_formats_from_args(settings, args)?;
    // --patch implies default if no format other than --summary is specified
    if patch && matches!(formats.as_slice(), [] | [DiffFormat::Summary]) {
        formats.push(default_diff_format(settings, args)?);
        formats.dedup();
    }
    Ok(formats)
}

fn diff_formats_from_args(
    settings: &UserSettings,
    args: &DiffFormatArgs,
) -> Result<Vec<DiffFormat>, ConfigGetError> {
    let mut formats = Vec::new();
    if args.summary {
        formats.push(DiffFormat::Summary);
    }
    if args.types {
        formats.push(DiffFormat::Types);
    }
    if args.name_only {
        formats.push(DiffFormat::NameOnly);
    }
    if args.git {
        let options = UnifiedDiffOptions::from_settings_and_args(settings, args)?;
        formats.push(DiffFormat::Git(Box::new(options)));
    }
    if args.color_words {
        let options = ColorWordsDiffOptions::from_settings_and_args(settings, args)?;
        formats.push(DiffFormat::ColorWords(Box::new(options)));
    }
    if args.stat {
        let options = DiffStatOptions::from_args(args);
        formats.push(DiffFormat::Stat(Box::new(options)));
    }
    if let Some(name) = &args.tool {
        let tool = merge_tools::get_external_tool_config(settings, name)?
            .unwrap_or_else(|| ExternalMergeTool::with_program(name));
        formats.push(DiffFormat::Tool(Box::new(tool)));
    }
    Ok(formats)
}

fn default_diff_format(
    settings: &UserSettings,
    args: &DiffFormatArgs,
) -> Result<DiffFormat, ConfigGetError> {
    if let Some(args) = settings.get("ui.diff.tool").optional()? {
        // External "tool" overrides the internal "format" option.
        let tool = if let CommandNameAndArgs::String(name) = &args {
            merge_tools::get_external_tool_config(settings, name)?
        } else {
            None
        }
        .unwrap_or_else(|| ExternalMergeTool::with_diff_args(&args));
        return Ok(DiffFormat::Tool(Box::new(tool)));
    }
    let name = if let Some(name) = settings.get_string("ui.diff.format").optional()? {
        name
    } else if let Some(name) = settings.get_string("diff.format").optional()? {
        name // old config name
    } else {
        "color-words".to_owned()
    };
    match name.as_ref() {
        "summary" => Ok(DiffFormat::Summary),
        "types" => Ok(DiffFormat::Types),
        "name-only" => Ok(DiffFormat::NameOnly),
        "git" => {
            let options = UnifiedDiffOptions::from_settings_and_args(settings, args)?;
            Ok(DiffFormat::Git(Box::new(options)))
        }
        "color-words" => {
            let options = ColorWordsDiffOptions::from_settings_and_args(settings, args)?;
            Ok(DiffFormat::ColorWords(Box::new(options)))
        }
        "stat" => {
            let options = DiffStatOptions::from_args(args);
            Ok(DiffFormat::Stat(Box::new(options)))
        }
        _ => Err(ConfigGetError::Type {
            name: "ui.diff.format".to_owned(),
            error: format!("Invalid diff format: {name}").into(),
            source_path: None,
        }),
    }
}

#[derive(Debug, Error)]
pub enum DiffRenderError {
    #[error("Failed to generate diff")]
    DiffGenerate(#[source] DiffGenerateError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("Access denied to {path}")]
    AccessDenied {
        path: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(transparent)]
    InvalidRepoPath(#[from] InvalidRepoPathError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Configuration and environment to render textual diff.
pub struct DiffRenderer<'a> {
    repo: &'a dyn Repo,
    path_converter: &'a RepoPathUiConverter,
    conflict_marker_style: ConflictMarkerStyle,
    formats: Vec<DiffFormat>,
}

impl<'a> DiffRenderer<'a> {
    pub fn new(
        repo: &'a dyn Repo,
        path_converter: &'a RepoPathUiConverter,
        conflict_marker_style: ConflictMarkerStyle,
        formats: Vec<DiffFormat>,
    ) -> Self {
        DiffRenderer {
            repo,
            path_converter,
            conflict_marker_style,
            formats,
        }
    }

    /// Generates diff between `from_tree` and `to_tree`.
    #[allow(clippy::too_many_arguments)]
    pub fn show_diff(
        &self,
        ui: &Ui, // TODO: remove Ui dependency if possible
        formatter: &mut dyn Formatter,
        from_tree: &MergedTree,
        to_tree: &MergedTree,
        matcher: &dyn Matcher,
        copy_records: &CopyRecords,
        width: usize,
    ) -> Result<(), DiffRenderError> {
        formatter.with_label("diff", |formatter| {
            self.show_diff_inner(
                ui,
                formatter,
                from_tree,
                to_tree,
                matcher,
                copy_records,
                width,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn show_diff_inner(
        &self,
        ui: &Ui,
        formatter: &mut dyn Formatter,
        from_tree: &MergedTree,
        to_tree: &MergedTree,
        matcher: &dyn Matcher,
        copy_records: &CopyRecords,
        width: usize,
    ) -> Result<(), DiffRenderError> {
        let store = self.repo.store();
        let path_converter = self.path_converter;
        for format in &self.formats {
            match format {
                DiffFormat::Summary => {
                    let tree_diff =
                        from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                    show_diff_summary(formatter, tree_diff, path_converter)?;
                }
                DiffFormat::Stat(options) => {
                    let tree_diff =
                        from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                    show_diff_stat(
                        formatter,
                        store,
                        tree_diff,
                        path_converter,
                        options,
                        width,
                        self.conflict_marker_style,
                    )?;
                }
                DiffFormat::Types => {
                    let tree_diff =
                        from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                    show_types(formatter, tree_diff, path_converter)?;
                }
                DiffFormat::NameOnly => {
                    let tree_diff =
                        from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                    show_names(formatter, tree_diff, path_converter)?;
                }
                DiffFormat::Git(options) => {
                    let tree_diff =
                        from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                    show_git_diff(
                        formatter,
                        store,
                        tree_diff,
                        options,
                        self.conflict_marker_style,
                    )?;
                }
                DiffFormat::ColorWords(options) => {
                    let tree_diff =
                        from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                    show_color_words_diff(
                        formatter,
                        store,
                        tree_diff,
                        path_converter,
                        options,
                        self.conflict_marker_style,
                    )?;
                }
                DiffFormat::Tool(tool) => {
                    match tool.diff_invocation_mode {
                        DiffToolMode::FileByFile => {
                            let tree_diff =
                                from_tree.diff_stream_with_copies(to_tree, matcher, copy_records);
                            show_file_by_file_diff(
                                ui,
                                formatter,
                                store,
                                tree_diff,
                                path_converter,
                                tool,
                                self.conflict_marker_style,
                            )
                        }
                        DiffToolMode::Dir => {
                            let mut writer = formatter.raw()?;
                            generate_diff(
                                ui,
                                writer.as_mut(),
                                from_tree,
                                to_tree,
                                matcher,
                                tool,
                                self.conflict_marker_style,
                            )
                            .map_err(DiffRenderError::DiffGenerate)
                        }
                    }?;
                }
            }
        }
        Ok(())
    }

    /// Generates diff between `from_commits` and `to_commit` based off their
    /// parents. The `from_commits` will temporarily be rebased onto the
    /// `to_commit` parents to exclude unrelated changes.
    pub fn show_inter_diff(
        &self,
        ui: &Ui,
        formatter: &mut dyn Formatter,
        from_commits: &[Commit],
        to_commit: &Commit,
        matcher: &dyn Matcher,
        width: usize,
    ) -> Result<(), DiffRenderError> {
        let from_tree = rebase_to_dest_parent(self.repo, from_commits, to_commit)?;
        let to_tree = to_commit.tree()?;
        let copy_records = CopyRecords::default(); // TODO
        self.show_diff(
            ui,
            formatter,
            &from_tree,
            &to_tree,
            matcher,
            &copy_records,
            width,
        )
    }

    /// Generates diff of the given `commit` compared to its parents.
    pub fn show_patch(
        &self,
        ui: &Ui,
        formatter: &mut dyn Formatter,
        commit: &Commit,
        matcher: &dyn Matcher,
        width: usize,
    ) -> Result<(), DiffRenderError> {
        let from_tree = commit.parent_tree(self.repo)?;
        let to_tree = commit.tree()?;
        let mut copy_records = CopyRecords::default();
        for parent_id in commit.parent_ids() {
            let records = get_copy_records(self.repo.store(), parent_id, commit.id(), matcher)?;
            copy_records.add_records(records)?;
        }
        self.show_diff(
            ui,
            formatter,
            &from_tree,
            &to_tree,
            matcher,
            &copy_records,
            width,
        )
    }
}

pub fn get_copy_records<'a>(
    store: &'a Store,
    root: &CommitId,
    head: &CommitId,
    matcher: &'a dyn Matcher,
) -> BackendResult<impl Iterator<Item = BackendResult<CopyRecord>> + 'a> {
    // TODO: teach backend about matching path prefixes?
    let stream = store.get_copy_records(None, root, head)?;
    // TODO: test record.source as well? should be AND-ed or OR-ed?
    Ok(block_on_stream(stream).filter_ok(|record| matcher.matches(&record.target)))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineDiffOptions {
    /// How equivalence of lines is tested.
    pub compare_mode: LineCompareMode,
    // TODO: add --ignore-blank-lines, etc. which aren't mutually exclusive.
}

impl LineDiffOptions {
    fn from_args(args: &DiffFormatArgs) -> Self {
        let compare_mode = if args.ignore_all_space {
            LineCompareMode::IgnoreAllSpace
        } else if args.ignore_space_change {
            LineCompareMode::IgnoreSpaceChange
        } else {
            LineCompareMode::Exact
        };
        LineDiffOptions { compare_mode }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineCompareMode {
    /// Compares lines literally.
    Exact,
    /// Compares lines ignoring any whitespace occurrences.
    IgnoreAllSpace,
    /// Compares lines ignoring changes in whitespace amount.
    IgnoreSpaceChange,
}

fn diff_by_line<'input, T: AsRef<[u8]> + ?Sized + 'input>(
    inputs: impl IntoIterator<Item = &'input T>,
    options: &LineDiffOptions,
) -> Diff<'input> {
    // TODO: If we add --ignore-blank-lines, its tokenizer will have to attach
    // blank lines to the preceding range. Maybe it can also be implemented as a
    // post-process (similar to refine_changed_regions()) that expands unchanged
    // regions across blank lines.
    match options.compare_mode {
        LineCompareMode::Exact => {
            Diff::for_tokenizer(inputs, find_line_ranges, CompareBytesExactly)
        }
        LineCompareMode::IgnoreAllSpace => {
            Diff::for_tokenizer(inputs, find_line_ranges, CompareBytesIgnoreAllWhitespace)
        }
        LineCompareMode::IgnoreSpaceChange => {
            Diff::for_tokenizer(inputs, find_line_ranges, CompareBytesIgnoreWhitespaceAmount)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColorWordsDiffOptions {
    /// Number of context lines to show.
    pub context: usize,
    /// How lines are tokenized and compared.
    pub line_diff: LineDiffOptions,
    /// Maximum number of removed/added word alternation to inline.
    pub max_inline_alternation: Option<usize>,
}

impl ColorWordsDiffOptions {
    fn from_settings_and_args(
        settings: &UserSettings,
        args: &DiffFormatArgs,
    ) -> Result<Self, ConfigGetError> {
        let max_inline_alternation = {
            let name = "diff.color-words.max-inline-alternation";
            match settings.get_int(name)? {
                -1 => None, // unlimited
                n => Some(usize::try_from(n).map_err(|err| ConfigGetError::Type {
                    name: name.to_owned(),
                    error: err.into(),
                    source_path: None,
                })?),
            }
        };
        let context = args
            .context
            .map_or_else(|| settings.get("diff.color-words.context"), Ok)?;
        Ok(ColorWordsDiffOptions {
            context,
            line_diff: LineDiffOptions::from_args(args),
            max_inline_alternation,
        })
    }
}

fn show_color_words_diff_hunks(
    formatter: &mut dyn Formatter,
    left: &[u8],
    right: &[u8],
    options: &ColorWordsDiffOptions,
) -> io::Result<()> {
    let line_diff = diff_by_line([left, right], &options.line_diff);
    let mut line_number = DiffLineNumber { left: 1, right: 1 };
    // Matching entries shouldn't appear consecutively in diff of two inputs.
    // However, if the inputs have conflicts, there may be a hunk that can be
    // resolved, resulting [matching, resolved, matching] sequence.
    let mut contexts = Vec::new();
    let mut emitted = false;

    for hunk in line_diff.hunks() {
        match hunk.kind {
            DiffHunkKind::Matching => contexts.push(hunk.contents),
            DiffHunkKind::Different => {
                let num_after = if emitted { options.context } else { 0 };
                line_number = show_color_words_context_lines(
                    formatter,
                    &contexts,
                    line_number,
                    options,
                    num_after,
                    options.context,
                )?;
                contexts.clear();
                emitted = true;
                line_number =
                    show_color_words_diff_lines(formatter, &hunk.contents, line_number, options)?;
            }
        }
    }

    if emitted {
        show_color_words_context_lines(
            formatter,
            &contexts,
            line_number,
            options,
            options.context,
            0,
        )?;
    }
    Ok(())
}

/// Prints `num_after` lines, ellipsis, and `num_before` lines.
fn show_color_words_context_lines(
    formatter: &mut dyn Formatter,
    contexts: &[DiffHunkContentVec],
    mut line_number: DiffLineNumber,
    options: &ColorWordsDiffOptions,
    num_after: usize,
    num_before: usize,
) -> io::Result<DiffLineNumber> {
    const SKIPPED_CONTEXT_LINE: &str = "    ...\n";
    let extract = |side: usize| -> (Vec<&[u8]>, Vec<&[u8]>, u32) {
        let mut lines = contexts
            .iter()
            .flat_map(|contents| contents[side].split_inclusive(|b| *b == b'\n'))
            .fuse();
        let after_lines = lines.by_ref().take(num_after).collect();
        let before_lines = lines.by_ref().rev().take(num_before + 1).collect();
        let num_skipped: u32 = lines.count().try_into().unwrap();
        (after_lines, before_lines, num_skipped)
    };
    let show = |formatter: &mut dyn Formatter,
                left_lines: &[&[u8]],
                right_lines: &[&[u8]],
                mut line_number: DiffLineNumber| {
        if left_lines == right_lines {
            for line in left_lines {
                show_color_words_line_number(
                    formatter,
                    Some(line_number.left),
                    Some(line_number.right),
                )?;
                show_color_words_inline_hunks(
                    formatter,
                    &[(DiffLineHunkSide::Both, line.as_ref())],
                )?;
                line_number.left += 1;
                line_number.right += 1;
            }
            Ok(line_number)
        } else {
            let left = left_lines.concat();
            let right = right_lines.concat();
            show_color_words_diff_lines(
                formatter,
                &[BStr::new(&left), BStr::new(&right)],
                line_number,
                options,
            )
        }
    };

    let (left_after, mut left_before, num_left_skipped) = extract(0);
    let (right_after, mut right_before, num_right_skipped) = extract(1);
    line_number = show(formatter, &left_after, &right_after, line_number)?;
    if num_left_skipped > 0 || num_right_skipped > 0 {
        write!(formatter, "{SKIPPED_CONTEXT_LINE}")?;
        line_number.left += num_left_skipped;
        line_number.right += num_right_skipped;
        if left_before.len() > num_before {
            left_before.pop();
            line_number.left += 1;
        }
        if right_before.len() > num_before {
            right_before.pop();
            line_number.right += 1;
        }
    }
    left_before.reverse();
    right_before.reverse();
    line_number = show(formatter, &left_before, &right_before, line_number)?;
    Ok(line_number)
}

fn show_color_words_diff_lines(
    formatter: &mut dyn Formatter,
    contents: &[&BStr],
    mut line_number: DiffLineNumber,
    options: &ColorWordsDiffOptions,
) -> io::Result<DiffLineNumber> {
    let word_diff_hunks = Diff::by_word(contents).hunks().collect_vec();
    let can_inline = match options.max_inline_alternation {
        None => true,     // unlimited
        Some(0) => false, // no need to count alternation
        Some(max_num) => {
            let groups = split_diff_hunks_by_matching_newline(&word_diff_hunks);
            groups.map(count_diff_alternation).max().unwrap_or(0) <= max_num
        }
    };
    if can_inline {
        let mut diff_line_iter =
            DiffLineIterator::with_line_number(word_diff_hunks.iter(), line_number);
        for diff_line in diff_line_iter.by_ref() {
            show_color_words_line_number(
                formatter,
                diff_line
                    .has_left_content()
                    .then_some(diff_line.line_number.left),
                diff_line
                    .has_right_content()
                    .then_some(diff_line.line_number.right),
            )?;
            show_color_words_inline_hunks(formatter, &diff_line.hunks)?;
        }
        line_number = diff_line_iter.next_line_number();
    } else {
        let (left_lines, right_lines) = unzip_diff_hunks_to_lines(&word_diff_hunks);
        for tokens in &left_lines {
            show_color_words_line_number(formatter, Some(line_number.left), None)?;
            show_color_words_single_sided_line(formatter, tokens, "removed")?;
            line_number.left += 1;
        }
        for tokens in &right_lines {
            show_color_words_line_number(formatter, None, Some(line_number.right))?;
            show_color_words_single_sided_line(formatter, tokens, "added")?;
            line_number.right += 1;
        }
    }
    Ok(line_number)
}

fn show_color_words_line_number(
    formatter: &mut dyn Formatter,
    left_line_number: Option<u32>,
    right_line_number: Option<u32>,
) -> io::Result<()> {
    if let Some(line_number) = left_line_number {
        formatter.with_label("removed", |formatter| {
            write!(formatter.labeled("line_number"), "{line_number:>4}")
        })?;
        write!(formatter, " ")?;
    } else {
        write!(formatter, "     ")?;
    }
    if let Some(line_number) = right_line_number {
        formatter.with_label("added", |formatter| {
            write!(formatter.labeled("line_number"), "{line_number:>4}",)
        })?;
        write!(formatter, ": ")?;
    } else {
        write!(formatter, "    : ")?;
    }
    Ok(())
}

/// Prints line hunks which may contain tokens originating from both sides.
fn show_color_words_inline_hunks(
    formatter: &mut dyn Formatter,
    line_hunks: &[(DiffLineHunkSide, &BStr)],
) -> io::Result<()> {
    for (side, data) in line_hunks {
        let label = match side {
            DiffLineHunkSide::Both => None,
            DiffLineHunkSide::Left => Some("removed"),
            DiffLineHunkSide::Right => Some("added"),
        };
        if let Some(label) = label {
            formatter.with_label(label, |formatter| {
                formatter.with_label("token", |formatter| formatter.write_all(data))
            })?;
        } else {
            formatter.write_all(data)?;
        }
    }
    let (_, data) = line_hunks.last().expect("diff line must not be empty");
    if !data.ends_with(b"\n") {
        writeln!(formatter)?;
    };
    Ok(())
}

/// Prints left/right-only line tokens with the given label.
fn show_color_words_single_sided_line(
    formatter: &mut dyn Formatter,
    tokens: &[(DiffTokenType, &[u8])],
    label: &str,
) -> io::Result<()> {
    formatter.with_label(label, |formatter| show_diff_line_tokens(formatter, tokens))?;
    let (_, data) = tokens.last().expect("diff line must not be empty");
    if !data.ends_with(b"\n") {
        writeln!(formatter)?;
    };
    Ok(())
}

/// Counts number of diff-side alternation, ignoring matching hunks.
///
/// This function is meant to measure visual complexity of diff hunks. It's easy
/// to read hunks containing some removed or added words, but is getting harder
/// as more removes and adds interleaved.
///
/// For example,
/// - `[matching]` -> 0
/// - `[left]` -> 1
/// - `[left, matching, left]` -> 1
/// - `[matching, left, right, matching, right]` -> 2
/// - `[left, right, matching, right, left]` -> 3
fn count_diff_alternation(diff_hunks: &[DiffHunk]) -> usize {
    diff_hunks
        .iter()
        .filter_map(|hunk| match hunk.kind {
            DiffHunkKind::Matching => None,
            DiffHunkKind::Different => Some(&hunk.contents),
        })
        // Map non-empty diff side to index (0: left, 1: right)
        .flat_map(|contents| contents.iter().positions(|content| !content.is_empty()))
        // Omit e.g. left->(matching->)*left
        .dedup()
        .count()
}

/// Splits hunks into slices of contiguous changed lines.
fn split_diff_hunks_by_matching_newline<'a, 'b>(
    diff_hunks: &'a [DiffHunk<'b>],
) -> impl Iterator<Item = &'a [DiffHunk<'b>]> {
    diff_hunks.split_inclusive(|hunk| match hunk.kind {
        DiffHunkKind::Matching => hunk.contents.iter().all(|content| content.contains(&b'\n')),
        DiffHunkKind::Different => false,
    })
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
    conflict_marker_style: ConflictMarkerStyle,
) -> io::Result<FileContent> {
    match value {
        MaterializedTreeValue::Absent => Ok(FileContent::empty()),
        MaterializedTreeValue::AccessDenied(err) => Ok(FileContent {
            is_binary: false,
            contents: format!("Access denied: {err}").into_bytes(),
        }),
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
            contents: format!("Git submodule checked out at {id}").into_bytes(),
        }),
        // TODO: are we sure this is never binary?
        MaterializedTreeValue::FileConflict {
            id: _,
            contents,
            executable: _,
        } => Ok(FileContent {
            is_binary: false,
            contents: materialize_merge_result_to_bytes(&contents, conflict_marker_style).into(),
        }),
        MaterializedTreeValue::OtherConflict { id } => Ok(FileContent {
            is_binary: false,
            contents: id.describe().into_bytes(),
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
        MaterializedTreeValue::AccessDenied(_) => "access denied",
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
        MaterializedTreeValue::FileConflict { .. }
        | MaterializedTreeValue::OtherConflict { .. } => "conflict",
    }
}

pub fn show_color_words_diff(
    formatter: &mut dyn Formatter,
    store: &Store,
    tree_diff: BoxStream<CopiesTreeDiffEntry>,
    path_converter: &RepoPathUiConverter,
    options: &ColorWordsDiffOptions,
    conflict_marker_style: ConflictMarkerStyle,
) -> Result<(), DiffRenderError> {
    let mut diff_stream = materialized_diff_stream(store, tree_diff);
    async {
        while let Some(MaterializedTreeDiffEntry { path, values }) = diff_stream.next().await {
            let left_path = path.source();
            let right_path = path.target();
            let left_ui_path = path_converter.format_file_path(left_path);
            let right_ui_path = path_converter.format_file_path(right_path);
            let (left_value, right_value) = values?;

            match (&left_value, &right_value) {
                (MaterializedTreeValue::AccessDenied(source), _) => {
                    write!(
                        formatter.labeled("access-denied"),
                        "Access denied to {left_ui_path}:"
                    )?;
                    writeln!(formatter, " {source}")?;
                    continue;
                }
                (_, MaterializedTreeValue::AccessDenied(source)) => {
                    write!(
                        formatter.labeled("access-denied"),
                        "Access denied to {right_ui_path}:"
                    )?;
                    writeln!(formatter, " {source}")?;
                    continue;
                }
                _ => {}
            }
            if left_value.is_absent() {
                let description = basic_diff_file_type(&right_value);
                writeln!(
                    formatter.labeled("header"),
                    "Added {description} {right_ui_path}:"
                )?;
                let right_content = diff_content(right_path, right_value, conflict_marker_style)?;
                if right_content.is_empty() {
                    writeln!(formatter.labeled("empty"), "    (empty)")?;
                } else if right_content.is_binary {
                    writeln!(formatter.labeled("binary"), "    (binary)")?;
                } else {
                    show_color_words_diff_hunks(formatter, &[], &right_content.contents, options)?;
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
                        MaterializedTreeValue::FileConflict { .. }
                        | MaterializedTreeValue::OtherConflict { .. },
                        MaterializedTreeValue::FileConflict { .. }
                        | MaterializedTreeValue::OtherConflict { .. },
                    ) => "Modified conflict in".to_string(),
                    (
                        MaterializedTreeValue::FileConflict { .. }
                        | MaterializedTreeValue::OtherConflict { .. },
                        _,
                    ) => "Resolved conflict in".to_string(),
                    (
                        _,
                        MaterializedTreeValue::FileConflict { .. }
                        | MaterializedTreeValue::OtherConflict { .. },
                    ) => "Created conflict in".to_string(),
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
                let left_content = diff_content(left_path, left_value, conflict_marker_style)?;
                let right_content = diff_content(right_path, right_value, conflict_marker_style)?;
                if left_path == right_path {
                    writeln!(
                        formatter.labeled("header"),
                        "{description} {right_ui_path}:"
                    )?;
                } else {
                    writeln!(
                        formatter.labeled("header"),
                        "{description} {right_ui_path} ({left_ui_path} => {right_ui_path}):"
                    )?;
                }
                if left_content.is_binary || right_content.is_binary {
                    writeln!(formatter.labeled("binary"), "    (binary)")?;
                } else {
                    show_color_words_diff_hunks(
                        formatter,
                        &left_content.contents,
                        &right_content.contents,
                        options,
                    )?;
                }
            } else {
                let description = basic_diff_file_type(&left_value);
                writeln!(
                    formatter.labeled("header"),
                    "Removed {description} {right_ui_path}:"
                )?;
                let left_content = diff_content(left_path, left_value, conflict_marker_style)?;
                if left_content.is_empty() {
                    writeln!(formatter.labeled("empty"), "    (empty)")?;
                } else if left_content.is_binary {
                    writeln!(formatter.labeled("binary"), "    (binary)")?;
                } else {
                    show_color_words_diff_hunks(formatter, &left_content.contents, &[], options)?;
                }
            }
        }
        Ok(())
    }
    .block_on()
}

pub fn show_file_by_file_diff(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    store: &Store,
    tree_diff: BoxStream<CopiesTreeDiffEntry>,
    path_converter: &RepoPathUiConverter,
    tool: &ExternalMergeTool,
    conflict_marker_style: ConflictMarkerStyle,
) -> Result<(), DiffRenderError> {
    let create_file = |path: &RepoPath,
                       wc_dir: &Path,
                       value: MaterializedTreeValue|
     -> Result<PathBuf, DiffRenderError> {
        let fs_path = path.to_fs_path(wc_dir)?;
        std::fs::create_dir_all(fs_path.parent().unwrap())?;
        let content = diff_content(path, value, conflict_marker_style)?;
        std::fs::write(&fs_path, content.contents)?;
        Ok(fs_path)
    };

    let temp_dir = new_utf8_temp_dir("jj-diff-")?;
    let left_wc_dir = temp_dir.path().join("left");
    let right_wc_dir = temp_dir.path().join("right");
    let mut diff_stream = materialized_diff_stream(store, tree_diff);
    async {
        while let Some(MaterializedTreeDiffEntry { path, values }) = diff_stream.next().await {
            let (left_value, right_value) = values?;
            let left_path = path.source();
            let right_path = path.target();
            let left_ui_path = path_converter.format_file_path(left_path);
            let right_ui_path = path_converter.format_file_path(right_path);

            match (&left_value, &right_value) {
                (_, MaterializedTreeValue::AccessDenied(source)) => {
                    write!(
                        formatter.labeled("access-denied"),
                        "Access denied to {right_ui_path}:"
                    )?;
                    writeln!(formatter, " {source}")?;
                    continue;
                }
                (MaterializedTreeValue::AccessDenied(source), _) => {
                    write!(
                        formatter.labeled("access-denied"),
                        "Access denied to {left_ui_path}:"
                    )?;
                    writeln!(formatter, " {source}")?;
                    continue;
                }
                _ => {}
            }
            let left_path = create_file(left_path, &left_wc_dir, left_value)?;
            let right_path = create_file(right_path, &right_wc_dir, right_value)?;

            let mut writer = formatter.raw()?;
            invoke_external_diff(
                ui,
                writer.as_mut(),
                tool,
                &maplit::hashmap! {
                    "left" => left_path.to_str().expect("temp_dir should be valid utf-8"),
                    "right" => right_path.to_str().expect("temp_dir should be valid utf-8"),
                },
            )
            .map_err(DiffRenderError::DiffGenerate)?;
        }
        Ok::<(), DiffRenderError>(())
    }
    .block_on()
}

struct GitDiffPart {
    /// Octal mode string or `None` if the file is absent.
    mode: Option<&'static str>,
    hash: String,
    content: FileContent,
}

fn git_diff_part(
    path: &RepoPath,
    value: MaterializedTreeValue,
    conflict_marker_style: ConflictMarkerStyle,
) -> Result<GitDiffPart, DiffRenderError> {
    const DUMMY_HASH: &str = "0000000000";
    let mode;
    let mut hash;
    let content;
    match value {
        MaterializedTreeValue::Absent => {
            return Ok(GitDiffPart {
                mode: None,
                hash: DUMMY_HASH.to_owned(),
                content: FileContent::empty(),
            });
        }
        MaterializedTreeValue::AccessDenied(err) => {
            return Err(DiffRenderError::AccessDenied {
                path: path.as_internal_file_string().to_owned(),
                source: err,
            });
        }
        MaterializedTreeValue::File {
            id,
            executable,
            mut reader,
        } => {
            mode = if executable { "100755" } else { "100644" };
            hash = id.hex();
            content = file_content_for_diff(&mut reader)?;
        }
        MaterializedTreeValue::Symlink { id, target } => {
            mode = "120000";
            hash = id.hex();
            content = FileContent {
                // Unix file paths can't contain null bytes.
                is_binary: false,
                contents: target.into_bytes(),
            };
        }
        MaterializedTreeValue::GitSubmodule(id) => {
            // TODO: What should we actually do here?
            mode = "040000";
            hash = id.hex();
            content = FileContent::empty();
        }
        MaterializedTreeValue::FileConflict {
            id: _,
            contents,
            executable,
        } => {
            mode = if executable { "100755" } else { "100644" };
            hash = DUMMY_HASH.to_owned();
            content = FileContent {
                is_binary: false, // TODO: are we sure this is never binary?
                contents: materialize_merge_result_to_bytes(&contents, conflict_marker_style)
                    .into(),
            };
        }
        MaterializedTreeValue::OtherConflict { id } => {
            mode = "100644";
            hash = DUMMY_HASH.to_owned();
            content = FileContent {
                is_binary: false,
                contents: id.describe().into_bytes(),
            };
        }
        MaterializedTreeValue::Tree(_) => {
            panic!("Unexpected tree in diff at path {path:?}");
        }
    }
    hash.truncate(10);
    Ok(GitDiffPart {
        mode: Some(mode),
        hash,
        content,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnifiedDiffOptions {
    /// Number of context lines to show.
    pub context: usize,
    /// How lines are tokenized and compared.
    pub line_diff: LineDiffOptions,
}

impl UnifiedDiffOptions {
    fn from_settings_and_args(
        settings: &UserSettings,
        args: &DiffFormatArgs,
    ) -> Result<Self, ConfigGetError> {
        let context = args
            .context
            .map_or_else(|| settings.get("diff.git.context"), Ok)?;
        Ok(UnifiedDiffOptions {
            context,
            line_diff: LineDiffOptions::from_args(args),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLineType {
    Context,
    Removed,
    Added,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffTokenType {
    Matching,
    Different,
}

type DiffTokenVec<'content> = Vec<(DiffTokenType, &'content [u8])>;

struct UnifiedDiffHunk<'content> {
    left_line_range: Range<usize>,
    right_line_range: Range<usize>,
    lines: Vec<(DiffLineType, DiffTokenVec<'content>)>,
}

impl<'content> UnifiedDiffHunk<'content> {
    fn extend_context_lines(&mut self, lines: impl IntoIterator<Item = &'content [u8]>) {
        let old_len = self.lines.len();
        self.lines.extend(lines.into_iter().map(|line| {
            let tokens = vec![(DiffTokenType::Matching, line)];
            (DiffLineType::Context, tokens)
        }));
        self.left_line_range.end += self.lines.len() - old_len;
        self.right_line_range.end += self.lines.len() - old_len;
    }

    fn extend_removed_lines(&mut self, lines: impl IntoIterator<Item = DiffTokenVec<'content>>) {
        let old_len = self.lines.len();
        self.lines
            .extend(lines.into_iter().map(|line| (DiffLineType::Removed, line)));
        self.left_line_range.end += self.lines.len() - old_len;
    }

    fn extend_added_lines(&mut self, lines: impl IntoIterator<Item = DiffTokenVec<'content>>) {
        let old_len = self.lines.len();
        self.lines
            .extend(lines.into_iter().map(|line| (DiffLineType::Added, line)));
        self.right_line_range.end += self.lines.len() - old_len;
    }
}

fn unified_diff_hunks<'content>(
    left_content: &'content [u8],
    right_content: &'content [u8],
    options: &UnifiedDiffOptions,
) -> Vec<UnifiedDiffHunk<'content>> {
    let mut hunks = vec![];
    let mut current_hunk = UnifiedDiffHunk {
        left_line_range: 0..0,
        right_line_range: 0..0,
        lines: vec![],
    };
    let diff = diff_by_line([left_content, right_content], &options.line_diff);
    let mut diff_hunks = diff.hunks().peekable();
    while let Some(hunk) = diff_hunks.next() {
        match hunk.kind {
            DiffHunkKind::Matching => {
                // Just use the right (i.e. new) content. We could count the
                // number of skipped lines separately, but the number of the
                // context lines should match the displayed content.
                let [_, right] = hunk.contents[..].try_into().unwrap();
                let mut lines = right.split_inclusive(|b| *b == b'\n').fuse();
                if !current_hunk.lines.is_empty() {
                    // The previous hunk line should be either removed/added.
                    current_hunk.extend_context_lines(lines.by_ref().take(options.context));
                }
                let before_lines = if diff_hunks.peek().is_some() {
                    lines.by_ref().rev().take(options.context).collect()
                } else {
                    vec![] // No more hunks
                };
                let num_skip_lines = lines.count();
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
                // The next hunk should be of DiffHunk::Different type if any.
                current_hunk.extend_context_lines(before_lines.into_iter().rev());
            }
            DiffHunkKind::Different => {
                let (left_lines, right_lines) =
                    unzip_diff_hunks_to_lines(Diff::by_word(hunk.contents).hunks());
                current_hunk.extend_removed_lines(left_lines);
                current_hunk.extend_added_lines(right_lines);
            }
        }
    }
    if !current_hunk.lines.is_empty() {
        hunks.push(current_hunk);
    }
    hunks
}

/// Splits `(left, right)` hunk pairs into `(left_lines, right_lines)`.
fn unzip_diff_hunks_to_lines<'content, I>(
    diff_hunks: I,
) -> (Vec<DiffTokenVec<'content>>, Vec<DiffTokenVec<'content>>)
where
    I: IntoIterator,
    I::Item: Borrow<DiffHunk<'content>>,
{
    let mut left_lines: Vec<DiffTokenVec<'content>> = vec![];
    let mut right_lines: Vec<DiffTokenVec<'content>> = vec![];
    let mut left_tokens: DiffTokenVec<'content> = vec![];
    let mut right_tokens: DiffTokenVec<'content> = vec![];

    for hunk in diff_hunks {
        let hunk = hunk.borrow();
        match hunk.kind {
            DiffHunkKind::Matching => {
                // TODO: add support for unmatched contexts
                debug_assert!(hunk.contents.iter().all_equal());
                for token in hunk.contents[0].split_inclusive(|b| *b == b'\n') {
                    left_tokens.push((DiffTokenType::Matching, token));
                    right_tokens.push((DiffTokenType::Matching, token));
                    if token.ends_with(b"\n") {
                        left_lines.push(mem::take(&mut left_tokens));
                        right_lines.push(mem::take(&mut right_tokens));
                    }
                }
            }
            DiffHunkKind::Different => {
                let [left, right] = hunk.contents[..]
                    .try_into()
                    .expect("hunk should have exactly two inputs");
                for token in left.split_inclusive(|b| *b == b'\n') {
                    left_tokens.push((DiffTokenType::Different, token));
                    if token.ends_with(b"\n") {
                        left_lines.push(mem::take(&mut left_tokens));
                    }
                }
                for token in right.split_inclusive(|b| *b == b'\n') {
                    right_tokens.push((DiffTokenType::Different, token));
                    if token.ends_with(b"\n") {
                        right_lines.push(mem::take(&mut right_tokens));
                    }
                }
            }
        }
    }

    if !left_tokens.is_empty() {
        left_lines.push(left_tokens);
    }
    if !right_tokens.is_empty() {
        right_lines.push(right_tokens);
    }
    (left_lines, right_lines)
}

fn show_unified_diff_hunks(
    formatter: &mut dyn Formatter,
    left_content: &[u8],
    right_content: &[u8],
    options: &UnifiedDiffOptions,
) -> io::Result<()> {
    // "If the chunk size is 0, the first number is one lower than one would
    // expect." - https://www.artima.com/weblogs/viewpost.jsp?thread=164293
    //
    // The POSIX spec also states that "the ending line number of an empty range
    // shall be the number of the preceding line, or 0 if the range is at the
    // start of the file."
    // - https://pubs.opengroup.org/onlinepubs/9799919799/utilities/diff.html
    fn to_line_number(range: Range<usize>) -> usize {
        if range.is_empty() {
            range.start
        } else {
            range.start + 1
        }
    }

    for hunk in unified_diff_hunks(left_content, right_content, options) {
        writeln!(
            formatter.labeled("hunk_header"),
            "@@ -{},{} +{},{} @@",
            to_line_number(hunk.left_line_range.clone()),
            hunk.left_line_range.len(),
            to_line_number(hunk.right_line_range.clone()),
            hunk.right_line_range.len()
        )?;
        for (line_type, tokens) in &hunk.lines {
            let (label, sigil) = match line_type {
                DiffLineType::Context => ("context", " "),
                DiffLineType::Removed => ("removed", "-"),
                DiffLineType::Added => ("added", "+"),
            };
            formatter.with_label(label, |formatter| {
                write!(formatter, "{sigil}")?;
                show_diff_line_tokens(formatter, tokens)
            })?;
            let (_, content) = tokens.last().expect("hunk line must not be empty");
            if !content.ends_with(b"\n") {
                write!(formatter, "\n\\ No newline at end of file\n")?;
            }
        }
    }
    Ok(())
}

fn show_diff_line_tokens(
    formatter: &mut dyn Formatter,
    tokens: &[(DiffTokenType, &[u8])],
) -> io::Result<()> {
    for (token_type, content) in tokens {
        match token_type {
            DiffTokenType::Matching => formatter.write_all(content)?,
            DiffTokenType::Different => {
                formatter.with_label("token", |formatter| formatter.write_all(content))?;
            }
        }
    }
    Ok(())
}

pub fn show_git_diff(
    formatter: &mut dyn Formatter,
    store: &Store,
    tree_diff: BoxStream<CopiesTreeDiffEntry>,
    options: &UnifiedDiffOptions,
    conflict_marker_style: ConflictMarkerStyle,
) -> Result<(), DiffRenderError> {
    let mut diff_stream = materialized_diff_stream(store, tree_diff);
    async {
        while let Some(MaterializedTreeDiffEntry { path, values }) = diff_stream.next().await {
            let left_path = path.source();
            let right_path = path.target();
            let left_path_string = left_path.as_internal_file_string();
            let right_path_string = right_path.as_internal_file_string();
            let (left_value, right_value) = values?;

            let left_part = git_diff_part(left_path, left_value, conflict_marker_style)?;
            let right_part = git_diff_part(right_path, right_value, conflict_marker_style)?;

            formatter.with_label("file_header", |formatter| {
                writeln!(
                    formatter,
                    "diff --git a/{left_path_string} b/{right_path_string}"
                )?;
                let left_hash = &left_part.hash;
                let right_hash = &right_part.hash;
                match (left_part.mode, right_part.mode) {
                    (None, Some(right_mode)) => {
                        writeln!(formatter, "new file mode {right_mode}")?;
                        writeln!(formatter, "index {left_hash}..{right_hash}")?;
                    }
                    (Some(left_mode), None) => {
                        writeln!(formatter, "deleted file mode {left_mode}")?;
                        writeln!(formatter, "index {left_hash}..{right_hash}")?;
                    }
                    (Some(left_mode), Some(right_mode)) => {
                        if let Some(op) = path.copy_operation() {
                            let operation = match op {
                                CopyOperation::Copy => "copy",
                                CopyOperation::Rename => "rename",
                            };
                            // TODO: include similarity index?
                            writeln!(formatter, "{operation} from {left_path_string}")?;
                            writeln!(formatter, "{operation} to {right_path_string}")?;
                        }
                        if left_mode != right_mode {
                            writeln!(formatter, "old mode {left_mode}")?;
                            writeln!(formatter, "new mode {right_mode}")?;
                            if left_hash != right_hash {
                                writeln!(formatter, "index {left_hash}..{right_hash}")?;
                            }
                        } else if left_hash != right_hash {
                            writeln!(formatter, "index {left_hash}..{right_hash} {left_mode}")?;
                        }
                    }
                    (None, None) => panic!("either left or right part should be present"),
                }
                Ok::<(), DiffRenderError>(())
            })?;

            if left_part.content.contents == right_part.content.contents {
                continue; // no content hunks
            }

            let left_path = match left_part.mode {
                Some(_) => format!("a/{left_path_string}"),
                None => "/dev/null".to_owned(),
            };
            let right_path = match right_part.mode {
                Some(_) => format!("b/{right_path_string}"),
                None => "/dev/null".to_owned(),
            };
            if left_part.content.is_binary || right_part.content.is_binary {
                // TODO: add option to emit Git binary diff
                writeln!(
                    formatter,
                    "Binary files {left_path} and {right_path} differ"
                )?;
            } else {
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "--- {left_path}")?;
                    writeln!(formatter, "+++ {right_path}")?;
                    io::Result::Ok(())
                })?;
                show_unified_diff_hunks(
                    formatter,
                    &left_part.content.contents,
                    &right_part.content.contents,
                    options,
                )?;
            }
        }
        Ok(())
    }
    .block_on()
}

#[instrument(skip_all)]
pub fn show_diff_summary(
    formatter: &mut dyn Formatter,
    mut tree_diff: BoxStream<CopiesTreeDiffEntry>,
    path_converter: &RepoPathUiConverter,
) -> Result<(), DiffRenderError> {
    async {
        while let Some(CopiesTreeDiffEntry { path, values }) = tree_diff.next().await {
            let (before, after) = values?;
            let before_path = path.source();
            let after_path = path.target();
            if let Some(op) = path.copy_operation() {
                let (label, sigil) = match op {
                    CopyOperation::Copy => ("copied", "C"),
                    CopyOperation::Rename => ("renamed", "R"),
                };
                let path = path_converter.format_copied_path(before_path, after_path);
                writeln!(formatter.labeled(label), "{sigil} {path}")?;
            } else {
                let path = path_converter.format_file_path(after_path);
                match (before.is_present(), after.is_present()) {
                    (true, true) => writeln!(formatter.labeled("modified"), "M {path}")?,
                    (false, true) => writeln!(formatter.labeled("added"), "A {path}")?,
                    (true, false) => writeln!(formatter.labeled("removed"), "D {path}")?,
                    (false, false) => unreachable!(),
                }
            }
        }
        Ok(())
    }
    .block_on()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffStatOptions {
    /// How lines are tokenized and compared.
    pub line_diff: LineDiffOptions,
}

impl DiffStatOptions {
    fn from_args(args: &DiffFormatArgs) -> Self {
        DiffStatOptions {
            line_diff: LineDiffOptions::from_args(args),
        }
    }
}

struct DiffStat {
    path: String,
    added: usize,
    removed: usize,
    is_deletion: bool,
}

fn get_diff_stat(
    path: String,
    left_content: &FileContent,
    right_content: &FileContent,
    options: &DiffStatOptions,
) -> DiffStat {
    // TODO: this matches git's behavior, which is to count the number of newlines
    // in the file. but that behavior seems unhelpful; no one really cares how
    // many `0x0a` characters are in an image.
    let diff = diff_by_line(
        [&left_content.contents, &right_content.contents],
        &options.line_diff,
    );
    let mut added = 0;
    let mut removed = 0;
    for hunk in diff.hunks() {
        match hunk.kind {
            DiffHunkKind::Matching => {}
            DiffHunkKind::Different => {
                let [left, right] = hunk.contents[..].try_into().unwrap();
                removed += left.split_inclusive(|b| *b == b'\n').count();
                added += right.split_inclusive(|b| *b == b'\n').count();
            }
        }
    }
    DiffStat {
        path,
        added,
        removed,
        is_deletion: right_content.contents.is_empty(),
    }
}

pub fn show_diff_stat(
    formatter: &mut dyn Formatter,
    store: &Store,
    tree_diff: BoxStream<CopiesTreeDiffEntry>,
    path_converter: &RepoPathUiConverter,
    options: &DiffStatOptions,
    display_width: usize,
    conflict_marker_style: ConflictMarkerStyle,
) -> Result<(), DiffRenderError> {
    let mut stats: Vec<DiffStat> = vec![];
    let mut unresolved_renames = HashSet::new();
    let mut max_path_width = 0;
    let mut max_diffs = 0;

    let mut diff_stream = materialized_diff_stream(store, tree_diff);
    async {
        while let Some(MaterializedTreeDiffEntry { path, values }) = diff_stream.next().await {
            let (left, right) = values?;
            let left_path = path.source();
            let right_path = path.target();
            let left_content = diff_content(left_path, left, conflict_marker_style)?;
            let right_content = diff_content(right_path, right, conflict_marker_style)?;

            let left_ui_path = path_converter.format_file_path(left_path);
            let path = if left_path == right_path {
                left_ui_path
            } else {
                unresolved_renames.insert(left_ui_path);
                path_converter.format_copied_path(left_path, right_path)
            };
            max_path_width = max(max_path_width, path.width());
            let stat = get_diff_stat(path, &left_content, &right_content, options);
            max_diffs = max(max_diffs, stat.added + stat.removed);
            stats.push(stat);
        }
        Ok::<(), DiffRenderError>(())
    }
    .block_on()?;

    let number_padding = max_diffs.to_string().len();
    // 4 characters padding for the graph
    let available_width = display_width.saturating_sub(4 + " | ".len() + number_padding);
    // Always give at least a tiny bit of room
    let available_width = max(available_width, 5);
    let max_path_width = max_path_width.clamp(3, (0.7 * available_width as f64) as usize);
    let max_bar_length = available_width.saturating_sub(max_path_width);
    let factor = if max_diffs < max_bar_length {
        1.0
    } else {
        max_bar_length as f64 / max_diffs as f64
    };

    let mut total_added = 0;
    let mut total_removed = 0;
    let mut total_files = 0;
    for stat in &stats {
        if stat.is_deletion && unresolved_renames.contains(&stat.path) {
            continue;
        }

        total_added += stat.added;
        total_removed += stat.removed;
        total_files += 1;
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
}

pub fn show_types(
    formatter: &mut dyn Formatter,
    mut tree_diff: BoxStream<CopiesTreeDiffEntry>,
    path_converter: &RepoPathUiConverter,
) -> Result<(), DiffRenderError> {
    async {
        while let Some(CopiesTreeDiffEntry { path, values }) = tree_diff.next().await {
            let (before, after) = values?;
            writeln!(
                formatter.labeled("modified"),
                "{}{} {}",
                diff_summary_char(&before),
                diff_summary_char(&after),
                path_converter.format_copied_path(path.source(), path.target())
            )?;
        }
        Ok(())
    }
    .block_on()
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

pub fn show_names(
    formatter: &mut dyn Formatter,
    mut tree_diff: BoxStream<CopiesTreeDiffEntry>,
    path_converter: &RepoPathUiConverter,
) -> io::Result<()> {
    async {
        while let Some(CopiesTreeDiffEntry { path, .. }) = tree_diff.next().await {
            writeln!(
                formatter,
                "{}",
                path_converter.format_file_path(path.target())
            )?;
        }
        Ok(())
    }
    .block_on()
}
