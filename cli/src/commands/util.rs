// Copyright 2020 The Jujutsu Authors
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

use std::io::Write;

use clap::Subcommand;
use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper};
use crate::ui::Ui;

/// Infrequently used commands such as for generating shell completions
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum UtilCommand {
    Completion(UtilCompletionArgs),
    Mangen(UtilMangenArgs),
    ConfigSchema(UtilConfigSchemaArgs),
}

/// Print a command-line-completion script
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct UtilCompletionArgs {
    /// Print a completion script for Bash
    ///
    /// Apply it by running this:
    ///
    /// source <(jj util completion)
    #[arg(long, verbatim_doc_comment)]
    bash: bool,
    /// Print a completion script for Fish
    ///
    /// Apply it by running this:
    ///
    /// jj util completion --fish | source
    #[arg(long, verbatim_doc_comment)]
    fish: bool,
    /// Print a completion script for Zsh
    ///
    /// Apply it by running this:
    ///
    /// autoload -U compinit
    /// compinit
    /// source <(jj util completion --zsh)
    #[arg(long, verbatim_doc_comment)]
    zsh: bool,
}

/// Print a ROFF (manpage)
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct UtilMangenArgs {}

/// Print the JSON schema for the jj TOML config format.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct UtilConfigSchemaArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_util(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &UtilCommand,
) -> Result<(), CommandError> {
    match subcommand {
        UtilCommand::Completion(args) => cmd_util_completion(ui, command, args),
        UtilCommand::Mangen(args) => cmd_util_mangen(ui, command, args),
        UtilCommand::ConfigSchema(args) => cmd_util_config_schema(ui, command, args),
    }
}

fn cmd_util_completion(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UtilCompletionArgs,
) -> Result<(), CommandError> {
    let mut app = command.app().clone();
    let mut buf = vec![];
    let shell = if args.zsh {
        clap_complete::Shell::Zsh
    } else if args.fish {
        clap_complete::Shell::Fish
    } else {
        clap_complete::Shell::Bash
    };
    clap_complete::generate(shell, &mut app, "jj", &mut buf);
    ui.stdout_formatter().write_all(&buf)?;
    Ok(())
}

fn cmd_util_mangen(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &UtilMangenArgs,
) -> Result<(), CommandError> {
    let mut buf = vec![];
    let man = clap_mangen::Man::new(command.app().clone());
    man.render(&mut buf)?;
    ui.stdout_formatter().write_all(&buf)?;
    Ok(())
}

fn cmd_util_config_schema(
    ui: &mut Ui,
    _command: &CommandHelper,
    _args: &UtilConfigSchemaArgs,
) -> Result<(), CommandError> {
    // TODO(#879): Consider generating entire schema dynamically vs. static file.
    let buf = include_bytes!("../config-schema.json");
    ui.stdout_formatter().write_all(buf)?;
    Ok(())
}
