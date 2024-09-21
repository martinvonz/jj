// Copyright 2024 The Jujutsu Authors
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

use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Print this message or the help of the given subcommand(s)
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct HelpArgs {
    /// Print help for the subcommand(s)
    pub(crate) command: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_help(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &HelpArgs,
) -> Result<(), CommandError> {
    let mut args_to_show_help = vec![command.app().get_name()];
    args_to_show_help.extend(args.command.iter().map(|s| s.as_str()));
    args_to_show_help.push("--help");

    // TODO: `help log -- -r` will gives an cryptic error, ideally, it should state
    // that the subcommand `log -r` doesn't exist.
    let help_err = command
        .app()
        .clone()
        .subcommand_required(true)
        .try_get_matches_from(args_to_show_help)
        .expect_err("Clap library should return a DisplayHelp error in this context");

    Err(command_error::cli_error(help_err))
}
