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

use std::io::{self, Read, Write};

use jj_lib::object_id::ObjectId;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::description_util::{
    description_template_for_describe, edit_description, join_message_paragraphs,
};
use crate::ui::Ui;

/// Update the change description or other metadata
///
/// Starts an editor to let you edit the description of a change. The editor
/// will be $EDITOR, or `pico` if that's not defined (`Notepad` on Windows).
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases = &["desc"])]
pub(crate) struct DescribeArgs {
    /// The revision whose description to edit
    #[arg(default_value = "@")]
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Read the change description from stdin
    #[arg(long)]
    stdin: bool,
    /// Don't open an editor
    ///
    /// This is mainly useful in combination with e.g. `--reset-author`.
    #[arg(long)]
    no_edit: bool,
    /// Reset the author to the configured user
    ///
    /// This resets the author name, email, and timestamp.
    ///
    /// You can use it in combination with the JJ_USER and JJ_EMAIL
    /// environment variables to set a different author:
    ///
    /// $ JJ_USER='Foo Bar' JJ_EMAIL=foo@bar.com jj describe --reset-author
    #[arg(long)]
    reset_author: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_describe(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DescribeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewritable([&commit])?;
    let description = if args.stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer).unwrap();
        buffer
    } else if !args.message_paragraphs.is_empty() {
        join_message_paragraphs(&args.message_paragraphs)
    } else if args.no_edit {
        commit.description().to_owned()
    } else {
        let template =
            description_template_for_describe(ui, command.settings(), &workspace_command, &commit)?;
        edit_description(workspace_command.repo(), &template, command.settings())?
    };
    if description == *commit.description() && !args.reset_author {
        writeln!(ui.status(), "Nothing changed.")?;
    } else {
        let mut tx = workspace_command.start_transaction();
        let mut commit_builder = tx
            .mut_repo()
            .rewrite_commit(command.settings(), &commit)
            .set_description(description);
        if args.reset_author {
            let new_author = commit_builder.committer().clone();
            commit_builder = commit_builder.set_author(new_author);
        }
        commit_builder.write()?;
        tx.finish(ui, format!("describe commit {}", commit.id().hex()))?;
    }
    Ok(())
}
