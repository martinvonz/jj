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

use std::io::Write;
use std::rc::Rc;

use clap_complete::ArgValueCandidates;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::CommitIteratorExt;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::revset::ResolvedRevsetExpression;
use jj_lib::revset::RevsetExpression;
use jj_lib::rewrite::duplicate_commits;
use jj_lib::rewrite::duplicate_commits_onto_parents;
use jj_lib::rewrite::DuplicateCommitsStats;
use tracing::instrument;

use crate::cli_util::short_commit_hash;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Create new changes with the same content as existing ones
///
/// When none of the `--destination`, `--insert-after`, or `--insert-before`
/// arguments are provided, commits will be duplicated onto their existing
/// parents or onto other newly duplicated commits.
///
/// When any of the `--destination`, `--insert-after`, or `--insert-before`
/// arguments are provided, the roots of the specified commits will be
/// duplicated onto the destination indicated by the arguments. Other specified
/// commits will be duplicated onto these newly duplicated commits. If the
/// `--insert-after` or `--insert-before` arguments are provided, the new
/// children indicated by the arguments will be rebased onto the heads of the
/// specified commits.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases = &["dup"])]
pub(crate) struct DuplicateArgs {
    /// The revision(s) to duplicate (default: @)
    #[arg(
        value_name = "REVSETS",
        add = ArgValueCandidates::new(complete::all_revisions)
    )]
    revisions_pos: Vec<RevisionArg>,
    #[arg(short = 'r', hide = true, value_name = "REVSETS")]
    revisions_opt: Vec<RevisionArg>,
    /// The revision(s) to duplicate onto (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short,
        value_name = "REVSETS",
        add = ArgValueCandidates::new(complete::all_revisions)
    )]
    destination: Vec<RevisionArg>,
    /// The revision(s) to insert after (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short = 'A',
        visible_alias = "after",
        conflicts_with = "destination",
        value_name = "REVSETS",
        add = ArgValueCandidates::new(complete::all_revisions),
    )]
    insert_after: Vec<RevisionArg>,
    /// The revision(s) to insert before (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short = 'B',
        visible_alias = "before",
        conflicts_with = "destination",
        value_name = "REVSETS",
        add = ArgValueCandidates::new(complete::mutable_revisions)
    )]
    insert_before: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DuplicateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_duplicate: Vec<CommitId> =
        if !args.revisions_pos.is_empty() || !args.revisions_opt.is_empty() {
            workspace_command
                .parse_union_revsets(ui, &[&*args.revisions_pos, &*args.revisions_opt].concat())?
        } else {
            workspace_command.parse_revset(ui, &RevisionArg::AT)?
        }
        .evaluate_to_commit_ids()?
        .try_collect()?; // in reverse topological order
    if to_duplicate.is_empty() {
        writeln!(ui.status(), "No revisions to duplicate.")?;
        return Ok(());
    }
    if to_duplicate.last() == Some(workspace_command.repo().store().root_commit_id()) {
        return Err(user_error("Cannot duplicate the root commit"));
    }

    let parent_commit_ids: Vec<CommitId>;
    let children_commit_ids: Vec<CommitId>;

    if !args.insert_before.is_empty() && !args.insert_after.is_empty() {
        let parent_commits = workspace_command
            .resolve_some_revsets_default_single(ui, &args.insert_after)?
            .into_iter()
            .collect_vec();
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        let children_commits = workspace_command
            .resolve_some_revsets_default_single(ui, &args.insert_before)?
            .into_iter()
            .collect_vec();
        children_commit_ids = children_commits.iter().ids().cloned().collect();
        workspace_command.check_rewritable(&children_commit_ids)?;
        let children_expression = RevsetExpression::commits(children_commit_ids.clone());
        let parents_expression = RevsetExpression::commits(parent_commit_ids.clone());
        ensure_no_commit_loop(
            workspace_command.repo(),
            &children_expression,
            &parents_expression,
        )?;
    } else if !args.insert_before.is_empty() {
        let children_commits = workspace_command
            .resolve_some_revsets_default_single(ui, &args.insert_before)?
            .into_iter()
            .collect_vec();
        children_commit_ids = children_commits.iter().ids().cloned().collect();
        workspace_command.check_rewritable(&children_commit_ids)?;
        let children_expression = RevsetExpression::commits(children_commit_ids.clone());
        let parents_expression = children_expression.parents();
        ensure_no_commit_loop(
            workspace_command.repo(),
            &children_expression,
            &parents_expression,
        )?;
        // Manually collect the parent commit IDs to preserve the order of parents.
        parent_commit_ids = children_commits
            .iter()
            .flat_map(|commit| commit.parent_ids())
            .unique()
            .cloned()
            .collect_vec();
    } else if !args.insert_after.is_empty() {
        let parent_commits = workspace_command
            .resolve_some_revsets_default_single(ui, &args.insert_after)?
            .into_iter()
            .collect_vec();
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        let parents_expression = RevsetExpression::commits(parent_commit_ids.clone());
        let children_expression = parents_expression.children();
        children_commit_ids = children_expression
            .clone()
            .evaluate(workspace_command.repo().as_ref())
            .map_err(|err| err.expect_backend_error())?
            .iter()
            .try_collect()?;
        workspace_command.check_rewritable(&children_commit_ids)?;
        ensure_no_commit_loop(
            workspace_command.repo(),
            &children_expression,
            &parents_expression,
        )?;
    } else if !args.destination.is_empty() {
        let parent_commits = workspace_command
            .resolve_some_revsets_default_single(ui, &args.destination)?
            .into_iter()
            .collect_vec();
        parent_commit_ids = parent_commits.iter().ids().cloned().collect();
        children_commit_ids = vec![];
    } else {
        parent_commit_ids = vec![];
        children_commit_ids = vec![];
    };

    let mut tx = workspace_command.start_transaction();

    if !parent_commit_ids.is_empty() {
        for commit_id in &to_duplicate {
            for parent_commit_id in &parent_commit_ids {
                if tx.repo().index().is_ancestor(commit_id, parent_commit_id) {
                    writeln!(
                        ui.warning_default(),
                        "Duplicating commit {} as a descendant of itself",
                        short_commit_hash(commit_id)
                    )?;
                    break;
                }
            }
        }

        for commit_id in &to_duplicate {
            for child_commit_id in &children_commit_ids {
                if tx.repo().index().is_ancestor(child_commit_id, commit_id) {
                    writeln!(
                        ui.warning_default(),
                        "Duplicating commit {} as an ancestor of itself",
                        short_commit_hash(commit_id)
                    )?;
                    break;
                }
            }
        }
    }

    let num_to_duplicate = to_duplicate.len();
    let DuplicateCommitsStats {
        duplicated_commits,
        num_rebased,
    } = if args.destination.is_empty()
        && args.insert_after.is_empty()
        && args.insert_before.is_empty()
    {
        duplicate_commits_onto_parents(command.settings(), tx.repo_mut(), &to_duplicate)?
    } else {
        duplicate_commits(
            command.settings(),
            tx.repo_mut(),
            &to_duplicate,
            &parent_commit_ids,
            &children_commit_ids,
        )?
    };

    if let Some(mut formatter) = ui.status_formatter() {
        for (old_id, new_commit) in &duplicated_commits {
            write!(formatter, "Duplicated {} as ", short_commit_hash(old_id))?;
            tx.write_commit_summary(formatter.as_mut(), new_commit)?;
            writeln!(formatter)?;
        }
        if num_rebased > 0 {
            writeln!(
                ui.status(),
                "Rebased {num_rebased} commits onto duplicated commits"
            )?;
        }
    }
    tx.finish(ui, format!("duplicate {num_to_duplicate} commit(s)"))?;
    Ok(())
}

/// Ensure that there is no possible cycle between the potential children and
/// parents of the duplicated commits.
fn ensure_no_commit_loop(
    repo: &ReadonlyRepo,
    children_expression: &Rc<ResolvedRevsetExpression>,
    parents_expression: &Rc<ResolvedRevsetExpression>,
) -> Result<(), CommandError> {
    if let Some(commit_id) = children_expression
        .dag_range_to(parents_expression)
        .evaluate(repo)?
        .iter()
        .next()
    {
        let commit_id = commit_id?;
        return Err(user_error(format!(
            "Refusing to create a loop: commit {} would be both an ancestor and a descendant of \
             the duplicated commits",
            short_commit_hash(&commit_id),
        )));
    }
    Ok(())
}
