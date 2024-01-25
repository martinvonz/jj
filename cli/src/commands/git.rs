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

use std::collections::HashSet;
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::{fmt, fs, io};

use clap::{ArgGroup, Subcommand};
use itertools::Itertools;
use jj_lib::backend::TreeValue;
use jj_lib::git::{
    self, parse_gitmodules, GitBranchPushTargets, GitFetchError, GitFetchStats, GitPushError,
};
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::RefTarget;
use jj_lib::refs::{
    classify_branch_push_action, BranchPushAction, BranchPushUpdate, TrackingRefPair,
};
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::revset::{self, RevsetExpression, RevsetIteratorExt as _};
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::str_util::StringPattern;
use jj_lib::view::View;
use jj_lib::workspace::Workspace;
use maplit::hashset;

use crate::cli_util::{
    parse_string_pattern, resolve_multiple_nonempty_revsets, short_change_hash, short_commit_hash,
    user_error, user_error_with_hint, CommandError, CommandHelper, RevisionArg,
    WorkspaceCommandHelper,
};
use crate::git_util::{
    get_git_repo, print_failed_git_export, print_git_import_stats, with_remote_git_callbacks,
};
use crate::ui::Ui;

/// Commands for working with the underlying Git repo
///
/// For a comparison with Git, including a table of commands, see
/// https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md.
#[derive(Subcommand, Clone, Debug)]
pub enum GitCommand {
    #[command(subcommand)]
    Remote(GitRemoteCommand),
    Fetch(GitFetchArgs),
    Clone(GitCloneArgs),
    Push(GitPushArgs),
    Import(GitImportArgs),
    Export(GitExportArgs),
    #[command(subcommand, hide = true)]
    Submodule(GitSubmoduleCommand),
}

/// Manage Git remotes
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(Subcommand, Clone, Debug)]
pub enum GitRemoteCommand {
    Add(GitRemoteAddArgs),
    Remove(GitRemoteRemoveArgs),
    Rename(GitRemoteRenameArgs),
    List(GitRemoteListArgs),
}

/// Add a Git remote
#[derive(clap::Args, Clone, Debug)]
pub struct GitRemoteAddArgs {
    /// The remote's name
    remote: String,
    /// The remote's URL
    url: String,
}

/// Remove a Git remote and forget its branches
#[derive(clap::Args, Clone, Debug)]
pub struct GitRemoteRemoveArgs {
    /// The remote's name
    remote: String,
}

/// Rename a Git remote
#[derive(clap::Args, Clone, Debug)]
pub struct GitRemoteRenameArgs {
    /// The name of an existing remote
    old: String,
    /// The desired name for `old`
    new: String,
}

/// List Git remotes
#[derive(clap::Args, Clone, Debug)]
pub struct GitRemoteListArgs {}

/// Fetch from a Git remote
#[derive(clap::Args, Clone, Debug)]
pub struct GitFetchArgs {
    /// Fetch only some of the branches
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// expand `*` as a glob. The other wildcard characters aren't supported.
    #[arg(long, default_value = "glob:*", value_parser = parse_string_pattern)]
    branch: Vec<StringPattern>,
    /// The remote to fetch from (only named remotes are supported, can be
    /// repeated)
    #[arg(long = "remote", value_name = "remote")]
    remotes: Vec<String>,
    /// Fetch from all remotes
    #[arg(long, conflicts_with = "remotes")]
    all_remotes: bool,
}

/// Create a new repo backed by a clone of a Git repo
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(clap::Args, Clone, Debug)]
pub struct GitCloneArgs {
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

/// Push to a Git remote
///
/// By default, pushes any branches pointing to
/// `remote_branches(remote=<remote>)..@`. Use `--branch` to push specific
/// branches. Use `--all` to push all branches. Use `--change` to generate
/// branch names based on the change IDs of specific commits.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("specific").args(&["branch", "change", "revisions"]).multiple(true)))]
#[command(group(ArgGroup::new("what").args(&["all", "deleted"]).conflicts_with("specific")))]
pub struct GitPushArgs {
    /// The remote to push to (only named remotes are supported)
    #[arg(long)]
    remote: Option<String>,
    /// Push only this branch (can be repeated)
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(long, short, value_parser = parse_string_pattern)]
    branch: Vec<StringPattern>,
    /// Push all branches (including deleted branches)
    #[arg(long)]
    all: bool,
    /// Push all deleted branches
    #[arg(long)]
    deleted: bool,
    /// Push branches pointing to these commits
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,
    /// Push this commit by creating a branch based on its change ID (can be
    /// repeated)
    #[arg(long, short)]
    change: Vec<RevisionArg>,
    /// Only display what will change on the remote
    #[arg(long)]
    dry_run: bool,
}

