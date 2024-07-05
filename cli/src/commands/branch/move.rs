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

use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::RefTarget;
use jj_lib::str_util::StringPattern;

use super::{find_branches_with, is_fast_forward};
use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error_with_hint, CommandError};
use crate::ui::Ui;

/// Move existing branches to target revision
///
/// If branch names are given, the specified branches will be updated to point
/// to the target revision.
///
/// If `--from` options are given, branches currently pointing to the specified
/// revisions will be updated. The branches can also be filtered by names.
///
/// Example: pull up the nearest branches to the working-copy parent
///
/// $ jj branch move --from 'heads(::@- & branches())' --to @-
#[derive(clap::Args, Clone, Debug)]
#[command(group(clap::ArgGroup::new("source").multiple(true).required(true)))]
pub struct BranchMoveArgs {
    /// Move branches from the given revisions
    #[arg(long, group = "source", value_name = "REVISIONS")]
    from: Vec<RevisionArg>,

    /// Move branches to this revision
    #[arg(long, default_value = "@", value_name = "REVISION")]
    to: RevisionArg,

    /// Allow moving branches backwards or sideways
    #[arg(long, short = 'B')]
    allow_backwards: bool,

    /// Move branches matching the given name patterns
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(group = "source", value_parser = StringPattern::parse)]
    names: Vec<StringPattern>,
}

pub fn cmd_branch_move(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchMoveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();

    let target_commit = workspace_command.resolve_single_rev(&args.to)?;
    let matched_branches = {
        let is_source_commit = if !args.from.is_empty() {
            workspace_command
                .parse_union_revsets(&args.from)?
                .evaluate()?
                .containing_fn()
        } else {
            Box::new(|_: &CommitId| true)
        };
        let mut branches = if !args.names.is_empty() {
            find_branches_with(&args.names, |pattern| {
                repo.view()
                    .local_branches_matching(pattern)
                    .filter(|(_, target)| target.added_ids().any(&is_source_commit))
            })?
        } else {
            repo.view()
                .local_branches()
                .filter(|(_, target)| target.added_ids().any(&is_source_commit))
                .collect()
        };
        // Noop matches aren't error, but should be excluded from stats.
        branches.retain(|(_, old_target)| old_target.as_normal() != Some(target_commit.id()));
        branches
    };

    if matched_branches.is_empty() {
        writeln!(ui.status(), "No branches to update.")?;
        return Ok(());
    }

    if !args.allow_backwards {
        if let Some((name, _)) = matched_branches
            .iter()
            .find(|(_, old_target)| !is_fast_forward(repo.as_ref(), old_target, target_commit.id()))
        {
            return Err(user_error_with_hint(
                format!("Refusing to move branch backwards or sideways: {name}"),
                "Use --allow-backwards to allow it.",
            ));
        }
    }

    let mut tx = workspace_command.start_transaction();
    for (name, _) in &matched_branches {
        tx.mut_repo()
            .set_local_branch_target(name, RefTarget::normal(target_commit.id().clone()));
    }

    if let Some(mut formatter) = ui.status_formatter() {
        write!(formatter, "Moved {} branches to ", matched_branches.len())?;
        tx.write_commit_summary(formatter.as_mut(), &target_commit)?;
        writeln!(formatter)?;
    }
    if matched_branches.len() > 1 && args.names.is_empty() {
        writeln!(
            ui.hint_default(),
            "Specify branch by name to update just one of the branches."
        )?;
    }

    tx.finish(
        ui,
        format!(
            "point branch {names} to commit {id}",
            names = matched_branches.iter().map(|(name, _)| name).join(", "),
            id = target_commit.id().hex()
        ),
    )?;
    Ok(())
}
