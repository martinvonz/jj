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

use self::add::{cmd_remote_add, AddArgs};
use self::list::{cmd_remote_list, ListArgs};
use self::remove::{cmd_remote_remove, RemoveArgs};
use self::rename::{cmd_remote_rename, RenameArgs};
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Manage Git remotes
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(Subcommand, Clone, Debug)]
pub enum RemoteCommand {
    Add(AddArgs),
    List(ListArgs),
    Remove(RemoveArgs),
    Rename(RenameArgs),
}

pub fn cmd_git_remote(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &RemoteCommand,
) -> Result<(), CommandError> {
    match subcommand {
        RemoteCommand::Add(args) => cmd_remote_add(ui, command, args),
        RemoteCommand::List(args) => cmd_remote_list(ui, command, args),
        RemoteCommand::Remove(args) => cmd_remote_remove(ui, command, args),
        RemoteCommand::Rename(args) => cmd_remote_rename(ui, command, args),
    }
}
