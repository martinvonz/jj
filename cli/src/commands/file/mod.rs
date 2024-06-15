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

pub mod chmod;
pub mod list;
pub mod print;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// File operations.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum FileCommand {
    Print(print::PrintArgs),
    Chmod(chmod::ChmodArgs),
    List(list::ListArgs),
}

pub fn cmd_file(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &FileCommand,
) -> Result<(), CommandError> {
    match subcommand {
        FileCommand::Print(sub_args) => print::cmd_print(ui, command, sub_args),
        FileCommand::Chmod(sub_args) => chmod::cmd_chmod(ui, command, sub_args),
        FileCommand::List(sub_args) => list::cmd_list(ui, command, sub_args),
    }
}
