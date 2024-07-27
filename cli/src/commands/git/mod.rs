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

pub mod clone;
pub mod export;
pub mod fetch;
pub mod import;
pub mod init;
pub mod push;
pub mod remote;
pub mod submodule;

use clap::Subcommand;

use self::clone::{cmd_git_clone, GitCloneArgs};
use self::export::{cmd_git_export, GitExportArgs};
use self::fetch::{cmd_git_fetch, GitFetchArgs};
use self::import::{cmd_git_import, GitImportArgs};
use self::init::{cmd_git_init, GitInitArgs};
use self::push::{cmd_git_push, GitPushArgs};
use self::remote::{cmd_git_remote, RemoteCommand};
use self::submodule::{cmd_git_submodule, GitSubmoduleCommand};
use crate::cli_util::{CommandHelper, WorkspaceCommandHelper};
use crate::command_error::{
    user_error, user_error_with_hint, user_error_with_message, CommandError,
};
use crate::ui::Ui;

/// Commands for working with Git remotes and the underlying Git repo
///
/// For a comparison with Git, including a table of commands, see
/// https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md.
#[derive(Subcommand, Clone, Debug)]
pub enum GitCommand {
    Clone(GitCloneArgs),
    Export(GitExportArgs),
    Fetch(GitFetchArgs),
    Import(GitImportArgs),
    Init(GitInitArgs),
    Push(GitPushArgs),
    #[command(subcommand)]
    Remote(RemoteCommand),
    #[command(subcommand, hide = true)]
    Submodule(GitSubmoduleCommand),
}

pub fn cmd_git(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &GitCommand,
) -> Result<(), CommandError> {
    match subcommand {
        GitCommand::Clone(args) => cmd_git_clone(ui, command, args),
        GitCommand::Export(args) => cmd_git_export(ui, command, args),
        GitCommand::Fetch(args) => cmd_git_fetch(ui, command, args),
        GitCommand::Import(args) => cmd_git_import(ui, command, args),
        GitCommand::Init(args) => cmd_git_init(ui, command, args),
        GitCommand::Push(args) => cmd_git_push(ui, command, args),
        GitCommand::Remote(args) => cmd_git_remote(ui, command, args),
        GitCommand::Submodule(args) => cmd_git_submodule(ui, command, args),
    }
}

fn map_git_error(err: git2::Error) -> CommandError {
    if err.class() == git2::ErrorClass::Ssh {
        let hint =
            if err.code() == git2::ErrorCode::Certificate && std::env::var_os("HOME").is_none() {
                "The HOME environment variable is not set, and might be required for Git to \
                 successfully load certificates. Try setting it to the path of a directory that \
                 contains a `.ssh` directory."
            } else {
                "There was an error creating an SSH connection. Does `ssh -F /dev/null` to the \
                 host work?"
            };

        user_error_with_hint(err, hint)
    } else {
        user_error(err.to_string())
    }
}

pub fn maybe_add_gitignore(workspace_command: &WorkspaceCommandHelper) -> Result<(), CommandError> {
    if workspace_command.working_copy_shared_with_git() {
        std::fs::write(
            workspace_command
                .workspace_root()
                .join(".jj")
                .join(".gitignore"),
            "/*\n",
        )
        .map_err(|e| user_error_with_message("Failed to write .jj/.gitignore file", e))
    } else {
        Ok(())
    }
}

fn get_single_remote(git_repo: &git2::Repository) -> Result<Option<String>, CommandError> {
    let git_remotes = git_repo.remotes()?;
    Ok(match git_remotes.len() {
        1 => git_remotes.get(0).map(ToOwned::to_owned),
        _ => None,
    })
}