/// Update repo with changes made in the underlying Git repo
#[derive(clap::Args, Clone, Debug)]
pub struct GitImportArgs {}

/// Update the underlying Git repo with changes made in the repo
#[derive(clap::Args, Clone, Debug)]
pub struct GitExportArgs {}

/// FOR INTERNAL USE ONLY Interact with git submodules
#[derive(Subcommand, Clone, Debug)]
pub enum GitSubmoduleCommand {
    /// Print the relevant contents from .gitmodules. For debugging purposes
    /// only.
    PrintGitmodules(GitSubmodulePrintGitmodulesArgs),
}

/// Print debugging info about Git submodules
#[derive(clap::Args, Clone, Debug)]
#[command(hide = true)]
pub struct GitSubmodulePrintGitmodulesArgs {
    /// Read .gitmodules from the given revision.
    #[arg(long, short = 'r', default_value = "@")]
    revisions: RevisionArg,
}

fn make_branch_term(branch_names: &[impl fmt::Display]) -> String {
    match branch_names {
        [branch_name] => format!("branch {}", branch_name),
        branch_names => format!("branches {}", branch_names.iter().join(", ")),
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
                "Jujutsu uses libssh2, which doesn't respect ~/.ssh/config. Does `ssh -F \
                 /dev/null` to the host work?"
            };

        user_error_with_hint(err.to_string(), hint)
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
        .map_err(|e| user_error(format!("Failed to write .jj/.gitignore file: {e}")))
    } else {
        Ok(())
    }
}

fn cmd_git_remote_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitRemoteAddArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    git::add_remote(&git_repo, &args.remote, &args.url)?;
    Ok(())
}

fn cmd_git_remote_remove(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitRemoteRemoveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = workspace_command.start_transaction();
    git::remove_remote(tx.mut_repo(), &git_repo, &args.remote)?;
    if tx.mut_repo().has_changes() {
        tx.finish(ui, format!("remove git remote {}", &args.remote))
    } else {
        Ok(()) // Do not print "Nothing changed."
    }
}

fn cmd_git_remote_rename(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitRemoteRenameArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = workspace_command.start_transaction();
    git::rename_remote(tx.mut_repo(), &git_repo, &args.old, &args.new)?;
    if tx.mut_repo().has_changes() {
        tx.finish(
            ui,
            format!("rename git remote {} to {}", &args.old, &args.new),
        )
    } else {
        Ok(()) // Do not print "Nothing changed."
    }
}

fn cmd_git_remote_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitRemoteListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    for remote_name in git_repo.remotes()?.iter().flatten() {
        let remote = git_repo.find_remote(remote_name)?;
        writeln!(
            ui.stdout(),
            "{} {}",
            remote_name,
            remote.url().unwrap_or("<no URL>")
        )?;
    }
    Ok(())
}

#[tracing::instrument(skip(ui, command))]
fn cmd_git_fetch(
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
        let stats = with_remote_git_callbacks(ui, |cb| {
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
                        err.to_string(),
                        "Prefix the pattern with `glob:` to expand `*` as a glob",
                    )
                } else {
                    user_error(err.to_string())
                }
            }
            GitFetchError::GitImportError(err) => err.into(),
            GitFetchError::InternalGitError(err) => map_git_error(err),
            _ => user_error(err.to_string()),
        })?;
        print_git_import_stats(ui, &stats.import_stats)?;
    }
    tx.finish(
        ui,
        format!("fetch from git remote(s) {}", remotes.iter().join(",")),
    )?;
    Ok(())
}

