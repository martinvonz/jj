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
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use jj_lib::file_util;
use jj_lib::git;
use jj_lib::git::parse_git_ref;
use jj_lib::git::RefName;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::workspace::Workspace;

use super::write_repository_level_trunk_alias;
use crate::cli_util::print_trackable_remote_bookmarks;
use crate::cli_util::start_repo_transaction;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::cli_error;
use crate::command_error::user_error_with_hint;
use crate::command_error::user_error_with_message;
use crate::command_error::CommandError;
use crate::commands::git::maybe_add_gitignore;
use crate::git_util::get_git_repo;
use crate::git_util::is_colocated_git_workspace;
use crate::git_util::print_failed_git_export;
use crate::git_util::print_git_import_stats;
use crate::ui::Ui;

/// Create a new Git backed repo.
#[derive(clap::Args, Clone, Debug)]
pub struct GitInitArgs {
    /// The destination directory where the `jj` repo will be created.
    /// If the directory does not exist, it will be created.
    /// If no directory is given, the current directory is used.
    ///
    /// By default the `git` repo is under `$destination/.jj`
    #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
    destination: String,

    /// Specifies that the `jj` repo should also be a valid
    /// `git` repo, allowing the use of both `jj` and `git` commands
    /// in the same directory.
    ///
    /// This is done by placing the backing git repo into a `.git` directory in
    /// the root of the `jj` repo along with the `.jj` directory. If the `.git`
    /// directory already exists, all the existing commits will be imported.
    ///
    /// This option is mutually exclusive with `--git-repo`.
    #[arg(long, conflicts_with = "git_repo")]
    colocate: bool,

    /// Specifies a path to an **existing** git repository to be
    /// used as the backing git repo for the newly created `jj` repo.
    ///
    /// If the specified `--git-repo` path happens to be the same as
    /// the `jj` repo path (both .jj and .git directories are in the
    /// same working directory), then both `jj` and `git` commands
    /// will work on the same repo. This is called a co-located repo.
    ///
    /// This option is mutually exclusive with `--colocate`.
    #[arg(long, conflicts_with = "colocate", value_hint = clap::ValueHint::DirPath)]
    git_repo: Option<String>,
}

pub fn cmd_git_init(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitInitArgs,
) -> Result<(), CommandError> {
    if command.global_args().ignore_working_copy {
        return Err(cli_error("--ignore-working-copy is not respected"));
    }
    if command.global_args().at_operation.is_some() {
        return Err(cli_error("--at-op is not respected"));
    }
    let cwd = command.cwd();
    let wc_path = cwd.join(&args.destination);
    let wc_path = file_util::create_or_reuse_dir(&wc_path)
        .and_then(|_| dunce::canonicalize(wc_path))
        .map_err(|e| user_error_with_message("Failed to create workspace", e))?;

    do_init(
        ui,
        command,
        &wc_path,
        args.colocate,
        args.git_repo.as_deref(),
    )?;

    let relative_wc_path = file_util::relative_path(cwd, &wc_path);
    writeln!(
        ui.status(),
        r#"Initialized repo in "{}""#,
        relative_wc_path.display()
    )?;

    Ok(())
}

