// Copyright 2024 The Jujutsu Authors
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

use std::cmp;
use std::collections::HashMap;
use std::io::Read;
use std::ops::Range;
use std::rc::Rc;

use bstr::BString;
use clap_complete::ArgValueCandidates;
use futures::StreamExt as _;
use itertools::Itertools as _;
use jj_lib::annotate::get_annotation_with_file_content;
use jj_lib::backend::BackendError;
use jj_lib::backend::BackendResult;
use jj_lib::backend::CommitId;
use jj_lib::backend::FileId;
use jj_lib::backend::TreeValue;
use jj_lib::commit::Commit;
use jj_lib::conflicts::materialized_diff_stream;
use jj_lib::conflicts::MaterializedTreeValue;
use jj_lib::copies::CopyRecords;
use jj_lib::diff::Diff;
use jj_lib::diff::DiffHunkKind;
use jj_lib::matchers::Matcher;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree::MergedTreeBuilder;
use jj_lib::repo::MutableRepo;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathUiConverter;
use jj_lib::revset::ResolvedRevsetExpression;
use jj_lib::settings::UserSettings;
use pollster::FutureExt as _;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Move changes from a revision into the stack of mutable revisions
///
/// This command splits changes in the source revision and moves each change to
/// the closest mutable ancestor where the corresponding lines were modified
/// last. If the destination revision cannot be determined unambiguously, the
/// change will be left in the source revision.
///
/// The modification made by `jj absorb` can be reviewed by `jj op show -p`.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct AbsorbArgs {
    /// Source revision to absorb from
    #[arg(
        long, short,
        default_value = "@",
        add = ArgValueCandidates::new(complete::mutable_revisions),
    )]
    from: RevisionArg,
    /// Destination revisions to absorb into
    ///
    /// Only ancestors of the source revision will be considered.
    #[arg(
        long, short = 't', visible_alias = "to",
        default_value = "mutable()",
        add = ArgValueCandidates::new(complete::mutable_revisions),
    )]
    into: Vec<RevisionArg>,
    /// Move only changes to these paths (instead of all paths)
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_absorb(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AbsorbArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let source_commit = workspace_command.resolve_single_rev(ui, &args.from)?;
    let destinations = workspace_command
        .parse_union_revsets(ui, &args.into)?
        .resolve()?;

    let matcher = workspace_command
        .parse_file_patterns(ui, &args.paths)?
        .to_matcher();

    let repo = workspace_command.repo().as_ref();
    let source = AbsorbSource::from_commit(repo, source_commit)?;
    let selected_trees = split_hunks_to_trees(
        ui,
        repo,
        &source,
        &destinations,
        &matcher,
        workspace_command.path_converter(),
    )
    .block_on()?;
    workspace_command.check_rewritable(selected_trees.keys())?;

    let mut tx = workspace_command.start_transaction();
    let (rewritten_commits, num_rebased) =
        absorb_hunks(tx.repo_mut(), &source, selected_trees, command.settings())?;

    if let Some(mut formatter) = ui.status_formatter() {
        if !rewritten_commits.is_empty() {
            writeln!(formatter, "Absorbed changes into these revisions:")?;
            let template = tx.commit_summary_template();
            for commit in rewritten_commits.iter().rev() {
                write!(formatter, "  ")?;
                template.format(commit, formatter.as_mut())?;
                writeln!(formatter)?;
            }
        }
        if num_rebased > 0 {
            writeln!(formatter, "Rebased {num_rebased} descendant commits.")?;
        }
    }

    tx.finish(
        ui,
        format!("absorb changes into {} commits", rewritten_commits.len()),
    )?;
    Ok(())
}

#[derive(Clone, Debug)]
struct AbsorbSource {
    commit: Commit,
    parent_tree: MergedTree,
}

impl AbsorbSource {
    fn from_commit(repo: &dyn Repo, commit: Commit) -> BackendResult<Self> {
        let parent_tree = commit.parent_tree(repo)?;
        Ok(AbsorbSource {
            commit,
            parent_tree,
        })
    }
}

