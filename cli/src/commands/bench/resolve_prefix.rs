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

use jj_lib::object_id::HexPrefix;
use jj_lib::repo::Repo as _;

use super::run_bench;
use super::CriterionArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Resolve a commit ID prefix
#[derive(clap::Args, Clone, Debug)]
pub struct BenchResolvePrefixArgs {
    prefix: String,
    #[command(flatten)]
    criterion: CriterionArgs,
}

pub fn cmd_bench_resolve_prefix(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BenchResolvePrefixArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let prefix = HexPrefix::new(&args.prefix).unwrap();
    let index = workspace_command.repo().index();
    let routine = || index.resolve_commit_id_prefix(&prefix);
    run_bench(
        ui,
        &format!("resolve-prefix-{}", prefix.hex()),
        &args.criterion,
        routine,
    )?;
    Ok(())
}
