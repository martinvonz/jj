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

use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::sync::Arc;

use clap::ArgGroup;
use itertools::Itertools;
use jujutsu_lib::backend::TreeValue;
use jujutsu_lib::commit::Commit;
use jujutsu_lib::diff::{Diff, DiffHunk};
use jujutsu_lib::files::DiffLine;
use jujutsu_lib::matchers::Matcher;
use jujutsu_lib::repo::ReadonlyRepo;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::tree::{Tree, TreeDiffIterator};
use jujutsu_lib::{conflicts, diff, files, rewrite, tree};

use crate::cli_util::{CommandError, WorkspaceCommandHelper};
use crate::formatter::{Formatter, PlainTextFormatter};
use crate::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("format").args(&["summary", "git", "color_words"])))]
pub struct DiffFormatArgs {
    /// For each path, show only whether it was modified, added, or removed
    #[arg(long, short)]
    pub summary: bool,
    /// Show a Git-format diff
    #[arg(long)]
    pub git: bool,
    /// Show a word-level diff with changes indicated only by color
    #[arg(long)]
    pub color_words: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffFormat {
    Summary,
    Git,
    ColorWords,
}

pub fn diff_format_for(ui: &Ui, args: &DiffFormatArgs) -> DiffFormat {
    if args.summary {
        DiffFormat::Summary
    } else if args.git {
        DiffFormat::Git
    } else if args.color_words {
        DiffFormat::ColorWords
    } else {
        default_diff_format(ui)
    }
}

pub fn diff_format_for_log(ui: &Ui, args: &DiffFormatArgs, patch: bool) -> Option<DiffFormat> {
    (patch || args.git || args.color_words || args.summary).then(|| diff_format_for(ui, args))
}

fn default_diff_format(ui: &Ui) -> DiffFormat {
    match ui.settings().config().get_string("diff.format").as_deref() {
        Ok("summary") => DiffFormat::Summary,
        Ok("git") => DiffFormat::Git,
        Ok("color-words") => DiffFormat::ColorWords,
        _ => DiffFormat::ColorWords,
    }
}

pub fn show_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    from_tree: &Tree,
    to_tree: &Tree,
    matcher: &dyn Matcher,
    format: DiffFormat,
) -> Result<(), CommandError> {
    let tree_diff = from_tree.diff(to_tree, matcher);
    match format {
        DiffFormat::Summary => {
            show_diff_summary(formatter, workspace_command, tree_diff)?;
        }
        DiffFormat::Git => {
            show_git_diff(formatter, workspace_command, tree_diff)?;
        }
        DiffFormat::ColorWords => {
            show_color_words_diff(formatter, workspace_command, tree_diff)?;
        }
    }
    Ok(())
}

pub fn show_patch(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    matcher: &dyn Matcher,
    format: DiffFormat,
) -> Result<(), CommandError> {
    let parents = commit.parents();
    let from_tree = rewrite::merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
    let to_tree = commit.tree();
    show_diff(
        formatter,
        workspace_command,
        &from_tree,
        &to_tree,
        matcher,
        format,
    )
}

pub fn diff_as_bytes(
    workspace_command: &WorkspaceCommandHelper,
    from_tree: &Tree,
    to_tree: &Tree,
    matcher: &dyn Matcher,
    format: DiffFormat,
) -> Result<Vec<u8>, CommandError> {
    let mut diff_bytes: Vec<u8> = vec![];
    let mut formatter = PlainTextFormatter::new(&mut diff_bytes);
    show_diff(
        &mut formatter,
        workspace_command,
        from_tree,
        to_tree,
        matcher,
        format,
    )?;
    Ok(diff_bytes)
}

fn show_color_words_diff_hunks(
    left: &[u8],
    right: &[u8],
    formatter: &mut dyn Formatter,
) -> io::Result<()> {
    const SKIPPED_CONTEXT_LINE: &[u8] = b"    ...\n";
    let num_context_lines = 3;
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
                formatter.write_bytes(SKIPPED_CONTEXT_LINE)?;
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
            formatter.write_bytes(SKIPPED_CONTEXT_LINE)?;
        }
    }

    // If the last diff line doesn't end with newline, add it.
    let no_hunk = left.is_empty() && right.is_empty();
    let any_last_newline = left.ends_with(b"\n") || right.ends_with(b"\n");
    if !skipped_context && !no_hunk && !any_last_newline {
        formatter.write_bytes(b"\n")?;
    }

    Ok(())
}

