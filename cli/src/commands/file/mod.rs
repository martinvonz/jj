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
pub mod show;
pub mod track;
pub mod untrack;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// File operations.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum FileCommand {
    Chmod(chmod::FileChmodArgs),
    List(list::FileListArgs),
    Show(show::FileShowArgs),
    Track(track::FileTrackArgs),
    Untrack(untrack::FileUntrackArgs),
}

pub fn cmd_file(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &FileCommand,
) -> Result<(), CommandError> {
    match subcommand {
        FileCommand::Chmod(args) => chmod::cmd_file_chmod(ui, command, args),
        FileCommand::List(args) => list::cmd_file_list(ui, command, args),
        FileCommand::Show(args) => show::cmd_file_show(ui, command, args),
        FileCommand::Track(args) => track::cmd_file_track(ui, command, args),
        FileCommand::Untrack(args) => untrack::cmd_file_untrack(ui, command, args),
    }
}
