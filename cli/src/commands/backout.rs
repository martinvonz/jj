// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use jj_lib::object_id::ObjectId;
use jj_lib::rewrite::back_out_commit;
use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper, RevisionArg};
use crate::ui::Ui;

/// Apply the reverse of a revision on top of another revision
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct BackoutArgs {
    /// The revision to apply the reverse of
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The revision to apply the reverse changes on top of
    // TODO: It seems better to default this to `@-`. Maybe the working
    // copy should be rebased on top?
    #[arg(long, short, default_value = "@")]
    destination: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BackoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit_to_back_out = workspace_command.resolve_single_rev(&args.revision)?;
    let mut parents = vec![];
    for revision_str in &args.destination {
        let destination = workspace_command.resolve_single_rev(revision_str)?;
        parents.push(destination);
    }
    let mut tx = workspace_command.start_transaction();
    back_out_commit(
        command.settings(),
        tx.mut_repo(),
        &commit_to_back_out,
        &parents,
    )?;
    tx.finish(
        ui,
        format!("back out commit {}", commit_to_back_out.id().hex()),
    )?;

    Ok(())
}
