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

use jj_lib::object_id::ObjectId;
use tracing::instrument;

use crate::cli_util::{
    print_dropped_signatures, resolve_multiple_nonempty_revsets, CommandHelper, RevisionArg,
};
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
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_abandon(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AbandonArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_abandon = resolve_multiple_nonempty_revsets(&args.revisions, &workspace_command)?;
    workspace_command.check_rewritable(to_abandon.iter())?;
    let mut tx = workspace_command.start_transaction();
    for commit in &to_abandon {
        tx.mut_repo().record_abandoned_commit(commit.id().clone());
    }
    let rebase_counts = tx.mut_repo().rebase_descendants(command.settings())?;
    let num_rebased = rebase_counts.rebased;

    if to_abandon.len() == 1 {
        write!(ui.stderr(), "Abandoned commit ")?;
        tx.base_workspace_helper()
            .write_commit_summary(ui.stderr_formatter().as_mut(), &to_abandon[0])?;
        writeln!(ui.stderr())?;
    } else if !args.summary {
        let mut formatter = ui.stderr_formatter();
        let template = tx.base_workspace_helper().commit_summary_template();
        writeln!(formatter, "Abandoned the following commits:")?;
        for commit in &to_abandon {
            write!(formatter, "  ")?;
            template.format(commit, formatter.as_mut())?;
            writeln!(formatter)?;
        }
    } else {
        writeln!(ui.stderr(), "Abandoned {} commits.", &to_abandon.len())?;
    }
    if num_rebased > 0 {
        writeln!(
            ui.stderr(),
            "Rebased {num_rebased} descendant commits onto parents of abandoned commits"
        )?;
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
    print_dropped_signatures(ui, rebase_counts.dropped_signatures)?;
    tx.finish(ui, transaction_description)?;
    Ok(())
}