/// Builds trees to be merged into destination commits by splitting source
/// changes based on file annotation.
async fn split_hunks_to_trees(
    ui: &Ui,
    repo: &dyn Repo,
    source: &AbsorbSource,
    destinations: &Rc<ResolvedRevsetExpression>,
    matcher: &dyn Matcher,
    path_converter: &RepoPathUiConverter,
) -> Result<HashMap<CommitId, MergedTreeBuilder>, CommandError> {
    let mut selected_trees: HashMap<CommitId, MergedTreeBuilder> = HashMap::new();

    let left_tree = &source.parent_tree;
    let right_tree = source.commit.tree()?;
    // TODO: enable copy tracking if we add support for annotate and merge
    let copy_records = CopyRecords::default();
    let tree_diff = left_tree.diff_stream_with_copies(&right_tree, matcher, &copy_records);
    let mut diff_stream = materialized_diff_stream(repo.store(), tree_diff);
    while let Some(entry) = diff_stream.next().await {
        let left_path = entry.path.source();
        let right_path = entry.path.target();
        let (left_value, right_value) = entry.values?;
        let (left_text, executable) = match to_file_value(left_value) {
            Ok(Some(mut value)) => (value.read(left_path)?, value.executable),
            Ok(None) => continue,
            Err(reason) => {
                let ui_path = path_converter.format_file_path(left_path);
                writeln!(ui.warning_default(), "Skipping {ui_path}: {reason}")?;
                continue;
            }
        };
        let right_text = match to_file_value(right_value) {
            Ok(Some(mut value)) => value.read(right_path)?,
            Ok(None) => continue,
            Err(reason) => {
                let ui_path = path_converter.format_file_path(right_path);
                writeln!(ui.warning_default(), "Skipping {ui_path}: {reason}")?;
                continue;
            }
        };

        // Compute annotation of parent (= left) content to map right hunks
        let annotation = get_annotation_with_file_content(
            repo,
            source.commit.id(),
            destinations,
            left_path,
            left_text.clone(),
        )?;
        let annotation_ranges = annotation
            .compact_line_ranges()
            .filter_map(|(commit_id, range)| Some((commit_id?, range)))
            .collect_vec();
        let diff = Diff::by_line([&left_text, &right_text]);
        let selected_ranges = split_file_hunks(&annotation_ranges, &diff);
        // Build trees containing parent (= left) contents + selected hunks
        for (&commit_id, ranges) in &selected_ranges {
            let tree_builder = selected_trees
                .entry(commit_id.clone())
                .or_insert_with(|| MergedTreeBuilder::new(left_tree.id().clone()));
            let new_text = combine_texts(&left_text, &right_text, ranges);
            let id = repo
                .store()
                .write_file(left_path, &mut new_text.as_slice())
                .await?;
            tree_builder.set_or_remove(
                left_path.to_owned(),
                Merge::normal(TreeValue::File { id, executable }),
            );
        }
    }

    Ok(selected_trees)
}

type SelectedRange = (Range<usize>, Range<usize>);

