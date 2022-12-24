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

use clap::{FromArgMatches, Subcommand};
use jujutsu::cli_util::{
    create_ui, handle_command_result, parse_args, short_commit_description, CommandError,
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
    #[arg(default_value = "@")]
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
            let new_commit = CommitBuilder::for_rewrite_from(tx.mut_repo(), ui.settings(), &commit)
                .set_description("Frobnicated!")
                .write();
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
    jujutsu::cleanup_guard::init();
    let (mut ui, result) = create_ui();
    let result = result.and_then(|()| run(&mut ui));
    let exit_code = handle_command_result(&mut ui, result);
    ui.finalize_writes();
    std::process::exit(exit_code);
}
