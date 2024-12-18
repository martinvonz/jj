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

use std::io::Write;

use clap::Subcommand;
use jj_lib::backend::TreeValue;
use jj_lib::git::parse_gitmodules;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// FOR INTERNAL USE ONLY Interact with git submodules
#[derive(Subcommand, Clone, Debug)]
pub enum GitSubmoduleCommand {
    /// Print the relevant contents from .gitmodules. For debugging purposes
    /// only.
    PrintGitmodules(PrintArgs),
}

pub fn cmd_git_submodule(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &GitSubmoduleCommand,
) -> Result<(), CommandError> {
    match subcommand {
        GitSubmoduleCommand::PrintGitmodules(args) => cmd_submodule_print(ui, command, args),
    }
}

// TODO: break everything below into a separate file as soon as there is more
// than one subcommand here.

/// Print debugging info about Git submodules
#[derive(clap::Args, Clone, Debug)]
#[command(hide = true)]
pub struct PrintArgs {
    /// Read .gitmodules from the given revision.
    #[arg(long, short = 'r', default_value = "@", value_name = "REVSET")]
    revisions: RevisionArg,
}

fn cmd_submodule_print(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PrintArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let commit = workspace_command.resolve_single_rev(ui, &args.revisions)?;
    let tree = commit.tree()?;
    let gitmodules_path = RepoPath::from_internal_string(".gitmodules");
    let mut gitmodules_file = match tree.path_value(gitmodules_path)?.into_resolved() {
        Ok(None) => {
            writeln!(ui.status(), "No submodules!")?;
            return Ok(());
        }
        Ok(Some(TreeValue::File { id, .. })) => repo.store().read_file(gitmodules_path, &id)?,
        _ => {
            return Err(user_error(".gitmodules is not a file."));
        }
    };

    let submodules = parse_gitmodules(&mut gitmodules_file)?;
    for (name, submodule) in submodules {
        writeln!(
            ui.stdout(),
            "name:{}\nurl:{}\npath:{}\n\n",
            name,
            submodule.url,
            submodule.path
        )?;
    }
    Ok(())
}
