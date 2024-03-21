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
use jj_lib::matchers::EverythingMatcher;
use jj_lib::repo::Repo;
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use super::resolve;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::diff_util;
use crate::ui::Ui;

/// Show high-level repo status
///
/// This includes:
///
///  * The working copy commit and its (first) parent, and a summary of the
///    changes between them
///
///  * Conflicted branches (see https://github.com/martinvonz/jj/blob/main/docs/branches.md)
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "st")]
pub(crate) struct StatusArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &StatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let maybe_wc_commit = workspace_command
        .get_wc_commit_id()
        .map(|id| repo.store().get_commit(id))
        .transpose()?;
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    if let Some(wc_commit) = &maybe_wc_commit {
        let parent_tree = merge_commit_trees(repo.as_ref(), &wc_commit.parents())?;
        let tree = wc_commit.tree()?;
        if tree.id() == parent_tree.id() {
            writeln!(formatter, "The working copy is clean")?;
        } else {
            writeln!(formatter, "Working copy changes:")?;
            diff_util::show_diff_summary(
                formatter,
                &workspace_command,
                parent_tree.diff_stream(&tree, &EverythingMatcher),
            )?;
        }

        let conflicts = wc_commit.tree()?.conflicts().collect_vec();
        if !conflicts.is_empty() {
            writeln!(
                formatter.labeled("conflict"),
                "There are unresolved conflicts at these paths:"
            )?;
            resolve::print_conflicted_paths(&conflicts, formatter, &workspace_command)?
        }

        let template = workspace_command.commit_summary_template();
        write!(formatter, "Working copy : ")?;
        formatter.with_label("working_copy", |fmt| template.format(wc_commit, fmt))?;
        writeln!(formatter)?;
        for parent in wc_commit.parents() {
            write!(formatter, "Parent commit: ")?;
            template.format(&parent, formatter)?;
            writeln!(formatter)?;
        }
    } else {
        writeln!(formatter, "No working copy")?;
    }

    let conflicted_local_branches = repo
        .view()
        .local_branches()
        .filter(|(_, target)| target.has_conflict())
        .map(|(branch_name, _)| branch_name)
        .collect_vec();
    let conflicted_remote_branches = repo
        .view()
        .all_remote_branches()
        .filter(|(_, remote_ref)| remote_ref.target.has_conflict())
        .map(|(full_name, _)| full_name)
        .collect_vec();
    if !conflicted_local_branches.is_empty() {
        writeln!(
            formatter.labeled("conflict"),
            "These branches have conflicts:"
        )?;
        for branch_name in conflicted_local_branches {
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "{branch_name}")?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj branch list` to see details. Use `jj branch set <name> -r <rev>` to \
             resolve."
        )?;
    }
    if !conflicted_remote_branches.is_empty() {
        writeln!(
            formatter.labeled("conflict"),
            "These remote branches have conflicts:"
        )?;
        for (branch_name, remote_name) in conflicted_remote_branches {
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "{branch_name}@{remote_name}")?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj branch list` to see details. Use `jj git fetch` to resolve."
        )?;
    }

    Ok(())
}