pub fn do_init(
    ui: &mut Ui,
    command: &CommandHelper,
    workspace_root: &Path,
    colocate: bool,
    git_repo: Option<&str>,
) -> Result<(), CommandError> {
    #[derive(Clone, Debug)]
    enum GitInitMode {
        Colocate,
        External(PathBuf),
        Internal,
    }

    let colocated_git_repo_path = workspace_root.join(".git");
    let init_mode = if colocate {
        if colocated_git_repo_path.exists() {
            GitInitMode::External(colocated_git_repo_path)
        } else {
            GitInitMode::Colocate
        }
    } else if let Some(path_str) = git_repo {
        let mut git_repo_path = command.cwd().join(path_str);
        if !git_repo_path.ends_with(".git") {
            git_repo_path.push(".git");
            // Undo if .git doesn't exist - likely a bare repo.
            if !git_repo_path.exists() {
                git_repo_path.pop();
            }
        }
        GitInitMode::External(git_repo_path)
    } else {
        if colocated_git_repo_path.exists() {
            return Err(user_error_with_hint(
                "Did not create a jj repo because there is an existing Git repo in this directory.",
                "To create a repo backed by the existing Git repo, run `jj git init --colocate` \
                 instead.",
            ));
        }
        GitInitMode::Internal
    };

    match &init_mode {
        GitInitMode::Colocate => {
            let (workspace, repo) =
                Workspace::init_colocated_git(command.settings(), workspace_root)?;
            let workspace_command = command.for_workable_repo(ui, workspace, repo)?;
            maybe_add_gitignore(&workspace_command)?;
        }
        GitInitMode::External(git_repo_path) => {
            let (workspace, repo) =
                Workspace::init_external_git(command.settings(), workspace_root, git_repo_path)?;
            // Import refs first so all the reachable commits are indexed in
            // chronological order.
            let colocated = is_colocated_git_workspace(&workspace, &repo);
            let repo = init_git_refs(ui, command, repo, colocated)?;
            let mut workspace_command = command.for_workable_repo(ui, workspace, repo)?;
            maybe_add_gitignore(&workspace_command)?;
            workspace_command.maybe_snapshot(ui)?;
            maybe_set_repository_level_trunk_alias(ui, &workspace_command)?;
            if !workspace_command.working_copy_shared_with_git() {
                let mut tx = workspace_command.start_transaction();
                jj_lib::git::import_head(tx.repo_mut())?;
                if let Some(git_head_id) = tx.repo().view().git_head().as_normal().cloned() {
                    let git_head_commit = tx.repo().store().get_commit(&git_head_id)?;
                    tx.check_out(&git_head_commit)?;
                }
                if tx.repo().has_changes() {
                    tx.finish(ui, "import git head")?;
                }
            }
            print_trackable_remote_bookmarks(ui, workspace_command.repo().view())?;
        }
        GitInitMode::Internal => {
            Workspace::init_internal_git(command.settings(), workspace_root)?;
        }
    }
    Ok(())
}

/// Imports branches and tags from the underlying Git repo, exports changes if
/// the repo is colocated.
///
/// This is similar to `WorkspaceCommandHelper::import_git_refs()`, but never
/// moves the Git HEAD to the working copy parent.
fn init_git_refs(
    ui: &mut Ui,
    command: &CommandHelper,
    repo: Arc<ReadonlyRepo>,
    colocated: bool,
) -> Result<Arc<ReadonlyRepo>, CommandError> {
    let mut tx = start_repo_transaction(&repo, command.settings(), command.string_args());
    // There should be no old refs to abandon, but enforce it.
    let mut git_settings = command.settings().git_settings()?;
    git_settings.abandon_unreachable_commits = false;
    let stats = git::import_some_refs(
        tx.repo_mut(),
        &git_settings,
        // Initial import shouldn't fail because of reserved remote name.
        |ref_name| !git::is_reserved_git_remote_ref(ref_name),
    )?;
    if !tx.repo().has_changes() {
        return Ok(repo);
    }
    print_git_import_stats(ui, tx.repo(), &stats, false)?;
    if colocated {
        // If git.auto-local-bookmark = true, local bookmarks could be created for
        // the imported remote branches.
        let failed_refs = git::export_refs(tx.repo_mut())?;
        print_failed_git_export(ui, &failed_refs)?;
    }
    let repo = tx.commit("import git refs")?;
    writeln!(
        ui.status(),
        "Done importing changes from the underlying Git repo."
    )?;
    Ok(repo)
}

// Set repository level `trunk()` alias to the default branch for "origin".
pub fn maybe_set_repository_level_trunk_alias(
    ui: &Ui,
    workspace_command: &WorkspaceCommandHelper,
) -> Result<(), CommandError> {
    let git_repo = get_git_repo(workspace_command.repo().store())?;
    if let Ok(reference) = git_repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(reference_name) = reference.symbolic_target() {
            if let Some(RefName::RemoteBranch { branch, .. }) = parse_git_ref(reference_name) {
                write_repository_level_trunk_alias(
                    ui,
                    workspace_command.repo_path(),
                    "origin",
                    &branch,
                )?;
            }
        };
    };

    Ok(())
}
