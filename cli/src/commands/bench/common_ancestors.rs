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

/// Find the common ancestor(s) of a set of commits
#[derive(clap::Args, Clone, Debug)]
pub struct BenchCommonAncestorsArgs {
    revision1: RevisionArg,
    revision2: RevisionArg,
    #[command(flatten)]
    criterion: CriterionArgs,
}

pub fn cmd_bench_common_ancestors(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BenchCommonAncestorsArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit1 = workspace_command.resolve_single_rev(ui, &args.revision1)?;
    let commit2 = workspace_command.resolve_single_rev(ui, &args.revision2)?;
    let index = workspace_command.repo().index();
    let routine = || index.common_ancestors(&[commit1.id().clone()], &[commit2.id().clone()]);
    run_bench(
        ui,
        &format!("common-ancestors-{}-{}", args.revision1, args.revision2),
        &args.criterion,
        routine,
    )?;
    Ok(())
}
