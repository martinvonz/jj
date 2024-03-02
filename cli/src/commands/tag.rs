// Copyright 2020-2024 The Jujutsu Authors
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

use itertools::Itertools;
use jj_lib::op_store::RefTarget;
use jj_lib::str_util::StringPattern;

use crate::cli_util::{parse_string_pattern, user_error, CommandError, CommandHelper};
use crate::ui::Ui;

/// Manage tags.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum TagCommand {
    #[command(visible_alias("l"))]
    List(TagListArgs),
    #[command(visible_alias("f"))]
    Forget(TagForgetArgs),
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

/// Forget tags. They may be recreated on `fetch`.
#[derive(clap::Args, Clone, Debug)]
pub struct TagForgetArgs {
    /// Forget tags whose local name matches
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select tags by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(required = true, value_parser = parse_string_pattern)]
    pub names: Vec<StringPattern>,
}

pub fn cmd_tag(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &TagCommand,
) -> Result<(), CommandError> {
    match subcommand {
        TagCommand::List(sub_args) => cmd_tag_list(ui, command, sub_args),
        TagCommand::Forget(sub_args) => cmd_tag_forget(ui, command, sub_args),
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

fn cmd_tag_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TagForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut tx = workspace_command.start_transaction();
    let view = tx.repo().base_repo().view();

    let mut to_forget = vec![];
    let mut unmatched_patterns = vec![];
    for pattern in args.names.iter() {
        let mut matched = false;
        for tagname in view.tags().keys().filter(|name| pattern.matches(name)) {
            matched = true;
            to_forget.push(tagname.clone());
        }
        if !matched {
            unmatched_patterns.push(pattern.to_string())
        }
    }
    if !unmatched_patterns.is_empty() {
        return Err(user_error(format!(
            "No matching tags for patterns: {}",
            unmatched_patterns.into_iter().join(", ")
        )));
    }
    for tagname in to_forget.iter() {
        // Forgetting a tag is the same as deleting it at the moment, as we never push
        // tags.
        tx.mut_repo().set_tag_target(tagname, RefTarget::absent());
    }
    tx.finish(ui, format!("forgot {} tags", to_forget.len()))?;

    if to_forget.len() > 1 {
        writeln!(ui.stderr(), "Forgot {} branches.", to_forget.len())?;
    };
    Ok(())
}
