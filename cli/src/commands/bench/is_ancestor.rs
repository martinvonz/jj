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

use jj_lib::repo::Repo as _;

use super::run_bench;
use super::CriterionArgs;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Checks if the first commit is an ancestor of the second commit
#[derive(clap::Args, Clone, Debug)]
pub struct BenchIsAncestorArgs {
    ancestor: RevisionArg,
    descendant: RevisionArg,
    #[command(flatten)]
    criterion: CriterionArgs,
}

pub fn cmd_bench_is_ancestor(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BenchIsAncestorArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let ancestor_commit = workspace_command.resolve_single_rev(ui, &args.ancestor)?;
    let descendant_commit = workspace_command.resolve_single_rev(ui, &args.descendant)?;
    let index = workspace_command.repo().index();
    let routine = || index.is_ancestor(ancestor_commit.id(), descendant_commit.id());
    run_bench(
        ui,
        &format!("is-ancestor-{}-{}", args.ancestor, args.descendant),
        &args.criterion,
        routine,
    )?;
    Ok(())
}