fn get_single_remote(git_repo: &git2::Repository) -> Result<Option<String>, CommandError> {
    let git_remotes = git_repo.remotes()?;
    Ok(match git_remotes.len() {
        1 => git_remotes.get(0).map(ToOwned::to_owned),
        _ => None,
    })
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
                ui.hint(),
                "Fetching from the only existing remote: {}",
                remote
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

fn cmd_git_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitCloneArgs,
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
        Err(err) => return Err(user_error(format!("Failed to create {wc_path_str}: {err}"))),
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
        .map_err(|err| user_error(format!("Failed to create {wc_path_str}: {err}")))?;
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
                ui.warning(),
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
        ui.stderr(),
        r#"Fetching into new repo in "{}""#,
        wc_path.display()
    )?;
    let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
    maybe_add_gitignore(&workspace_command)?;
    git_repo.remote(remote_name, source).unwrap();
    let mut fetch_tx = workspace_command.start_transaction();

    let stats = with_remote_git_callbacks(ui, |cb| {
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
    print_git_import_stats(ui, &stats.import_stats)?;
    fetch_tx.finish(ui, "fetch from git remote into empty repo")?;
    Ok((workspace_command, stats))
}

fn cmd_git_push(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitPushArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let git_repo = get_git_repo(workspace_command.repo().store())?;

    let remote = if let Some(name) = &args.remote {
        name.clone()
    } else {
        get_default_push_remote(ui, command.settings(), &git_repo)?
    };

    let repo = workspace_command.repo().clone();
    let wc_commit_id = workspace_command.get_wc_commit_id().cloned();
    let change_commits: Vec<_> = args
        .change
        .iter()
        .map(|change_str| workspace_command.resolve_single_rev(change_str, ui))
        .try_collect()?;

    let mut tx = workspace_command.start_transaction();
    let tx_description;
    let mut branch_updates = vec![];
    if args.all {
        for (branch_name, targets) in repo.view().local_remote_branches(&remote) {
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }
        tx_description = format!("push all branches to git remote {remote}");
    } else if args.deleted {
        for (branch_name, targets) in repo.view().local_remote_branches(&remote) {
            if targets.local_target.is_present() {
                continue;
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }
        tx_description = format!("push all deleted branches to git remote {remote}");
    } else {
        let mut seen_branches = hashset! {};
        let branches_by_name =
            find_branches_to_push(repo.view(), &args.branch, &remote, &mut seen_branches)?;
        for (branch_name, targets) in branches_by_name {
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => writeln!(
                    ui.stderr(),
                    "Branch {branch_name}@{remote} already matches {branch_name}",
                )?,
                Err(reason) => return Err(reason.into()),
            }
        }

        for (change_str, commit) in std::iter::zip(args.change.iter(), change_commits) {
            let mut branch_name = format!(
                "{}{}",
                command.settings().push_branch_prefix(),
                commit.change_id().hex()
            );
            if !seen_branches.insert(branch_name.clone()) {
                continue;
            }
            let view = tx.base_repo().view();
            if view.get_local_branch(&branch_name).is_absent() {
                // A local branch with the full change ID doesn't exist already, so use the
                // short ID if it's not ambiguous (which it shouldn't be most of the time).
                let short_change_id = short_change_hash(commit.change_id());
                if tx
                    .base_workspace_helper()
                    .resolve_single_rev(&short_change_id, ui)
                    .is_ok()
                {
                    // Short change ID is not ambiguous, so update the branch name to use it.
                    branch_name = format!(
                        "{}{}",
                        command.settings().push_branch_prefix(),
                        short_change_id
                    );
                };
            }
            if view.get_local_branch(&branch_name).is_absent() {
                writeln!(
                    ui.stderr(),
                    "Creating branch {} for revision {}",
                    branch_name,
                    change_str.deref()
                )?;
            }
            tx.mut_repo()
                .set_local_branch_target(&branch_name, RefTarget::normal(commit.id().clone()));
            let targets = TrackingRefPair {
                local_target: tx.repo().view().get_local_branch(&branch_name),
                remote_ref: tx.repo().view().get_remote_branch(&branch_name, &remote),
            };
            match classify_branch_update(&branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.clone(), update)),
                Ok(None) => writeln!(
                    ui.stderr(),
                    "Branch {branch_name}@{remote} already matches {branch_name}",
                )?,
                Err(reason) => return Err(reason.into()),
            }
        }

        let use_default_revset =
            args.branch.is_empty() && args.change.is_empty() && args.revisions.is_empty();
        let revision_commit_ids: HashSet<_> = if use_default_revset {
            let Some(wc_commit_id) = wc_commit_id else {
                return Err(user_error("Nothing checked out in this workspace"));
            };
            let current_branches_expression = RevsetExpression::remote_branches(
                StringPattern::everything(),
                StringPattern::Exact(remote.to_owned()),
            )
            .range(&RevsetExpression::commit(wc_commit_id))
            .intersection(&RevsetExpression::branches(StringPattern::everything()));
            let current_branches_revset = tx
                .base_workspace_helper()
                .evaluate_revset(current_branches_expression)?;
            current_branches_revset.iter().collect()
        } else {
            // TODO: Narrow search space to local target commits.
            // TODO: Remove redundant CommitId -> Commit -> CommitId round trip.
            resolve_multiple_nonempty_revsets(&args.revisions, tx.base_workspace_helper(), ui)?
                .iter()
                .map(|commit| commit.id().clone())
                .collect()
        };
        let branches_targeted = repo
            .view()
            .local_remote_branches(&remote)
            .filter(|(_, targets)| {
                let mut local_ids = targets.local_target.added_ids();
                local_ids.any(|id| revision_commit_ids.contains(id))
            })
            .collect_vec();
        for &(branch_name, targets) in &branches_targeted {
            if !seen_branches.insert(branch_name.to_owned()) {
                continue;
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }
        if (!args.revisions.is_empty() || use_default_revset) && branches_targeted.is_empty() {
            writeln!(
                ui.warning(),
                "No branches point to the specified revisions."
            )?;
        }

        tx_description = format!(
            "push {} to git remote {}",
            make_branch_term(
                &branch_updates
                    .iter()
                    .map(|(branch, _)| branch.as_str())
                    .collect_vec()
            ),
            &remote
        );
    }
    if branch_updates.is_empty() {
        writeln!(ui.stderr(), "Nothing changed.")?;
        return Ok(());
    }

    let mut new_heads = vec![];
    let mut force_pushed_branches = hashset! {};
    for (branch_name, update) in &branch_updates {
        if let Some(new_target) = &update.new_target {
            new_heads.push(new_target.clone());
            let force = match &update.old_target {
                None => false,
                Some(old_target) => !repo.index().is_ancestor(old_target, new_target),
            };
            if force {
                force_pushed_branches.insert(branch_name.to_string());
            }
        }
    }

    // Check if there are conflicts in any commits we're about to push that haven't
    // already been pushed.
    let mut old_heads = repo
        .view()
        .remote_branches(&remote)
        .flat_map(|(_, old_head)| old_head.target.added_ids())
        .cloned()
        .collect_vec();
    if old_heads.is_empty() {
        old_heads.push(repo.store().root_commit_id().clone());
    }
    for commit in revset::walk_revs(repo.as_ref(), &new_heads, &old_heads)?
        .iter()
        .commits(repo.store())
    {
        let commit = commit?;
        let mut reasons = vec![];
        if commit.description().is_empty() {
            reasons.push("it has no description");
        }
        if commit.author().name.is_empty()
            || commit.author().name == UserSettings::USER_NAME_PLACEHOLDER
            || commit.author().email.is_empty()
            || commit.author().email == UserSettings::USER_EMAIL_PLACEHOLDER
            || commit.committer().name.is_empty()
            || commit.committer().name == UserSettings::USER_NAME_PLACEHOLDER
            || commit.committer().email.is_empty()
            || commit.committer().email == UserSettings::USER_EMAIL_PLACEHOLDER
        {
            reasons.push("it has no author and/or committer set");
        }
        if commit.has_conflict()? {
            reasons.push("it has conflicts");
        }
        if !reasons.is_empty() {
            return Err(user_error(format!(
                "Won't push commit {} since {}",
                short_commit_hash(commit.id()),
                reasons.join(" and ")
            )));
        }
    }

    writeln!(ui.stderr(), "Branch changes to push to {}:", &remote)?;
    for (branch_name, update) in &branch_updates {
        match (&update.old_target, &update.new_target) {
            (Some(old_target), Some(new_target)) => {
                if force_pushed_branches.contains(branch_name) {
                    writeln!(
                        ui.stderr(),
                        "  Force branch {branch_name} from {} to {}",
                        short_commit_hash(old_target),
                        short_commit_hash(new_target)
                    )?;
                } else {
                    writeln!(
                        ui.stderr(),
                        "  Move branch {branch_name} from {} to {}",
                        short_commit_hash(old_target),
                        short_commit_hash(new_target)
                    )?;
                }
            }
            (Some(old_target), None) => {
                writeln!(
                    ui.stderr(),
                    "  Delete branch {branch_name} from {}",
                    short_commit_hash(old_target)
                )?;
            }
            (None, Some(new_target)) => {
                writeln!(
                    ui.stderr(),
                    "  Add branch {branch_name} to {}",
                    short_commit_hash(new_target)
                )?;
            }
            (None, None) => {
                panic!("Not pushing any change to branch {branch_name}");
            }
        }
    }

    if args.dry_run {
        writeln!(ui.stderr(), "Dry-run requested, not pushing.")?;
        return Ok(());
    }

    let targets = GitBranchPushTargets {
        branch_updates,
        force_pushed_branches,
    };
    with_remote_git_callbacks(ui, |cb| {
        git::push_branches(tx.mut_repo(), &git_repo, &remote, &targets, cb)
    })
    .map_err(|err| match err {
        GitPushError::InternalGitError(err) => map_git_error(err),
        GitPushError::NotFastForward => user_error_with_hint(
            "The push conflicts with changes made on the remote (it is not fast-forwardable).",
            "Try fetching from the remote, then make the branch point to where you want it to be, \
             and push again.",
        ),
        _ => user_error(err.to_string()),
    })?;
    tx.finish(ui, tx_description)?;
    Ok(())
}

