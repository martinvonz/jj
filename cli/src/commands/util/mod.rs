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

mod completion;
mod config_schema;
mod exec;
mod gc;
mod mangen;
mod markdown_help;

use clap::Subcommand;
use tracing::instrument;

use self::completion::cmd_util_completion;
use self::completion::UtilCompletionArgs;
use self::config_schema::cmd_util_config_schema;
use self::config_schema::UtilConfigSchemaArgs;
use self::exec::cmd_util_exec;
use self::exec::UtilExecArgs;
use self::gc::cmd_util_gc;
use self::gc::UtilGcArgs;
use self::mangen::cmd_util_mangen;
use self::mangen::UtilMangenArgs;
use self::markdown_help::cmd_util_markdown_help;
use self::markdown_help::UtilMarkdownHelp;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Infrequently used commands such as for generating shell completions
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum UtilCommand {
    Completion(UtilCompletionArgs),
    ConfigSchema(UtilConfigSchemaArgs),
    Exec(UtilExecArgs),
    Gc(UtilGcArgs),
    Mangen(UtilMangenArgs),
    MarkdownHelp(UtilMarkdownHelp),
}

#[instrument(skip_all)]
pub(crate) fn cmd_util(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &UtilCommand,
) -> Result<(), CommandError> {
    match subcommand {
        UtilCommand::Completion(args) => cmd_util_completion(ui, command, args),
        UtilCommand::ConfigSchema(args) => cmd_util_config_schema(ui, command, args),
        UtilCommand::Exec(args) => cmd_util_exec(ui, command, args),
        UtilCommand::Gc(args) => cmd_util_gc(ui, command, args),
        UtilCommand::Mangen(args) => cmd_util_mangen(ui, command, args),
        UtilCommand::MarkdownHelp(args) => cmd_util_markdown_help(ui, command, args),
    }
}