/// Maps `diff` hunks to commits based on the left `annotation_ranges`. The
/// `annotation_ranges` should be compacted.
fn split_file_hunks<'a>(
    mut annotation_ranges: &[(&'a CommitId, Range<usize>)],
    diff: &Diff,
) -> HashMap<&'a CommitId, Vec<SelectedRange>> {
    debug_assert!(annotation_ranges.iter().all(|(_, range)| !range.is_empty()));
    let mut selected_ranges: HashMap<&CommitId, Vec<_>> = HashMap::new();
    let mut diff_hunk_ranges = diff
        .hunk_ranges()
        .filter(|hunk| hunk.kind == DiffHunkKind::Different);
    while !annotation_ranges.is_empty() {
        let Some(hunk) = diff_hunk_ranges.next() else {
            break;
        };
        let [left_range, right_range]: &[_; 2] = hunk.ranges[..].try_into().unwrap();
        assert!(!left_range.is_empty() || !right_range.is_empty());
        if right_range.is_empty() {
            // If the hunk is pure deletion, it can be mapped to multiple
            // overlapped annotation ranges unambiguously.
            let skip = annotation_ranges
                .iter()
                .take_while(|(_, range)| range.end <= left_range.start)
                .count();
            annotation_ranges = &annotation_ranges[skip..];
            let pre_overlap = annotation_ranges
                .iter()
                .take_while(|(_, range)| range.end < left_range.end)
                .count();
            let maybe_overlapped_ranges = annotation_ranges.get(..pre_overlap + 1);
            annotation_ranges = &annotation_ranges[pre_overlap..];
            let Some(overlapped_ranges) = maybe_overlapped_ranges else {
                continue;
            };
            // Ensure that the ranges are contiguous and include the start.
            let all_covered = overlapped_ranges
                .iter()
                .try_fold(left_range.start, |prev_end, (_, cur)| {
                    (cur.start <= prev_end).then_some(cur.end)
                })
                .inspect(|&last_end| assert!(left_range.end <= last_end))
                .is_some();
            if all_covered {
                for (commit_id, cur_range) in overlapped_ranges {
                    let start = cmp::max(cur_range.start, left_range.start);
                    let end = cmp::min(cur_range.end, left_range.end);
                    assert!(start < end);
                    let selected = selected_ranges.entry(commit_id).or_default();
                    selected.push((start..end, right_range.clone()));
                }
            }
        } else {
            // In other cases, the hunk should be included in an annotation
            // range to map it unambiguously. Skip any pre-overlapped ranges.
            let skip = annotation_ranges
                .iter()
                .take_while(|(_, range)| range.end < left_range.end)
                .count();
            annotation_ranges = &annotation_ranges[skip..];
            let Some((commit_id, cur_range)) = annotation_ranges.first() else {
                continue;
            };
            let contained = cur_range.start <= left_range.start && left_range.end <= cur_range.end;
            // If the hunk is pure insertion, it can be mapped to two distinct
            // annotation ranges, which is ambiguous.
            let ambiguous = cur_range.end == left_range.start
                && annotation_ranges
                    .get(1)
                    .is_some_and(|(_, next_range)| next_range.start == left_range.end);
            if contained && !ambiguous {
                let selected = selected_ranges.entry(commit_id).or_default();
                selected.push((left_range.clone(), right_range.clone()));
            }
        }
    }
    selected_ranges
}

/// Constructs new text by replacing `text1` range with `text2` range for each
/// selected `(range1, range2)` pairs.
fn combine_texts(text1: &[u8], text2: &[u8], selected_ranges: &[SelectedRange]) -> BString {
    itertools::chain!(
        [(0..0, 0..0)],
        selected_ranges.iter().cloned(),
        [(text1.len()..text1.len(), text2.len()..text2.len())],
    )
    .tuple_windows()
    // Copy unchanged hunk from text1 and current hunk from text2
    .map(|((prev1, _), (cur1, cur2))| (prev1.end..cur1.start, cur2))
    .flat_map(|(range1, range2)| [&text1[range1], &text2[range2]])
    .collect()
}

/// Merges selected trees into the specified commits.
fn absorb_hunks(
    repo: &mut MutableRepo,
    source: &AbsorbSource,
    mut selected_trees: HashMap<CommitId, MergedTreeBuilder>,
    settings: &UserSettings,
) -> BackendResult<(Vec<Commit>, usize)> {
    let store = repo.store().clone();
    let mut rewritten_commits = Vec::new();
    let mut num_rebased = 0;
    // Rewrite commits in topological order so that descendant commits wouldn't
    // be rewritten multiple times.
    repo.transform_descendants(
        settings,
        selected_trees.keys().cloned().collect(),
        |rewriter| {
            // Remove selected hunks from the source commit by reparent()
            if rewriter.old_commit().id() == source.commit.id() {
                // TODO: should we abandon the source if it's discardable?
                rewriter.reparent(settings)?.write()?;
                num_rebased += 1;
                return Ok(());
            }
            let Some(tree_builder) = selected_trees.remove(rewriter.old_commit().id()) else {
                rewriter.rebase(settings)?.write()?;
                num_rebased += 1;
                return Ok(());
            };
            // Merge hunks between source parent tree and selected tree
            let selected_tree_id = tree_builder.write_tree(&store)?;
            let commit_builder = rewriter.rebase(settings)?;
            let destination_tree = store.get_root_tree(commit_builder.tree_id())?;
            let selected_tree = store.get_root_tree(&selected_tree_id)?;
            let new_tree = destination_tree.merge(&source.parent_tree, &selected_tree)?;
            let mut predecessors = commit_builder.predecessors().to_vec();
            predecessors.push(source.commit.id().clone());
            let new_commit = commit_builder
                .set_tree_id(new_tree.id())
                .set_predecessors(predecessors)
                .write()?;
            rewritten_commits.push(new_commit);
            Ok(())
        },
    )?;
    Ok((rewritten_commits, num_rebased))
}