fn get_default_push_remote(
    ui: &Ui,
    settings: &UserSettings,
    git_repo: &git2::Repository,
) -> Result<String, CommandError> {
    if let Some(remote) = settings.config().get_string("git.push").optional()? {
        Ok(remote)
    } else if let Some(remote) = get_single_remote(git_repo)? {
        // similar to get_default_fetch_remotes
        if remote != DEFAULT_REMOTE {
            writeln!(ui.hint(), "Pushing to the only existing remote: {}", remote)?;
        }
        Ok(remote)
    } else {
        Ok(DEFAULT_REMOTE.to_owned())
    }
}

#[derive(Clone, Debug)]
struct RejectedBranchUpdateReason {
    message: String,
    hint: Option<String>,
}

impl RejectedBranchUpdateReason {
    fn print(&self, ui: &Ui) -> io::Result<()> {
        writeln!(ui.warning(), "{}", self.message)?;
        if let Some(hint) = &self.hint {
            writeln!(ui.hint(), "Hint: {hint}")?;
        }
        Ok(())
    }
}

impl From<RejectedBranchUpdateReason> for CommandError {
    fn from(reason: RejectedBranchUpdateReason) -> Self {
        let RejectedBranchUpdateReason { message, hint } = reason;
        CommandError::UserError { message, hint }
    }
}

