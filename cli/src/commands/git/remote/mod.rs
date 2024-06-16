// Copyright 2020-2023 The Jujutsu Authors
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

pub mod add;
pub mod list;
pub mod remove;
pub mod rename;

use clap::Subcommand;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Manage Git remotes
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    Add(add::Args),
    Remove(remove::Args),
    Rename(rename::Args),
    List(list::Args),
}

pub fn run(ui: &mut Ui, command: &CommandHelper, subcommand: &Command) -> Result<(), CommandError> {
    match subcommand {
        Command::Add(args) => add::run(ui, command, args),
        Command::Remove(args) => remove::run(ui, command, args),
        Command::Rename(args) => rename::run(ui, command, args),
        Command::List(args) => list::run(ui, command, args),
    }
}