fn show_color_words_diff_line(
    formatter: &mut dyn Formatter,
    diff_line: &DiffLine,
) -> io::Result<()> {
    if diff_line.has_left_content {
        formatter.with_label("removed", |formatter| {
            formatter.write_bytes(format!("{:>4}", diff_line.left_line_number).as_bytes())
        })?;
        formatter.write_bytes(b" ")?;
    } else {
        formatter.write_bytes(b"     ")?;
    }
    if diff_line.has_right_content {
        formatter.with_label("added", |formatter| {
            formatter.write_bytes(format!("{:>4}", diff_line.right_line_number).as_bytes())
        })?;
        formatter.write_bytes(b": ")?;
    } else {
        formatter.write_bytes(b"    : ")?;
    }
    for hunk in &diff_line.hunks {
        match hunk {
            DiffHunk::Matching(data) => {
                formatter.write_bytes(data)?;
            }
            DiffHunk::Different(data) => {
                let before = data[0];
                let after = data[1];
                if !before.is_empty() {
                    formatter.with_label("removed", |formatter| formatter.write_bytes(before))?;
                }
                if !after.is_empty() {
                    formatter.with_label("added", |formatter| formatter.write_bytes(after))?;
                }
            }
        }
    }

    Ok(())
}

fn diff_content(
    repo: &Arc<ReadonlyRepo>,
    path: &RepoPath,
    value: &TreeValue,
) -> Result<Vec<u8>, CommandError> {
    match value {
        TreeValue::File { id, .. } => {
            let mut file_reader = repo.store().read_file(path, id).unwrap();
            let mut content = vec![];
            file_reader.read_to_end(&mut content)?;
            Ok(content)
        }
        TreeValue::Symlink(id) => {
            let target = repo.store().read_symlink(path, id)?;
            Ok(target.into_bytes())
        }
        TreeValue::Tree(_) => {
            panic!(
                "Got an unexpected tree in a diff of path {}",
                path.to_internal_file_string()
            );
        }
        TreeValue::GitSubmodule(id) => {
            Ok(format!("Git submodule checked out at {}", id.hex()).into_bytes())
        }
        TreeValue::Conflict(id) => {
            let conflict = repo.store().read_conflict(path, id).unwrap();
            let mut content = vec![];
            conflicts::materialize_conflict(repo.store(), path, &conflict, &mut content).unwrap();
            Ok(content)
        }
    }
}

fn basic_diff_file_type(value: &TreeValue) -> String {
    match value {
        TreeValue::File { executable, .. } => {
            if *executable {
                "executable file".to_string()
            } else {
                "regular file".to_string()
            }
        }
        TreeValue::Symlink(_) => "symlink".to_string(),
        TreeValue::Tree(_) => "tree".to_string(),
        TreeValue::GitSubmodule(_) => "Git submodule".to_string(),
        TreeValue::Conflict(_) => "conflict".to_string(),
    }
}

