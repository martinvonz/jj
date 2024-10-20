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

use std::fmt::Write as _;
use std::io::Write;

use crossterm::style::Stylize;
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
    ui: &mut Ui,
    command: &CommandHelper,
    args: &HelpArgs,
) -> Result<(), CommandError> {
    if let [name] = &*args.command {
        if let Some(category) = find_category(name.as_str()) {
            ui.request_pager();
            write!(ui.stdout(), "{}", category.content)?;

            return Ok(());
        }
    }

    let mut args_to_show_help = vec![command.app().get_name()];
    args_to_show_help.extend(args.command.iter().map(|s| s.as_str()));
    args_to_show_help.push("--help");

    // TODO: `help log -- -r` will gives an cryptic error, ideally, it should state
    // that the subcommand `log -r` doesn't exist.
    let help_err = command
        .app()
        .clone()
        .subcommand_required(true)
        .after_help(format_categories(command.app()))
        .try_get_matches_from(args_to_show_help)
        .expect_err("Clap library should return a DisplayHelp error in this context");

    Err(command_error::cli_error(help_err))
}

#[derive(Clone)]
struct Category {
    description: &'static str,
    content: &'static str,
}

// TODO: Add all documentation to categories
//
// Maybe adding some code to build.rs to find all the docs files and build the
// `CATEGORIES` at compile time.
//
// It would be cool to follow the docs hierarchy somehow.
//
// One of the problems would be `config.md`, as it has the same name as a
// subcommand.
//
// TODO: Find a way to render markdown using ANSI escape codes.
//
// Maybe we can steal some ideas from https://github.com/martinvonz/jj/pull/3130
const CATEGORIES: &[(&str, Category)] = &[
    (
        "revsets",
        Category {
            description: "A functional language for selecting a set of revision",
            content: include_str!("../../../docs/revsets.md"),
        },
    ),
    (
        "tutorial",
        Category {
            description: "Show a tutorial to get started with jj",
            content: include_str!("../../../docs/tutorial.md"),
        },
    ),
];

fn find_category(name: &str) -> Option<&Category> {
    CATEGORIES
        .iter()
        .find(|(cat_name, _)| *cat_name == name)
        .map(|tuple| &tuple.1)
}

fn format_categories(command: &clap::Command) -> String {
    let subcommand_max_len = command
        .get_subcommands()
        .map(|cmd| cmd.get_name().len())
        .max()
        .unwrap();

    let mut ret = String::new();

    writeln!(ret, "{}", "Help Categories:".bold().underlined()).unwrap();
    for (name, category) in CATEGORIES {
        write!(ret, "  {}  ", format!("{name:<subcommand_max_len$}").bold()).unwrap();
        writeln!(ret, "{}", category.description,).unwrap();
    }

    ret
}
