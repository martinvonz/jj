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

use clap::builder::PossibleValue;
use clap::builder::StyledStr;
use crossterm::style::Stylize;
use itertools::Itertools;
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
    /// Show help for keywords instead of commands
    #[arg(
        long,
        short = 'k',
        conflicts_with = "command",
        value_parser = KEYWORDS
            .iter()
            .map(|k| PossibleValue::new(k.name).help(k.description))
            .collect_vec()
    )]
    pub(crate) keyword: Option<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_help(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &HelpArgs,
) -> Result<(), CommandError> {
    if let Some(name) = &args.keyword {
        let keyword = find_keyword(name).expect("clap should check this with `value_parser`");
        ui.request_pager();
        write!(ui.stdout(), "{}", keyword.content)?;

        return Ok(());
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
        .try_get_matches_from(args_to_show_help)
        .expect_err("Clap library should return a DisplayHelp error in this context");

    Err(command_error::cli_error(help_err))
}

#[derive(Clone)]
struct Keyword {
    name: &'static str,
    description: &'static str,
    content: &'static str,
}

// TODO: Add all documentation to keywords
//
// Maybe adding some code to build.rs to find all the docs files and build the
// `KEYWORDS` at compile time.
//
// It would be cool to follow the docs hierarchy somehow.
//
// One of the problems would be `config.md`, as it has the same name as a
// subcommand.
//
// TODO: Find a way to render markdown using ANSI escape codes.
//
// Maybe we can steal some ideas from https://github.com/martinvonz/jj/pull/3130
const KEYWORDS: &[Keyword] = &[
    Keyword {
        name: "revsets",
        description: "A functional language for selecting a set of revision",
        content: include_str!(concat!("../../", env!("JJ_DOCS_DIR"), "revsets.md")),
    },
    Keyword {
        name: "tutorial",
        description: "Show a tutorial to get started with jj",
        content: include_str!(concat!("../../", env!("JJ_DOCS_DIR"), "tutorial.md")),
    },
];

fn find_keyword(name: &str) -> Option<&Keyword> {
    KEYWORDS.iter().find(|keyword| keyword.name == name)
}

pub fn show_keyword_hint_after_help() -> StyledStr {
    let mut ret = StyledStr::new();
    writeln!(
        ret,
        "{} list available keywords. Use {} to show help for one of these keywords.",
        "'jj help --help'".bold(),
        "'jj help -k'".bold(),
    )
    .unwrap();
    ret
}
