// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Write as _;

use jj_cli::cli_util::{CliRunner, CommandError};
use jj_cli::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
struct CustomGlobalArgs {
    /// Show a greeting before each command
    #[arg(long, global = true)]
    greet: bool,
}

fn process_before(ui: &mut Ui, custom_global_args: CustomGlobalArgs) -> Result<(), CommandError> {
    if custom_global_args.greet {
        writeln!(ui.stdout(), "Hello!")?;
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    CliRunner::init().add_global_args(process_before).run()
}
