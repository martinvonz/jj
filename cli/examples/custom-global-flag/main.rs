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

use std::io::Write as _;

use jj_cli::cli_util::CliRunner;
use jj_cli::command_error::CommandError;
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
