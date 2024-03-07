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

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Sets the specified revision as the working-copy revision
///
/// Note: it is generally recommended to instead use `jj new` and `jj
/// squash`.
///
/// For more information, see https://martinvonz.github.io/jj/latest/FAQ#how-do-i-resume-working-on-an-existing-change
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct EditArgs {
    /// The commit to edit
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_edit(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &EditArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewritable([&new_commit])?;
    if workspace_command.get_wc_commit_id() == Some(new_commit.id()) {
        writeln!(ui.stderr(), "Already editing that commit")?;
    } else {
        let mut tx = workspace_command.start_transaction();
        tx.edit(&new_commit)?;
        tx.finish(ui, format!("edit commit {}", new_commit.id().hex()))?;
    }
    Ok(())
}
