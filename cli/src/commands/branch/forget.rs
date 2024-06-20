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

use jj_lib::str_util::StringPattern;
use jj_lib::view::View;

use super::{find_branches_with, make_branch_term};
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
    let view = workspace_command.repo().view();
    let names = find_forgettable_branches(view, &args.names)?;
    let mut tx = workspace_command.start_transaction();
    for branch_name in names.iter() {
        tx.mut_repo().remove_branch(branch_name);
    }
    tx.finish(ui, format!("forget {}", make_branch_term(&names)))?;
    if names.len() > 1 {
        writeln!(ui.status(), "Forgot {} branches.", names.len())?;
    }
    Ok(())
}

fn find_forgettable_branches(
    view: &View,
    name_patterns: &[StringPattern],
) -> Result<Vec<String>, CommandError> {
    find_branches_with(name_patterns, |pattern| {
        view.branches()
            .filter(|(name, _)| pattern.matches(name))
            .map(|(name, _)| name.to_owned())
    })
}