pub fn show_color_words_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();
    formatter.add_label("diff")?;
    for (path, diff) in tree_diff {
        let ui_path = workspace_command.format_file_path(&path);
        match diff {
            tree::Diff::Added(right_value) => {
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = basic_diff_file_type(&right_value);
                formatter.with_label("header", |formatter| {
                    formatter.write_str(&format!("Added {} {}:\n", description, ui_path))
                })?;
                show_color_words_diff_hunks(&[], &right_content, formatter)?;
            }
            tree::Diff::Modified(left_value, right_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = match (left_value, right_value) {
                    (
                        TreeValue::File {
                            executable: left_executable,
                            ..
                        },
                        TreeValue::File {
                            executable: right_executable,
                            ..
                        },
                    ) => {
                        if left_executable && right_executable {
                            "Modified executable file".to_string()
                        } else if left_executable {
                            "Executable file became non-executable at".to_string()
                        } else if right_executable {
                            "Non-executable file became executable at".to_string()
                        } else {
                            "Modified regular file".to_string()
                        }
                    }
                    (TreeValue::Conflict(_), TreeValue::Conflict(_)) => {
                        "Modified conflict in".to_string()
                    }
                    (TreeValue::Conflict(_), _) => "Resolved conflict in".to_string(),
                    (_, TreeValue::Conflict(_)) => "Created conflict in".to_string(),
                    (TreeValue::Symlink(_), TreeValue::Symlink(_)) => {
                        "Symlink target changed at".to_string()
                    }
                    (left_value, right_value) => {
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
                formatter.with_label("header", |formatter| {
                    formatter.write_str(&format!("{} {}:\n", description, ui_path))
                })?;
                show_color_words_diff_hunks(&left_content, &right_content, formatter)?;
            }
            tree::Diff::Removed(left_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let description = basic_diff_file_type(&left_value);
                formatter.with_label("header", |formatter| {
                    formatter.write_str(&format!("Removed {} {}:\n", description, ui_path))
                })?;
                show_color_words_diff_hunks(&left_content, &[], formatter)?;
            }
        }
    }
    formatter.remove_label()?;
    Ok(())
}

struct GitDiffPart {
    mode: String,
    hash: String,
    content: Vec<u8>,
}

fn git_diff_part(
    repo: &Arc<ReadonlyRepo>,
    path: &RepoPath,
    value: &TreeValue,
) -> Result<GitDiffPart, CommandError> {
    let mode;
    let hash;
    let mut content = vec![];
    match value {
        TreeValue::File { id, executable } => {
            mode = if *executable {
                "100755".to_string()
            } else {
                "100644".to_string()
            };
            hash = id.hex();
            let mut file_reader = repo.store().read_file(path, id).unwrap();
            file_reader.read_to_end(&mut content)?;
        }
        TreeValue::Symlink(id) => {
            mode = "120000".to_string();
            hash = id.hex();
            let target = repo.store().read_symlink(path, id)?;
            content = target.into_bytes();
        }
        TreeValue::Tree(_) => {
            panic!(
                "Got an unexpected tree in a diff of path {}",
                path.to_internal_file_string()
            );
        }
        TreeValue::GitSubmodule(id) => {
            // TODO: What should we actually do here?
            mode = "040000".to_string();
            hash = id.hex();
        }
        TreeValue::Conflict(id) => {
            mode = "100644".to_string();
            hash = id.hex();
            let conflict = repo.store().read_conflict(path, id).unwrap();
            conflicts::materialize_conflict(repo.store(), path, &conflict, &mut content).unwrap();
        }
    }
    let hash = hash[0..10].to_string();
    Ok(GitDiffPart {
        mode,
        hash,
        content,
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
) -> Result<(), CommandError> {
    for hunk in unified_diff_hunks(left_content, right_content, 3) {
        formatter.with_label("hunk_header", |formatter| {
            writeln!(
                formatter,
                "@@ -{},{} +{},{} @@",
                hunk.left_line_range.start,
                hunk.left_line_range.len(),
                hunk.right_line_range.start,
                hunk.right_line_range.len()
            )
        })?;
        for (line_type, content) in hunk.lines {
            match line_type {
                DiffLineType::Context => {
                    formatter.with_label("context", |formatter| {
                        formatter.write_str(" ")?;
                        formatter.write_all(content)
                    })?;
                }
                DiffLineType::Removed => {
                    formatter.with_label("removed", |formatter| {
                        formatter.write_str("-")?;
                        formatter.write_all(content)
                    })?;
                }
                DiffLineType::Added => {
                    formatter.with_label("added", |formatter| {
                        formatter.write_str("+")?;
                        formatter.write_all(content)
                    })?;
                }
            }
            if !content.ends_with(b"\n") {
                formatter.write_str("\n\\ No newline at end of file\n")?;
            }
        }
    }
    Ok(())
}

pub fn show_git_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();
    formatter.add_label("diff")?;
    for (path, diff) in tree_diff {
        let path_string = path.to_internal_file_string();
        match diff {
            tree::Diff::Added(right_value) => {
                let right_part = git_diff_part(repo, &path, &right_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
                    writeln!(formatter, "new file mode {}", &right_part.mode)?;
                    writeln!(formatter, "index 0000000000..{}", &right_part.hash)?;
                    writeln!(formatter, "--- /dev/null")?;
                    writeln!(formatter, "+++ b/{}", path_string)
                })?;
                show_unified_diff_hunks(formatter, &[], &right_part.content)?;
            }
            tree::Diff::Modified(left_value, right_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                let right_part = git_diff_part(repo, &path, &right_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
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
                        writeln!(formatter, "--- a/{}", path_string)?;
                        writeln!(formatter, "+++ b/{}", path_string)?;
                    }
                    Ok(())
                })?;
                show_unified_diff_hunks(formatter, &left_part.content, &right_part.content)?;
            }
            tree::Diff::Removed(left_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
                    writeln!(formatter, "deleted file mode {}", &left_part.mode)?;
                    writeln!(formatter, "index {}..0000000000", &left_part.hash)?;
                    writeln!(formatter, "--- a/{}", path_string)?;
                    writeln!(formatter, "+++ /dev/null")
                })?;
                show_unified_diff_hunks(formatter, &left_part.content, &[])?;
            }
        }
    }
    formatter.remove_label()?;
    Ok(())
}

pub fn show_diff_summary(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
) -> io::Result<()> {
    formatter.with_label("diff", |formatter| {
        for (repo_path, diff) in tree_diff {
            match diff {
                tree::Diff::Modified(_, _) => {
                    formatter.with_label("modified", |formatter| {
                        writeln!(
                            formatter,
                            "M {}",
                            workspace_command.format_file_path(&repo_path)
                        )
                    })?;
                }
                tree::Diff::Added(_) => {
                    formatter.with_label("added", |formatter| {
                        writeln!(
                            formatter,
                            "A {}",
                            workspace_command.format_file_path(&repo_path)
                        )
                    })?;
                }
                tree::Diff::Removed(_) => {
                    formatter.with_label("removed", |formatter| {
                        writeln!(
                            formatter,
                            "R {}",
                            workspace_command.format_file_path(&repo_path)
                        )
                    })?;
                }
            }
        }
        Ok(())
    })
}
