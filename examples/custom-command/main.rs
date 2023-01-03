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

use clap::{ArgMatches, FromArgMatches};
use jujutsu::cli_util::{short_commit_description, CliRunner, CommandError, CommandHelper};
use jujutsu::commands::run_command;
use jujutsu::ui::Ui;

#[derive(clap::Parser, Clone, Debug)]
enum CustomCommands {
    Frobnicate(FrobnicateArgs),
}

/// Frobnicate a revisions
#[derive(clap::Args, Clone, Debug)]
struct FrobnicateArgs {
    /// The revision to frobnicate
    #[arg(default_value = "@")]
    revision: String,
}

fn run(
    ui: &mut Ui,
    command_helper: CommandHelper,
    matches: &ArgMatches,
) -> Result<(), CommandError> {
    match CustomCommands::from_arg_matches(matches) {
        // Handle our custom command
        Ok(CustomCommands::Frobnicate(args)) => {
            let mut workspace_command = command_helper.workspace_helper(ui)?;
            let commit = workspace_command.resolve_single_rev(&args.revision)?;
            let mut tx = workspace_command.start_transaction("Frobnicate");
            let new_commit = tx
                .mut_repo()
                .rewrite_commit(ui.settings(), &commit)
                .set_description("Frobnicated!")
                .write()?;
            workspace_command.finish_transaction(ui, tx)?;
            writeln!(
                ui,
                "Frobnicated revision: {}",
                short_commit_description(&new_commit)
            )?;
            Ok(())
        }
        // Handle default commands
        Err(_) => run_command(ui, &command_helper, matches),
    }
}

fn main() {
    CliRunner::init()
        .add_subcommand::<CustomCommands>()
        .set_dispatch_fn(run)
        .run_and_exit();
}
