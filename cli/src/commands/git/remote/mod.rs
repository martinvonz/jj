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
pub mod set_url;

use clap::Subcommand;

use self::add::{cmd_git_remote_add, GitRemoteAddArgs};
use self::list::{cmd_git_remote_list, GitRemoteListArgs};
use self::remove::{cmd_git_remote_remove, GitRemoteRemoveArgs};
use self::rename::{cmd_git_remote_rename, GitRemoteRenameArgs};
use self::set_url::{cmd_git_remote_set_url, GitRemoteSetUrlArgs};
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Manage Git remotes
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(Subcommand, Clone, Debug)]
pub enum RemoteCommand {
    Add(GitRemoteAddArgs),
    List(GitRemoteListArgs),
    Remove(GitRemoteRemoveArgs),
    Rename(GitRemoteRenameArgs),
    SetUrl(GitRemoteSetUrlArgs),
}

pub fn cmd_git_remote(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &RemoteCommand,
) -> Result<(), CommandError> {
    match subcommand {
        RemoteCommand::Add(args) => cmd_git_remote_add(ui, command, args),
        RemoteCommand::List(args) => cmd_git_remote_list(ui, command, args),
        RemoteCommand::Remove(args) => cmd_git_remote_remove(ui, command, args),
        RemoteCommand::Rename(args) => cmd_git_remote_rename(ui, command, args),
        RemoteCommand::SetUrl(args) => cmd_git_remote_set_url(ui, command, args),
    }
}
