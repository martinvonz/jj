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

use std::io::Write as _;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Print the CLI help for all subcommands in Markdown
#[derive(clap::Args, Clone, Debug)]
pub struct UtilMarkdownHelp {}

pub fn cmd_util_markdown_help(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &UtilMarkdownHelp,
) -> Result<(), CommandError> {
    // If we ever need more flexibility, the code of `clap_markdown` is simple and
    // readable. We could reimplement the parts we need without trouble.
    let markdown = clap_markdown::help_markdown_command(command.app()).into_bytes();
    ui.stdout().write_all(&markdown)?;
    Ok(())
}
