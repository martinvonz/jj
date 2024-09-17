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

use itertools::Itertools as _;
use jj_lib::commit::CommitIteratorExt;
use jj_lib::object_id::ObjectId;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Abandon a revision
///
/// Abandon a revision, rebasing descendants onto its parent(s). The behavior is
/// similar to `jj restore --changes-in`; the difference is that `jj abandon`
/// gives you a new change, while `jj restore` updates the existing change.
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct AbandonArgs {
    /// The revision(s) to abandon
    #[arg(default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// Do not print every abandoned commit on a separate line
    #[arg(long, short)]
    summary: bool,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true, action = clap::ArgAction::Count)]
    unused_revision: u8,
    /// Do not modify the content of the children of the abandoned commits
    #[arg(long)]
    restore_descendants: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_abandon(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AbandonArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_abandon: Vec<_> = workspace_command
        .parse_union_revsets(ui, &args.revisions)?
        .evaluate_to_commits()?
        .try_collect()?;
    if to_abandon.is_empty() {
        writeln!(ui.status(), "No revisions to abandon.")?;
        return Ok(());
    }
    workspace_command.check_rewritable(to_abandon.iter().ids())?;

    let mut tx = workspace_command.start_transaction();
    for commit in &to_abandon {
        tx.repo_mut().record_abandoned_commit(commit.id().clone());
    }
    let (num_rebased, extra_msg) = if args.restore_descendants {
        (
            tx.repo_mut().reparent_descendants(command.settings())?,
            " (while preserving their content)",
        )
    } else {
        (tx.repo_mut().rebase_descendants(command.settings())?, "")
    };

    if let Some(mut formatter) = ui.status_formatter() {
        if to_abandon.len() == 1 {
            write!(formatter, "Abandoned commit ")?;
            tx.base_workspace_helper()
                .write_commit_summary(formatter.as_mut(), &to_abandon[0])?;
            writeln!(ui.status())?;
        } else if !args.summary {
            let template = tx.base_workspace_helper().commit_summary_template();
            writeln!(formatter, "Abandoned the following commits:")?;
            for commit in &to_abandon {
                write!(formatter, "  ")?;
                template.format(commit, formatter.as_mut())?;
                writeln!(formatter)?;
            }
        } else {
            writeln!(formatter, "Abandoned {} commits.", &to_abandon.len())?;
        }
        if num_rebased > 0 {
            writeln!(
                formatter,
                "Rebased {num_rebased} descendant commits{extra_msg} onto parents of abandoned \
                 commits",
            )?;
        }
    }
    let transaction_description = if to_abandon.len() == 1 {
        format!("abandon commit {}", to_abandon[0].id().hex())
    } else {
        format!(
            "abandon commit {} and {} more",
            to_abandon[0].id().hex(),
            to_abandon.len() - 1
        )
    };
    tx.finish(ui, transaction_description)?;
    Ok(())
}
