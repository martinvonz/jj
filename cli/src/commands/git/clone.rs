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
use std::path::{Path, PathBuf};
use std::{fs, io};

use jj_lib::git::{self, sanitize_git_url_if_path, GitFetchError, GitFetchStats};
use jj_lib::repo::Repo;
use jj_lib::str_util::StringPattern;
use jj_lib::workspace::Workspace;

use crate::cli_util::{CommandHelper, WorkspaceCommandHelper};
use crate::command_error::{cli_error, user_error, user_error_with_message, CommandError};
use crate::commands::git::{map_git_error, maybe_add_gitignore};
use crate::config::{write_config_value_to_file, ConfigNamePathBuf};
use crate::git_util::{get_git_repo, print_git_import_stats, with_remote_git_callbacks};
use crate::ui::Ui;

/// Create a new repo backed by a clone of a Git repo
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(clap::Args, Clone, Debug)]
pub struct GitCloneArgs {
    /// URL or path of the Git repo to clone
    #[arg(value_hint = clap::ValueHint::DirPath)]
    source: String,
    /// Specifies the target directory for the Jujutsu repository clone.
    /// If not provided, defaults to a directory named after the last component
    /// of the source URL. The full directory path will be created if it
    /// doesn't exist.
    #[arg(value_hint = clap::ValueHint::DirPath)]
    destination: Option<String>,
    /// Whether or not to colocate the Jujutsu repo with the git repo
    #[arg(long)]
    colocate: bool,
}

fn clone_destination_for_source(source: &str) -> Option<&str> {
    let destination = source.strip_suffix(".git").unwrap_or(source);
    let destination = destination.strip_suffix('/').unwrap_or(destination);
    destination
        .rsplit_once(&['/', '\\', ':'][..])
        .map(|(_, name)| name)
}

fn is_empty_dir(path: &Path) -> bool {
    if let Ok(mut entries) = path.read_dir() {
        entries.next().is_none()
    } else {
        false
    }
}

pub fn cmd_git_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitCloneArgs,
) -> Result<(), CommandError> {
    if command.global_args().at_operation.is_some() {
        return Err(cli_error("--at-op is not respected"));
    }
    let remote_name = "origin";
    let source = sanitize_git_url_if_path(command.cwd(), &args.source);
    let wc_path_str = args
        .destination
        .as_deref()
        .or_else(|| clone_destination_for_source(&source))
        .ok_or_else(|| user_error("No destination specified and wasn't able to guess it"))?;
    let wc_path = command.cwd().join(wc_path_str);

    let wc_path_existed = wc_path.exists();
    if wc_path_existed && !is_empty_dir(&wc_path) {
        return Err(user_error(
            "Destination path exists and is not an empty directory",
        ));
    }

    // will create a tree dir in case if was deleted after last check
    fs::create_dir_all(&wc_path)
        .map_err(|err| user_error_with_message(format!("Failed to create {wc_path_str}"), err))?;

    // Canonicalize because fs::remove_dir_all() doesn't seem to like e.g.
    // `/some/path/.`
    let canonical_wc_path: PathBuf = wc_path
        .canonicalize()
        .map_err(|err| user_error_with_message(format!("Failed to create {wc_path_str}"), err))?;
    let clone_result = do_git_clone(
        ui,
        command,
        args.colocate,
        remote_name,
        &source,
        &canonical_wc_path,
    );
    if clone_result.is_err() {
        let clean_up_dirs = || -> io::Result<()> {
            fs::remove_dir_all(canonical_wc_path.join(".jj"))?;
            if args.colocate {
                fs::remove_dir_all(canonical_wc_path.join(".git"))?;
            }
            if !wc_path_existed {
                fs::remove_dir(&canonical_wc_path)?;
            }
            Ok(())
        };
        if let Err(err) = clean_up_dirs() {
            writeln!(
                ui.warning_default(),
                "Failed to clean up {}: {}",
                canonical_wc_path.display(),
                err
            )
            .ok();
        }
    }

    let (mut workspace_command, stats) = clone_result?;
    if let Some(default_branch) = &stats.default_branch {
        // Set repository level `trunk()` alias to the default remote branch.
        let config_path = workspace_command.repo().repo_path().join("config.toml");
        write_config_value_to_file(
            &ConfigNamePathBuf::from_iter(["revset-aliases", "trunk()"]),
            format!("{default_branch}@{remote_name}").into(),
            &config_path,
        )?;
        writeln!(
            ui.status(),
            "Setting the revset alias \"trunk()\" to \"{default_branch}@{remote_name}\""
        )?;

        let default_branch_remote_ref = workspace_command
            .repo()
            .view()
            .get_remote_branch(default_branch, remote_name);
        if let Some(commit_id) = default_branch_remote_ref.target.as_normal().cloned() {
            let mut checkout_tx = workspace_command.start_transaction();
            // For convenience, create local branch as Git would do.
            checkout_tx
                .mut_repo()
                .track_remote_branch(default_branch, remote_name);
            if let Ok(commit) = checkout_tx.repo().store().get_commit(&commit_id) {
                checkout_tx.check_out(&commit)?;
            }
            checkout_tx.finish(ui, "check out git remote's default branch")?;
        }
    }
    Ok(())
}

fn do_git_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    colocate: bool,
    remote_name: &str,
    source: &str,
    wc_path: &Path,
) -> Result<(WorkspaceCommandHelper, GitFetchStats), CommandError> {
    let (workspace, repo) = if colocate {
        Workspace::init_colocated_git(command.settings(), wc_path)?
    } else {
        Workspace::init_internal_git(command.settings(), wc_path)?
    };
    let git_repo = get_git_repo(repo.store())?;
    writeln!(
        ui.status(),
        r#"Fetching into new repo in "{}""#,
        wc_path.display()
    )?;
    let mut workspace_command = command.for_workable_repo(ui, workspace, repo)?;
    maybe_add_gitignore(&workspace_command)?;
    git_repo.remote(remote_name, source).unwrap();
    let mut fetch_tx = workspace_command.start_transaction();

    let stats = with_remote_git_callbacks(ui, None, |cb| {
        git::fetch(
            fetch_tx.mut_repo(),
            &git_repo,
            remote_name,
            &[StringPattern::everything()],
            cb,
            &command.settings().git_settings(),
        )
    })
    .map_err(|err| match err {
        GitFetchError::NoSuchRemote(_) => {
            panic!("shouldn't happen as we just created the git remote")
        }
        GitFetchError::GitImportError(err) => CommandError::from(err),
        GitFetchError::InternalGitError(err) => map_git_error(err),
        GitFetchError::InvalidBranchPattern => {
            unreachable!("we didn't provide any globs")
        }
    })?;
    print_git_import_stats(ui, fetch_tx.repo(), &stats.import_stats, true)?;
    fetch_tx.finish(ui, "fetch from git remote into empty repo")?;
    Ok((workspace_command, stats))
}
