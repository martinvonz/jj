// Copyright 2022 Google LLC
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

use clap::{FromArgMatches, Subcommand};
use jujutsu::cli_util::{
    create_ui, parse_args, report_command_error, short_commit_description, CommandError,
};
use jujutsu::commands::{default_app, run_command};
use jujutsu::ui::Ui;
use jujutsu_lib::commit_builder::CommitBuilder;

#[derive(clap::Parser, Clone, Debug)]
enum CustomCommands {
    Frobnicate(FrobnicateArgs),
}

/// Frobnicate a revisions
#[derive(clap::Args, Clone, Debug)]
struct FrobnicateArgs {
    /// The revision to frobnicate
    #[clap(default_value = "@")]
    revision: String,
}

fn run(ui: &mut Ui) -> Result<(), CommandError> {
    let app = CustomCommands::augment_subcommands(default_app());
    let (command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    match CustomCommands::from_arg_matches(&matches) {
        // Handle our custom command
        Ok(CustomCommands::Frobnicate(args)) => {
            let mut workspace_command = command_helper.workspace_helper(ui)?;
            let commit = workspace_command.resolve_single_rev(&args.revision)?;
            let mut tx = workspace_command.start_transaction("Frobnicate");
            let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), &commit)
                .set_description("Frobnicated!".to_string())
                .write_to_repo(tx.mut_repo());
            workspace_command.finish_transaction(ui, tx)?;
            writeln!(
                ui,
                "Frobnicated revision: {}",
                short_commit_description(&new_commit)
            )?;
            Ok(())
        }
        // Handle default commands
        Err(_) => run_command(ui, &command_helper, &matches),
    }
}

fn main() {
    let mut ui = create_ui();
    let exit_code = match run(&mut ui) {
        Ok(()) => 0,
        Err(err) => report_command_error(&mut ui, err),
    };
    std::process::exit(exit_code);
}