struct FileValue {
    id: FileId,
    executable: bool,
    reader: Box<dyn Read>,
}

impl FileValue {
    fn read(&mut self, path: &RepoPath) -> BackendResult<BString> {
        let mut buf = Vec::new();
        self.reader
            .read_to_end(&mut buf)
            .map_err(|err| BackendError::ReadFile {
                path: path.to_owned(),
                id: self.id.clone(),
                source: err.into(),
            })?;
        Ok(buf.into())
    }
}

fn to_file_value(value: MaterializedTreeValue) -> Result<Option<FileValue>, String> {
    match value {
        MaterializedTreeValue::Absent => Ok(None), // New or deleted file
        MaterializedTreeValue::AccessDenied(err) => Err(format!("Access is denied: {err}")),
        MaterializedTreeValue::File {
            id,
            executable,
            reader,
        } => Ok(Some(FileValue {
            id,
            executable,
            reader,
        })),
        MaterializedTreeValue::Symlink { .. } => Err("Is a symlink".into()),
        MaterializedTreeValue::FileConflict { .. }
        | MaterializedTreeValue::OtherConflict { .. } => Err("Is a conflict".into()),
        MaterializedTreeValue::GitSubmodule(_) => Err("Is a Git submodule".into()),
        MaterializedTreeValue::Tree(_) => panic!("diff should not contain trees"),
    }
}

#[cfg(test)]
mod tests {
    use maplit::hashmap;

    use super::*;

    #[test]
    fn test_split_file_hunks_empty_or_single_line() {
        let commit_id1 = &CommitId::from_hex("111111");

        // unchanged
        assert_eq!(split_file_hunks(&[], &Diff::by_line(["", ""])), hashmap! {});

        // insert single line
        assert_eq!(
            split_file_hunks(&[], &Diff::by_line(["", "2X\n"])),
            hashmap! {}
        );
        // delete single line
        assert_eq!(
            split_file_hunks(&[(commit_id1, 0..3)], &Diff::by_line(["1a\n", ""])),
            hashmap! { commit_id1 => vec![(0..3, 0..0)] }
        );
        // modify single line
        assert_eq!(
            split_file_hunks(&[(commit_id1, 0..3)], &Diff::by_line(["1a\n", "1AA\n"])),
            hashmap! { commit_id1 => vec![(0..3, 0..4)] }
        );
    }

