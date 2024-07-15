// Copyright 2020-2023 The Jujutsu Authors
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

use clap::builder::NonEmptyStringValueParser;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::RefTarget;

use super::{has_tracked_remote_branches, is_fast_forward};
use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error_with_hint, CommandError};
use crate::ui::Ui;

/// Create or update a branch to point to a certain commit
#[derive(clap::Args, Clone, Debug)]
pub struct BranchSetArgs {
    /// The branch's target revision
    #[arg(long, short, visible_alias = "to")]
    revision: Option<RevisionArg>,

    /// Allow moving the branch backwards or sideways
    #[arg(long, short = 'B')]
    allow_backwards: bool,

    /// The branches to update
    #[arg(required = true, value_parser = NonEmptyStringValueParser::new())]
    names: Vec<String>,
}

pub fn cmd_branch_set(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchSetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let target_commit =
        workspace_command.resolve_single_rev(args.revision.as_ref().unwrap_or(&RevisionArg::AT))?;
    let repo = workspace_command.repo().as_ref();
    let branch_names = &args.names;
    let mut new_branch_count = 0;
    let mut moved_branch_count = 0;
    for name in branch_names {
        let old_target = repo.view().get_local_branch(name);
        // If a branch is absent locally but is still tracking remote branches,
        // we are resurrecting the local branch, not "creating" a new branch.
        if old_target.is_absent() && !has_tracked_remote_branches(repo.view(), name) {
            new_branch_count += 1;
        } else if old_target.as_normal() != Some(target_commit.id()) {
            moved_branch_count += 1;
        }
        if !args.allow_backwards && !is_fast_forward(repo, old_target, target_commit.id()) {
            return Err(user_error_with_hint(
                format!("Refusing to move branch backwards or sideways: {name}"),
                "Use --allow-backwards to allow it.",
            ));
        }
    }

    let mut tx = workspace_command.start_transaction();
    for branch_name in branch_names {
        tx.mut_repo()
            .set_local_branch_target(branch_name, RefTarget::normal(target_commit.id().clone()));
    }

    if let Some(mut formatter) = ui.status_formatter() {
        if new_branch_count > 0 {
            write!(
                formatter,
                "Created {new_branch_count} branches pointing to "
            )?;
            tx.write_commit_summary(formatter.as_mut(), &target_commit)?;
            writeln!(formatter)?;
        }
        if moved_branch_count > 0 {
            write!(formatter, "Moved {moved_branch_count} branches to ")?;
            tx.write_commit_summary(formatter.as_mut(), &target_commit)?;
            writeln!(formatter)?;
        }
    }
    if branch_names.len() > 1 && args.revision.is_none() {
        writeln!(ui.hint_default(), "Use -r to specify the target revision.")?;
    }
    if new_branch_count > 0 {
        // TODO: delete this hint in jj 0.25+
        writeln!(
            ui.hint_default(),
            "Consider using `jj branch move` if your intention was to move existing branches."
        )?;
    }

    tx.finish(
        ui,
        format!(
            "point branch {names} to commit {id}",
            names = branch_names.join(", "),
            id = target_commit.id().hex()
        ),
    )?;
    Ok(())
}
