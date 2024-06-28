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

use itertools::Itertools;
use jj_lib::git::{self, GitFetchError};
use jj_lib::repo::Repo;
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::str_util::StringPattern;

use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, user_error_with_hint, CommandError};
use crate::commands::git::{get_single_remote, map_git_error};
use crate::git_util::{get_git_repo, print_git_import_stats, with_remote_git_callbacks};
use crate::ui::Ui;

/// Fetch from a Git remote
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
pub struct GitFetchArgs {
    /// Fetch only some of the branches
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// expand `*` as a glob. The other wildcard characters aren't supported.
    #[arg(long, short, default_value = "glob:*", value_parser = StringPattern::parse)]
    branch: Vec<StringPattern>,
    /// The remote to fetch from (only named remotes are supported, can be
    /// repeated)
    #[arg(long = "remote", value_name = "remote")]
    remotes: Vec<String>,
    /// Fetch from all remotes
    #[arg(long, conflicts_with = "remotes")]
    all_remotes: bool,
}

#[tracing::instrument(skip(ui, command))]
pub fn cmd_git_fetch(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitFetchArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let git_repo = get_git_repo(workspace_command.repo().store())?;
    let remotes = if args.all_remotes {
        get_all_remotes(&git_repo)?
    } else if args.remotes.is_empty() {
        get_default_fetch_remotes(ui, command.settings(), &git_repo)?
    } else {
        args.remotes.clone()
    };
    let mut tx = workspace_command.start_transaction();
    for remote in &remotes {
        let stats = with_remote_git_callbacks(ui, None, |cb| {
            git::fetch(
                tx.mut_repo(),
                &git_repo,
                remote,
                &args.branch,
                cb,
                &command.settings().git_settings(),
            )
        })
        .map_err(|err| match err {
            GitFetchError::InvalidBranchPattern => {
                if args
                    .branch
                    .iter()
                    .any(|pattern| pattern.as_exact().map_or(false, |s| s.contains('*')))
                {
                    user_error_with_hint(
                        err,
                        "Prefix the pattern with `glob:` to expand `*` as a glob",
                    )
                } else {
                    user_error(err)
                }
            }
            GitFetchError::GitImportError(err) => err.into(),
            GitFetchError::InternalGitError(err) => map_git_error(err),
            _ => user_error(err),
        })?;
        print_git_import_stats(ui, tx.repo(), &stats.import_stats, true)?;
    }
    tx.finish(
        ui,
        format!("fetch from git remote(s) {}", remotes.iter().join(",")),
    )?;
    Ok(())
}

const DEFAULT_REMOTE: &str = "origin";

fn get_default_fetch_remotes(
    ui: &Ui,
    settings: &UserSettings,
    git_repo: &git2::Repository,
) -> Result<Vec<String>, CommandError> {
    const KEY: &str = "git.fetch";
    if let Ok(remotes) = settings.config().get(KEY) {
        Ok(remotes)
    } else if let Some(remote) = settings.config().get_string(KEY).optional()? {
        Ok(vec![remote])
    } else if let Some(remote) = get_single_remote(git_repo)? {
        // if nothing was explicitly configured, try to guess
        if remote != DEFAULT_REMOTE {
            writeln!(
                ui.hint_default(),
                "Fetching from the only existing remote: {remote}"
            )?;
        }
        Ok(vec![remote])
    } else {
        Ok(vec![DEFAULT_REMOTE.to_owned()])
    }
}

fn get_all_remotes(git_repo: &git2::Repository) -> Result<Vec<String>, CommandError> {
    let git_remotes = git_repo.remotes()?;
    Ok(git_remotes
        .iter()
        .filter_map(|x| x.map(ToOwned::to_owned))
        .collect())
}
