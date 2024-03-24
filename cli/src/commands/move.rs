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

use clap::ArgGroup;
use jj_lib::object_id::ObjectId;
use tracing::instrument;

use super::squash::move_diff;
use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Move changes from one revision into another
///
/// Use `--interactive` to move only part of the source revision into the
/// destination. The selected changes (or all the changes in the source revision
/// if not using `--interactive`) will be moved into the destination. The
/// changes will be removed from the source. If that means that the source is
/// now empty compared to its parent, it will be abandoned. Without
/// `--interactive`, the source change will always be empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("to_move").args(&["from", "to"]).multiple(true).required(true)))]
pub(crate) struct MoveArgs {
    /// Move part of this change into the destination
    #[arg(long, short)]
    from: Option<RevisionArg>,
    /// Move part of the source into this change
    #[arg(long, short)]
    to: Option<RevisionArg>,
    /// Interactively choose which parts to move
    #[arg(long, short)]
    interactive: bool,
    /// Specify diff editor to be used (implies --interactive)
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
    /// Move only changes to these paths (instead of all paths)
    #[arg(conflicts_with_all = ["interactive", "tool"], value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_move(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &MoveArgs,
) -> Result<(), CommandError> {
    writeln!(
        ui.warning_default(),
        "`jj move` is deprecated; use `jj squash` instead, which is equivalent"
    )?;
    writeln!(
        ui.warning_default(),
        "`jj move` will be removed in a future version, and this will be a hard error"
    )?;
    let mut workspace_command = command.workspace_helper(ui)?;
    let source = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
    let destination = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;
    if source.id() == destination.id() {
        return Err(user_error("Source and destination cannot be the same."));
    }
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_selector =
        workspace_command.diff_selector(ui, args.tool.as_deref(), args.interactive)?;
    let mut tx = workspace_command.start_transaction();
    let tx_description = format!(
        "move changes from {} to {}",
        source.id().hex(),
        destination.id().hex()
    );
    move_diff(
        ui,
        &mut tx,
        command.settings(),
        &[source],
        &destination,
        matcher.as_ref(),
        &diff_selector,
        None,
        false,
        &args.paths,
    )?;
    tx.finish(ui, tx_description)?;
    Ok(())
}
