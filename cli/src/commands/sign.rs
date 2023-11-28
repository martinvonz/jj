// Copyright 2023 The Jujutsu Authors
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
use jj_lib::signing::SignBehavior;

use crate::cli_util::{user_error, CommandError, CommandHelper, RevisionArg};
use crate::ui::Ui;

/// Cryptographically sign a revision
#[derive(clap::Args, Clone, Debug)]
pub struct SignArgs {
    /// What key to use, depends on the configured signing backend.
    #[arg()]
    key: Option<String>,
    /// What revision to sign
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Sign a commit that is not authored by you or was already signed.
    #[arg(long, short)]
    force: bool,
    /// Drop the signature, explicitly "un-signing" the commit.
    #[arg(long, short = 'D', conflicts_with = "force")]
    drop: bool,
}

pub fn cmd_sign(ui: &mut Ui, command: &CommandHelper, args: &SignArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewritable([&commit])?;

    if !args.force {
        if !args.drop && commit.is_signed() {
            return Err(user_error(
                "Commit is already signed, use --force to sign anyway",
            ));
        }
        if commit.author().email != command.settings().user_email() {
            return Err(user_error(
                "Commit is not authored by you, use --force to sign anyway",
            ));
        }
    }

    let mut tx = workspace_command.start_transaction();

    let behavior = if args.drop {
        SignBehavior::Drop
    } else if args.force {
        SignBehavior::Force
    } else {
        SignBehavior::Own
    };
    let rewritten = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .override_sign_key(args.key.clone())
        .set_sign_behavior(behavior)
        .write()?;

    tx.finish(ui, format!("sign commit {}", commit.id().hex()))?;

    let summary = workspace_command.format_commit_summary(&rewritten);
    if args.drop {
        writeln!(ui.stderr(), "Signature was dropped: {summary}")?;
    } else {
        writeln!(ui.stderr(), "Commit was signed: {summary}")?;
    }

    Ok(())
}
