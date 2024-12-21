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

use std::path::Path;

use clap::Subcommand;
use jj_lib::config::ConfigFile;
use jj_lib::config::ConfigSource;

use self::clone::cmd_git_clone;
use self::clone::GitCloneArgs;
use self::export::cmd_git_export;
use self::export::GitExportArgs;
use self::fetch::cmd_git_fetch;
use self::fetch::GitFetchArgs;
use self::import::cmd_git_import;
use self::import::GitImportArgs;
use self::init::cmd_git_init;
use self::init::GitInitArgs;
use self::push::cmd_git_push;
use self::push::GitPushArgs;
use self::remote::cmd_git_remote;
use self::remote::RemoteCommand;
use self::submodule::cmd_git_submodule;
use self::submodule::GitSubmoduleCommand;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::user_error_with_message;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Commands for working with Git remotes and the underlying Git repo
///
/// For a comparison with Git, including a table of commands, see
/// https://jj-vcs.github.io/jj/latest/git-comparison/.
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

/// Sets repository level `trunk()` alias to the specified remote branch.
fn write_repository_level_trunk_alias(
    ui: &Ui,
    repo_path: &Path,
    remote: &str,
    branch: &str,
) -> Result<(), CommandError> {
    let mut file = ConfigFile::load_or_empty(ConfigSource::Repo, repo_path.join("config.toml"))?;
    file.set_value(["revset-aliases", "trunk()"], format!("{branch}@{remote}"))
        .expect("initial repo config shouldn't have invalid values");
    file.save()?;
    writeln!(
        ui.status(),
        r#"Setting the revset alias "trunk()" to "{branch}@{remote}""#,
    )?;
    Ok(())
}