    #[test]
    fn test_split_file_hunks_single_range() {
        let commit_id1 = &CommitId::from_hex("111111");

        // insert first, middle, and last lines
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6)],
                &Diff::by_line(["1a\n1b\n", "1X\n1a\n1Y\n1b\n1Z\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..0, 0..3), (3..3, 6..9), (6..6, 12..15)],
            }
        );
        // delete first, middle, and last lines
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..15)],
                &Diff::by_line(["1a\n1b\n1c\n1d\n1e\n1f\n", "1b\n1d\n1f\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..3, 0..0), (6..9, 3..3), (12..15, 6..6)],
            }
        );
        // modify non-contiguous lines
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..12)],
                &Diff::by_line(["1a\n1b\n1c\n1d\n", "1A\n1b\n1C\n1d\n"])
            ),
            hashmap! { commit_id1 => vec![(0..3, 0..3), (6..9, 6..9)] }
        );
    }

    #[test]
    fn test_split_file_hunks_contiguous_ranges_insert() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // insert first line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1X\n1a\n1b\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(0..0, 0..3)] }
        );
        // insert middle line to first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1X\n1b\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(3..3, 3..6)] }
        );
        // insert middle line between ranges (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n3X\n2a\n2b\n"])
            ),
            hashmap! {}
        );
        // insert middle line to second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2a\n2X\n2b\n"])
            ),
            hashmap! { commit_id2 => vec![(9..9, 9..12)] }
        );
        // insert last line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2a\n2b\n2X\n"])
            ),
            hashmap! { commit_id2 => vec![(12..12, 12..15)] }
        );
    }

    #[test]
    fn test_split_file_hunks_contiguous_ranges_delete() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // delete first line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1b\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(0..3, 0..0)] }
        );
        // delete middle line from first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(3..6, 3..3)] }
        );
        // delete middle line from second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2b\n"])
            ),
            hashmap! { commit_id2 => vec![(6..9, 6..6)] }
        );
        // delete last line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2a\n"])
            ),
            hashmap! { commit_id2 => vec![(9..12, 9..9)] }
        );
        // delete first and last lines
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1b\n2a\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..3, 0..0)],
                commit_id2 => vec![(9..12, 6..6)],
            }
        );

        // delete across ranges (split first annotation range)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n"])
            ),
            hashmap! {
                commit_id1 => vec![(3..6, 3..3)],
                commit_id2 => vec![(6..12, 3..3)],
            }
        );
        // delete middle lines across ranges (split both annotation ranges)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n2b\n"])
            ),
            hashmap! {
                commit_id1 => vec![(3..6, 3..3)],
                commit_id2 => vec![(6..9, 3..3)],
            }
        );
        // delete across ranges (split second annotation range)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "2b\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..6, 0..0)],
                commit_id2 => vec![(6..9, 0..0)],
            }
        );

        // delete all
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", ""])
            ),
            hashmap! {
                commit_id1 => vec![(0..6, 0..0)],
                commit_id2 => vec![(6..12, 0..0)],
            }
        );
    }

    #[test]
    fn test_split_file_hunks_contiguous_ranges_modify() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // modify first line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n1b\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(0..3, 0..3)] }
        );
        // modify middle line of first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1B\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(3..6, 3..6)] }
        );
        // modify middle lines of both ranges (ambiguous)
        // ('hg absorb' accepts this)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1B\n2A\n2b\n"])
            ),
            hashmap! {}
        );
        // modify middle line of second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2A\n2b\n"])
            ),
            hashmap! { commit_id2 => vec![(6..9, 6..9)] }
        );
        // modify last line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2a\n2B\n"])
            ),
            hashmap! { commit_id2 => vec![(9..12, 9..12)] }
        );
        // modify first and last lines
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n1b\n2a\n2B\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..3, 0..3)],
                commit_id2 => vec![(9..12, 9..12)],
            }
        );
    }

    #[test]
    fn test_split_file_hunks_contiguous_ranges_modify_insert() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // modify first range, insert adjacent middle line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n1B\n1X\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(0..6, 0..9)] }
        );
        // modify second range, insert adjacent middle line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2X\n2A\n2B\n"])
            ),
            hashmap! { commit_id2 => vec![(6..12, 6..15)] }
        );
        // modify second range, insert last line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2A\n2B\n2X\n"])
            ),
            hashmap! { commit_id2 => vec![(6..12, 6..15)] }
        );
        // modify first and last lines (unambiguous), insert middle line between
        // ranges (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n1b\n3X\n2a\n2B\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..3, 0..3)],
                commit_id2 => vec![(9..12, 12..15)],
            }
        );
    }

    #[test]
    fn test_split_file_hunks_contiguous_ranges_modify_delete() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // modify first line, delete adjacent middle line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(0..6, 0..3)] }
        );
        // modify last line, delete adjacent middle line
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1b\n2B\n"])
            ),
            hashmap! { commit_id2 => vec![(6..12, 6..9)] }
        );
        // modify first and last lines, delete middle line from first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n2a\n2B\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..6, 0..3)],
                commit_id2 => vec![(9..12, 6..9)],
            }
        );
        // modify first and last lines, delete middle line from second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1A\n1b\n2B\n"])
            ),
            hashmap! {
                commit_id1 => vec![(0..3, 0..3)],
                commit_id2 => vec![(6..12, 6..9)],
            }
        );
        // modify middle line, delete adjacent middle line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), (commit_id2, 6..12)],
                &Diff::by_line(["1a\n1b\n2a\n2b\n", "1a\n1B\n2b\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_insert() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // insert middle line to first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n1X\n0a\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(6..6, 6..9)] }
        );
        // insert middle line to second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n0a\n2X\n2a\n2b\n"])
            ),
            hashmap! { commit_id2 => vec![(9..9, 9..12)] }
        );
        // insert middle lines to both ranges
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n1X\n0a\n2X\n2a\n2b\n"])
            ),
            hashmap! {
                commit_id1 => vec![(6..6, 6..9)],
                commit_id2 => vec![(9..9, 12..15)],
            }
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_insert_modify_masked() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // insert middle line to first range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n1X\n0A\n2a\n2b\n"])
            ),
            hashmap! {}
        );
        // insert middle line to second range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n0A\n2X\n2a\n2b\n"])
            ),
            hashmap! {}
        );
        // insert middle lines to both ranges, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n1X\n0A\n2X\n2a\n2b\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_delete() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // delete middle line from first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n0a\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(3..6, 3..3)] }
        );
        // delete middle line from second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n0a\n2b\n"])
            ),
            hashmap! { commit_id2 => vec![(9..12, 9..9)] }
        );
        // delete middle lines from both ranges
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n0a\n2b\n"])
            ),
            hashmap! {
                commit_id1 => vec![(3..6, 3..3)],
                commit_id2 => vec![(9..12, 6..6)],
            }
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_delete_modify_masked() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // delete middle line from first range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n0A\n2a\n2b\n"])
            ),
            hashmap! {}
        );
        // delete middle line from second range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n0A\n2b\n"])
            ),
            hashmap! {}
        );
        // delete middle lines from both ranges, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n0A\n2b\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_delete_delete_masked() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // 'hg absorb' accepts these, but it seems better to reject them as
        // ambiguous. Masked lines cannot be deleted.

        // delete middle line from first range, delete masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n2a\n2b\n"])
            ),
            hashmap! {}
        );
        // delete middle line from second range, delete masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n2b\n"])
            ),
            hashmap! {}
        );
        // delete middle lines from both ranges, delete masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n2b\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_modify() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // modify middle line of first range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1B\n0a\n2a\n2b\n"])
            ),
            hashmap! { commit_id1 => vec![(3..6, 3..6)] }
        );
        // modify middle line of second range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n0a\n2A\n2b\n"])
            ),
            hashmap! { commit_id2 => vec![(9..12, 9..12)] }
        );
        // modify middle lines of both ranges
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1B\n0a\n2A\n2b\n"])
            ),
            hashmap! {
                commit_id1 => vec![(3..6, 3..6)],
                commit_id2 => vec![(9..12, 9..12)],
            }
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_ranges_modify_modify_masked() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");

        // modify middle line of first range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1B\n0A\n2a\n2b\n"])
            ),
            hashmap! {}
        );
        // modify middle line of second range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1b\n0A\n2A\n2b\n"])
            ),
            hashmap! {}
        );
        // modify middle lines to both ranges, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6), /* 6..9, */ (commit_id2, 9..15)],
                &Diff::by_line(["1a\n1b\n0a\n2a\n2b\n", "1a\n1B\n0A\n2A\n2b\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_insert() {
        let commit_id1 = &CommitId::from_hex("111111");

        // insert middle line to range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n1b\n1X\n0a\n"])
            ),
            hashmap! { commit_id1 => vec![(6..6, 6..9)] }
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_insert_modify_masked() {
        let commit_id1 = &CommitId::from_hex("111111");

        // insert middle line to range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n1b\n1X\n0A\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_delete() {
        let commit_id1 = &CommitId::from_hex("111111");

        // delete middle line from range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n0a\n"])
            ),
            hashmap! { commit_id1 => vec![(3..6, 3..3)] }
        );
        // delete all lines from range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "0a\n"])
            ),
            hashmap! { commit_id1 => vec![(0..6, 0..0)] }
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_delete_modify_masked() {
        let commit_id1 = &CommitId::from_hex("111111");

        // delete middle line from range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n0A\n"])
            ),
            hashmap! {}
        );
        // delete all lines from range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "0A\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_delete_delete_masked() {
        let commit_id1 = &CommitId::from_hex("111111");

        // 'hg absorb' accepts these, but it seems better to reject them as
        // ambiguous. Masked lines cannot be deleted.

        // delete middle line from range, delete masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n"])
            ),
            hashmap! {}
        );
        // delete all lines from range, delete masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", ""])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_modify() {
        let commit_id1 = &CommitId::from_hex("111111");

        // modify middle line of range
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n1B\n0a\n"])
            ),
            hashmap! { commit_id1 => vec![(3..6, 3..6)] }
        );
    }

    #[test]
    fn test_split_file_hunks_non_contiguous_tail_range_modify_modify_masked() {
        let commit_id1 = &CommitId::from_hex("111111");

        // modify middle line of range, modify masked line (ambiguous)
        assert_eq!(
            split_file_hunks(
                &[(commit_id1, 0..6) /* , 6..9 */],
                &Diff::by_line(["1a\n1b\n0a\n", "1a\n1B\n0A\n"])
            ),
            hashmap! {}
        );
    }

    #[test]
    fn test_split_file_hunks_multiple_edits() {
        let commit_id1 = &CommitId::from_hex("111111");
        let commit_id2 = &CommitId::from_hex("222222");
        let commit_id3 = &CommitId::from_hex("333333");

        assert_eq!(
            split_file_hunks(
                &[
                    (commit_id1, 0..3),   // 1a       => 1A
                    (commit_id2, 3..6),   // 2a       => 2a
                    (commit_id1, 6..15),  // 1b 1c 1d => 1B 1d
                    (commit_id3, 15..21), // 3a 3b    => 3X 3A 3b 3Y
                ],
                &Diff::by_line([
                    "1a\n2a\n1b\n1c\n1d\n3a\n3b\n",
                    "1A\n2a\n1B\n1d\n3X\n3A\n3b\n3Y\n"
                ])
            ),
            hashmap! {
                commit_id1 => vec![(0..3, 0..3), (6..12, 6..9)],
                commit_id3 => vec![(15..18, 12..18), (21..21, 21..24)],
            }
        );
    }

    #[test]
    fn test_combine_texts() {
        assert_eq!(combine_texts(b"", b"", &[]), "");
        assert_eq!(combine_texts(b"foo", b"bar", &[]), "foo");
        assert_eq!(combine_texts(b"foo", b"bar", &[(0..3, 0..3)]), "bar");

        assert_eq!(
            combine_texts(
                b"1a\n2a\n1b\n1c\n1d\n3a\n3b\n",
                b"1A\n2a\n1B\n1d\n3X\n3A\n3b\n3Y\n",
                &[(0..3, 0..3), (6..12, 6..9)]
            ),
            "1A\n2a\n1B\n1d\n3a\n3b\n"
        );
        assert_eq!(
            combine_texts(
                b"1a\n2a\n1b\n1c\n1d\n3a\n3b\n",
                b"1A\n2a\n1B\n1d\n3X\n3A\n3b\n3Y\n",
                &[(15..18, 12..18), (21..21, 21..24)]
            ),
            "1a\n2a\n1b\n1c\n1d\n3X\n3A\n3b\n3Y\n"
        );
    }
}
