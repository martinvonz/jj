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

use itertools::Itertools;
use jj_lib::copies::CopyRecords;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetFilterPredicate;
use tracing::instrument;

use crate::cli_util::print_conflicted_paths;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::diff_util::get_copy_records;
use crate::diff_util::DiffFormat;
use crate::ui::Ui;

/// Show high-level repo status
///
/// This includes:
///
///  * The working copy commit and its (first) parent, and a summary of the
///    changes between them
///  * Conflicted bookmarks (see https://martinvonz.github.io/jj/latest/bookmarks/)
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "st")]
pub(crate) struct StatusArgs {
    /// Restrict the status display to these paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &StatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let maybe_wc_commit = workspace_command
        .get_wc_commit_id()
        .map(|id| repo.store().get_commit(id))
        .transpose()?;
    let matcher = workspace_command
        .parse_file_patterns(&args.paths)?
        .to_matcher();
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    if let Some(wc_commit) = &maybe_wc_commit {
        let parent_tree = wc_commit.parent_tree(repo.as_ref())?;
        let tree = wc_commit.tree()?;
        if tree.id() == parent_tree.id() {
            writeln!(formatter, "The working copy is clean")?;
        } else {
            writeln!(formatter, "Working copy changes:")?;
            let mut copy_records = CopyRecords::default();
            for parent in wc_commit.parent_ids() {
                let records = get_copy_records(repo.store(), parent, wc_commit.id(), &matcher)?;
                copy_records.add_records(records)?;
            }
            let diff_renderer = workspace_command.diff_renderer(vec![DiffFormat::Summary]);
            let width = ui.term_width();
            diff_renderer.show_diff(
                ui,
                formatter,
                &parent_tree,
                &tree,
                &matcher,
                &copy_records,
                width,
            )?;
        }

        // TODO: Conflicts should also be filtered by the `matcher`. See the related
        // TODO on `MergedTree::conflicts()`.
        let conflicts = wc_commit.tree()?.conflicts().collect_vec();
        if !conflicts.is_empty() {
            writeln!(
                formatter.labeled("conflict"),
                "There are unresolved conflicts at these paths:"
            )?;
            print_conflicted_paths(&conflicts, formatter, &workspace_command)?
        }

        let template = workspace_command.commit_summary_template();
        write!(formatter, "Working copy : ")?;
        formatter.with_label("working_copy", |fmt| template.format(wc_commit, fmt))?;
        writeln!(formatter)?;
        for parent in wc_commit.parents() {
            let parent = parent?;
            write!(formatter, "Parent commit: ")?;
            template.format(&parent, formatter)?;
            writeln!(formatter)?;
        }

        if wc_commit.has_conflict()? {
            let wc_revset = RevsetExpression::commit(wc_commit.id().clone());

            // Ancestors with conflicts, excluding the current working copy commit.
            let ancestors_conflicts = workspace_command
                .attach_revset_evaluator(
                    wc_revset
                        .parents()
                        .ancestors()
                        .filtered(RevsetFilterPredicate::HasConflict)
                        .minus(&workspace_command.env().immutable_expression()),
                )
                .evaluate_to_commit_ids()?
                .collect();

            workspace_command.report_repo_conflicts(formatter, repo, ancestors_conflicts)?;
        } else {
            for parent in wc_commit.parents() {
                let parent = parent?;
                if parent.has_conflict()? {
                    writeln!(
                        formatter.labeled("hint"),
                        "Conflict in parent commit has been resolved in working copy"
                    )?;
                    break;
                }
            }
        }
    } else {
        writeln!(formatter, "No working copy")?;
    }

    let conflicted_local_bookmarks = repo
        .view()
        .local_bookmarks()
        .filter(|(_, target)| target.has_conflict())
        .map(|(bookmark_name, _)| bookmark_name)
        .collect_vec();
    let conflicted_remote_bookmarks = repo
        .view()
        .all_remote_bookmarks()
        .filter(|(_, remote_ref)| remote_ref.target.has_conflict())
        .map(|(full_name, _)| full_name)
        .collect_vec();
    if !conflicted_local_bookmarks.is_empty() {
        writeln!(
            formatter.labeled("conflict"),
            "These bookmarks have conflicts:"
        )?;
        for bookmark_name in conflicted_local_bookmarks {
            write!(formatter, "  ")?;
            write!(formatter.labeled("bookmark"), "{bookmark_name}")?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj bookmark list` to see details. Use `jj bookmark set <name> -r <rev>` to \
             resolve."
        )?;
    }
    if !conflicted_remote_bookmarks.is_empty() {
        writeln!(
            formatter.labeled("conflict"),
            "These remote bookmarks have conflicts:"
        )?;
        for (bookmark_name, remote_name) in conflicted_remote_bookmarks {
            write!(formatter, "  ")?;
            write!(
                formatter.labeled("bookmark"),
                "{bookmark_name}@{remote_name}"
            )?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj bookmark list` to see details. Use `jj git fetch` to resolve."
        )?;
    }

    Ok(())
}
