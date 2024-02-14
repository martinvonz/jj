// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Write as _;

use jj_cli::cli_util::{CliRunner, CommandError, CommandHelper};
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
