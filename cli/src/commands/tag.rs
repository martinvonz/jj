// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use jj_lib::str_util::StringPattern;

use crate::cli_util::{parse_string_pattern, CommandError, CommandHelper};
use crate::ui::Ui;

/// Manage tags.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum TagCommand {
    #[command(visible_alias("l"))]
    List(TagListArgs),
}

/// List tags.
#[derive(clap::Args, Clone, Debug)]
pub struct TagListArgs {
    /// Show tags whose local name matches
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select tags by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(value_parser = parse_string_pattern)]
    pub names: Vec<StringPattern>,
}

pub fn cmd_tag(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &TagCommand,
) -> Result<(), CommandError> {
    match subcommand {
        TagCommand::List(sub_args) => cmd_tag_list(ui, command, sub_args),
    }
}

fn cmd_tag_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TagListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let view = repo.view();

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    for name in view.tags().keys() {
        if !args.names.is_empty() && !args.names.iter().any(|pattern| pattern.matches(name)) {
            continue;
        }

        writeln!(formatter.labeled("tag"), "{name}")?;
    }

    Ok(())
}