fn classify_branch_update(
    branch_name: &str,
    remote_name: &str,
    targets: TrackingRefPair,
) -> Result<Option<BranchPushUpdate>, RejectedBranchUpdateReason> {
    let push_action = classify_branch_push_action(targets);
    match push_action {
        BranchPushAction::AlreadyMatches => Ok(None),
        BranchPushAction::LocalConflicted => Err(RejectedBranchUpdateReason {
            message: format!("Branch {branch_name} is conflicted"),
            hint: Some(
                "Run `jj branch list` to inspect, and use `jj branch set` to fix it up.".to_owned(),
            ),
        }),
        BranchPushAction::RemoteConflicted => Err(RejectedBranchUpdateReason {
            message: format!("Branch {branch_name}@{remote_name} is conflicted"),
            hint: Some("Run `jj git fetch` to update the conflicted remote branch.".to_owned()),
        }),
        BranchPushAction::RemoteUntracked => Err(RejectedBranchUpdateReason {
            message: format!("Non-tracking remote branch {branch_name}@{remote_name} exists"),
            hint: Some(format!(
                "Run `jj branch track {branch_name}@{remote_name}` to import the remote branch."
            )),
        }),
        BranchPushAction::Update(update) => Ok(Some(update)),
    }
}

fn find_branches_to_push<'a>(
    view: &'a View,
    branch_patterns: &[StringPattern],
    remote_name: &str,
    seen_branches: &mut HashSet<String>,
) -> Result<Vec<(&'a str, TrackingRefPair<'a>)>, CommandError> {
    let mut matching_branches = vec![];
    let mut unmatched_patterns = vec![];
    for pattern in branch_patterns {
        let mut matches = view
            .local_remote_branches_matching(pattern, remote_name)
            .filter(|(_, targets)| {
                // If the remote exists but is not tracking, the absent local shouldn't
                // be considered a deleted branch.
                targets.local_target.is_present() || targets.remote_ref.is_tracking()
            })
            .peekable();
        if matches.peek().is_none() {
            unmatched_patterns.push(pattern);
        }
        matching_branches
            .extend(matches.filter(|&(name, _)| seen_branches.insert(name.to_owned())));
    }
    match &unmatched_patterns[..] {
        [] => Ok(matching_branches),
        [pattern] if pattern.is_exact() => Err(user_error(format!("No such branch: {pattern}"))),
        patterns => Err(user_error(format!(
            "No matching branches for patterns: {}",
            patterns.iter().join(", ")
        ))),
    }
}

