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

use jj_lib::object_id::ObjectId;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::description_util::join_message_paragraphs;
use crate::ui::Ui;

/// Create a new, empty change and edit it in the working copy
///
/// For more information, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct CheckoutArgs {
    /// The revision to update to
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_checkout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CheckoutArgs,
) -> Result<(), CommandError> {
    writeln!(
        ui.warning_default(),
        "`jj checkout` is deprecated; use `jj new` instead, which is equivalent"
    )?;
    writeln!(
        ui.warning_default(),
        "`jj checkout` will be removed in a future version, and this will be a hard error"
    )?;
    let mut workspace_command = command.workspace_helper(ui)?;
    let target = workspace_command.resolve_single_rev(&args.revision)?;
    let mut tx = workspace_command.start_transaction();
    let commit_builder = tx
        .mut_repo()
        .new_commit(
            command.settings(),
            vec![target.id().clone()],
            target.tree_id().clone(),
        )
        .set_description(join_message_paragraphs(&args.message_paragraphs));
    let new_commit = commit_builder.write()?;
    tx.edit(&new_commit).unwrap();
    tx.finish(ui, format!("check out commit {}", target.id().hex()))?;
    Ok(())
}
