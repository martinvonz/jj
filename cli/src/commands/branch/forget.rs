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
use jj_lib::op_store::{BranchTarget, RefTarget, RemoteRef};
use jj_lib::str_util::StringPattern;
use jj_lib::view::View;

use super::find_branches_with;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Forget everything about a branch, including its local and remote
/// targets
///
/// A forgotten branch will not impact remotes on future pushes. It will be
/// recreated on future pulls if it still exists in the remote.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchForgetArgs {
    /// The branches to forget
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(required = true, value_parser = StringPattern::parse)]
    names: Vec<StringPattern>,
}

pub fn cmd_branch_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();
    let matched_branches = find_forgettable_branches(repo.view(), &args.names)?;
    let mut tx = workspace_command.start_transaction();
    for (name, branch_target) in &matched_branches {
        tx.mut_repo()
            .set_local_branch_target(name, RefTarget::absent());
        for (remote_name, _) in &branch_target.remote_refs {
            tx.mut_repo()
                .set_remote_branch(name, remote_name, RemoteRef::absent());
        }
    }
    tx.finish(
        ui,
        format!(
            "forget branch {}",
            matched_branches.iter().map(|(name, _)| name).join(", ")
        ),
    )?;
    writeln!(ui.status(), "Forgot {} branches.", matched_branches.len())?;
    Ok(())
}

fn find_forgettable_branches<'a>(
    view: &'a View,
    name_patterns: &[StringPattern],
) -> Result<Vec<(&'a str, BranchTarget<'a>)>, CommandError> {
    find_branches_with(name_patterns, |pattern| {
        view.branches().filter(|(name, _)| pattern.matches(name))
    })
}