fn cmd_git_import(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitImportArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut tx = workspace_command.start_transaction();
    // In non-colocated repo, HEAD@git will never be moved internally by jj.
    // That's why cmd_git_export() doesn't export the HEAD ref.
    git::import_head(tx.mut_repo())?;
    let stats = git::import_refs(tx.mut_repo(), &command.settings().git_settings())?;
    print_git_import_stats(ui, &stats)?;
    tx.finish(ui, "import git refs")?;
    Ok(())
}

fn cmd_git_export(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitExportArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut tx = workspace_command.start_transaction();
    let failed_branches = git::export_refs(tx.mut_repo())?;
    tx.finish(ui, "export git refs")?;
    print_failed_git_export(ui, &failed_branches)?;
    Ok(())
}

fn cmd_git_submodule_print_gitmodules(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitSubmodulePrintGitmodulesArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let commit = workspace_command.resolve_single_rev(&args.revisions, ui)?;
    let tree = commit.tree()?;
    let gitmodules_path = RepoPath::from_internal_string(".gitmodules");
    let mut gitmodules_file = match tree.path_value(gitmodules_path).into_resolved() {
        Ok(None) => {
            writeln!(ui.stderr(), "No submodules!")?;
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

pub fn cmd_git(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &GitCommand,
) -> Result<(), CommandError> {
    match subcommand {
        GitCommand::Fetch(args) => cmd_git_fetch(ui, command, args),
        GitCommand::Clone(args) => cmd_git_clone(ui, command, args),
        GitCommand::Remote(GitRemoteCommand::Add(args)) => cmd_git_remote_add(ui, command, args),
        GitCommand::Remote(GitRemoteCommand::Remove(args)) => {
            cmd_git_remote_remove(ui, command, args)
        }
        GitCommand::Remote(GitRemoteCommand::Rename(args)) => {
            cmd_git_remote_rename(ui, command, args)
        }
        GitCommand::Remote(GitRemoteCommand::List(args)) => cmd_git_remote_list(ui, command, args),
        GitCommand::Push(args) => cmd_git_push(ui, command, args),
        GitCommand::Import(args) => cmd_git_import(ui, command, args),
        GitCommand::Export(args) => cmd_git_export(ui, command, args),
        GitCommand::Submodule(GitSubmoduleCommand::PrintGitmodules(args)) => {
            cmd_git_submodule_print_gitmodules(ui, command, args)
        }
    }
}
