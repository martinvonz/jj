// Copyright 2022 The Jujutsu Authors
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

use std::io::Write as _;

use jj_cli::cli_util::{CliRunner, CommandHelper};
use jj_cli::command_error::CommandError;
use jj_cli::ui::Ui;

#[derive(clap::Parser, Clone, Debug)]
enum CustomCommand {
    Frobnicate(FrobnicateArgs),
}

/// Frobnicate a revisions
#[derive(clap::Args, Clone, Debug)]
struct FrobnicateArgs {
    /// The revision to frobnicate
    #[arg(default_value = "@")]
    revision: String,
}

fn run_custom_command(
    ui: &mut Ui,
    command_helper: &CommandHelper,
    command: CustomCommand,
) -> Result<(), CommandError> {
    match command {
        CustomCommand::Frobnicate(args) => {
            let mut workspace_command = command_helper.workspace_helper(ui)?;
            let commit = workspace_command.resolve_single_rev(&args.revision)?;
            let mut tx = workspace_command.start_transaction();
            let new_commit = tx
                .mut_repo()
                .rewrite_commit(command_helper.settings(), &commit)
                .set_description("Frobnicated!")
                .write()?;
            tx.finish(ui, "Frobnicate")?;
            writeln!(
                ui.stderr(),
                "Frobnicated revision: {}",
                workspace_command.format_commit_summary(&new_commit)
            )?;
            Ok(())
        }
    }
}

fn main() -> std::process::ExitCode {
    CliRunner::init().add_subcommand(run_custom_command).run()
}
