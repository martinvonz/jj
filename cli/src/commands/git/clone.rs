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

use jj_lib::git::{self, GitFetchError, GitFetchStats};
use jj_lib::repo::Repo;
use jj_lib::str_util::StringPattern;
use jj_lib::workspace::Workspace;

use crate::cli_util::{CommandHelper, WorkspaceCommandHelper};
use crate::command_error::{user_error, user_error_with_message, CommandError};
use crate::commands::git::{map_git_error, maybe_add_gitignore};
use crate::git_util::{get_git_repo, print_git_import_stats, with_remote_git_callbacks};
use crate::ui::Ui;

/// Create a new repo backed by a clone of a Git repo
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(clap::Args, Clone, Debug)]
pub struct CloneArgs {
    /// URL or path of the Git repo to clone
    #[arg(value_hint = clap::ValueHint::DirPath)]
    source: String,
    /// The directory to write the Jujutsu repo to
    #[arg(value_hint = clap::ValueHint::DirPath)]
    destination: Option<String>,
    /// Whether or not to colocate the Jujutsu repo with the git repo
    #[arg(long)]
    colocate: bool,
}

fn absolute_git_source(cwd: &Path, source: &str) -> String {
    // Git appears to turn URL-like source to absolute path if local git directory
    // exits, and fails because '$PWD/https' is unsupported protocol. Since it would
    // be tedious to copy the exact git (or libgit2) behavior, we simply assume a
    // source containing ':' is a URL, SSH remote, or absolute path with Windows
    // drive letter.
    if !source.contains(':') && Path::new(source).exists() {
        // It's less likely that cwd isn't utf-8, so just fall back to original source.
        cwd.join(source)
            .into_os_string()
            .into_string()
            .unwrap_or_else(|_| source.to_owned())
    } else {
        source.to_owned()
    }
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
    args: &CloneArgs,
) -> Result<(), CommandError> {
    let remote_name = "origin";
    let source = absolute_git_source(command.cwd(), &args.source);
    let wc_path_str = args
        .destination
        .as_deref()
        .or_else(|| clone_destination_for_source(&source))
        .ok_or_else(|| user_error("No destination specified and wasn't able to guess it"))?;
    let wc_path = command.cwd().join(wc_path_str);
    let wc_path_existed = match fs::create_dir(&wc_path) {
        Ok(()) => false,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => true,
        Err(err) => {
            return Err(user_error_with_message(
                format!("Failed to create {wc_path_str}"),
                err,
            ));
        }
    };
    if wc_path_existed && !is_empty_dir(&wc_path) {
        return Err(user_error(
            "Destination path exists and is not an empty directory",
        ));
    }

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
    let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
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
