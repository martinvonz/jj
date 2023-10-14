use std::collections::HashSet;
use std::io::{Read, Seek as _, SeekFrom, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;
use std::{fs, io};

use clap::{ArgGroup, Subcommand};
use itertools::Itertools;
use jj_lib::backend::{ObjectId, TreeValue};
use jj_lib::git::{
    self, parse_gitmodules, GitBranchPushTargets, GitFetchError, GitFetchStats, GitPushError,
};
use jj_lib::git_backend::GitBackend;
use jj_lib::op_store::RefTarget;
use jj_lib::refs::{
    classify_branch_push_action, BranchPushAction, BranchPushUpdate, TrackingRefPair,
};
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::revset::{self, RevsetExpression, RevsetIteratorExt as _, StringPattern};
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::store::Store;
use jj_lib::workspace::Workspace;
use maplit::hashset;

use crate::cli_util::{
    print_failed_git_export, print_git_import_stats, resolve_multiple_nonempty_revsets,
    short_change_hash, short_commit_hash, user_error, user_error_with_hint, CommandError,
    CommandHelper, RevisionArg, WorkspaceCommandHelper,
};
use crate::commands::make_branch_term;
use crate::progress::Progress;
use crate::ui::Ui;

/// Commands for working with the underlying Git repo
///
/// For a comparison with Git, including a table of commands, see
/// https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md.
#[derive(Subcommand, Clone, Debug)]
pub enum GitCommands {
    #[command(subcommand)]
    Remote(GitRemoteCommands),
    Fetch(GitFetchArgs),
    Clone(GitCloneArgs),
    Push(GitPushArgs),
    Import(GitImportArgs),
    Export(GitExportArgs),
    #[command(subcommand, hide = true)]
    Submodule(GitSubmoduleCommands),
}

/// Manage Git remotes
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(Subcommand, Clone, Debug)]
pub enum GitRemoteCommands {
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
    /// Any `*` in the argument is expanded as a glob. So, one `--branch` can
    /// match several branches.
    #[arg(long)]
    branch: Vec<String>,
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
    #[arg(long, short)]
    branch: Vec<String>,
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
pub enum GitSubmoduleCommands {
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

fn get_git_repo(store: &Store) -> Result<git2::Repository, CommandError> {
    match store.backend_impl().downcast_ref::<GitBackend>() {
        None => Err(user_error("The repo is not backed by a git repo")),
        Some(git_backend) => Ok(git_backend.open_git_repo()?),
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

pub fn add_to_git_exclude(ui: &Ui, git_repo: &git2::Repository) -> Result<(), CommandError> {
    let exclude_file_path = git_repo.path().join("info").join("exclude");
    if exclude_file_path.exists() {
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&exclude_file_path)
        {
            Ok(mut exclude_file) => {
                let mut buf = vec![];
                exclude_file.read_to_end(&mut buf)?;
                let pattern = b"\n/.jj/\n";
                if !buf.windows(pattern.len()).any(|window| window == pattern) {
                    exclude_file.seek(SeekFrom::End(0))?;
                    if !buf.ends_with(b"\n") {
                        exclude_file.write_all(b"\n")?;
                    }
                    exclude_file.write_all(b"/.jj/\n")?;
                }
            }
            Err(err) => {
                writeln!(
                    ui.error(),
                    "Failed to add `.jj/` to {}: {}",
                    exclude_file_path.to_string_lossy(),
                    err
                )?;
            }
        }
    } else {
        writeln!(
            ui.error(),
            "Failed to add `.jj/` to {} because it doesn't exist",
            exclude_file_path.to_string_lossy()
        )?;
    }
    Ok(())
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
    let mut tx =
        workspace_command.start_transaction(&format!("remove git remote {}", &args.remote));
    git::remove_remote(tx.mut_repo(), &git_repo, &args.remote)?;
    if tx.mut_repo().has_changes() {
        tx.finish(ui)
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
    let mut tx = workspace_command
        .start_transaction(&format!("rename git remote {} to {}", &args.old, &args.new));
    git::rename_remote(tx.mut_repo(), &git_repo, &args.old, &args.new)?;
    if tx.mut_repo().has_changes() {
        tx.finish(ui)
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
    let mut tx = workspace_command.start_transaction(&format!(
        "fetch from git remote(s) {}",
        remotes.iter().join(",")
    ));
    let branches = args.branch.iter().map(|b| b.as_str()).collect_vec();
    for remote in remotes {
        let stats = with_remote_callbacks(ui, |cb| {
            git::fetch(
                tx.mut_repo(),
                &git_repo,
                &remote,
                (!branches.is_empty()).then_some(&*branches),
                cb,
                &command.settings().git_settings(),
            )
        })
        .map_err(|err| match err {
            GitFetchError::GitImportError(err) => err.into(),
            GitFetchError::InternalGitError(err) => map_git_error(err),
            _ => user_error(err.to_string()),
        })?;
        print_git_import_stats(ui, &stats.import_stats)?;
    }
    tx.finish(ui)?;
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
    if command.global_args().repository.is_some() {
        return Err(user_error("'--repository' cannot be used with 'git clone'"));
    }
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
            let mut checkout_tx =
                workspace_command.start_transaction("check out git remote's default branch");
            if let Ok(commit) = checkout_tx.repo().store().get_commit(&commit_id) {
                checkout_tx.check_out(&commit)?;
            }
            checkout_tx.finish(ui)?;
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
        let git_repo = git2::Repository::init(wc_path)?;
        add_to_git_exclude(ui, &git_repo)?;
        Workspace::init_external_git(command.settings(), wc_path, git_repo.path())?
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
    git_repo.remote(remote_name, source).unwrap();
    let mut fetch_tx = workspace_command.start_transaction("fetch from git remote into empty repo");

    let stats = with_remote_callbacks(ui, |cb| {
        git::fetch(
            fetch_tx.mut_repo(),
            &git_repo,
            remote_name,
            None,
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
        GitFetchError::InvalidGlob => {
            unreachable!("we didn't provide any globs")
        }
    })?;
    print_git_import_stats(ui, &stats.import_stats)?;
    fetch_tx.finish(ui)?;
    Ok((workspace_command, stats))
}

fn with_remote_callbacks<T>(ui: &mut Ui, f: impl FnOnce(git::RemoteCallbacks<'_>) -> T) -> T {
    let mut ui = Mutex::new(ui);
    let mut callback = None;
    if let Some(mut output) = ui.get_mut().unwrap().progress_output() {
        let mut progress = Progress::new(Instant::now());
        callback = Some(move |x: &git::Progress| {
            _ = progress.update(Instant::now(), x, &mut output);
        });
    }
    let mut callbacks = git::RemoteCallbacks::default();
    callbacks.progress = callback
        .as_mut()
        .map(|x| x as &mut dyn FnMut(&git::Progress));
    let mut get_ssh_keys = get_ssh_keys; // Coerce to unit fn type
    callbacks.get_ssh_keys = Some(&mut get_ssh_keys);
    let mut get_pw = |url: &str, _username: &str| {
        pinentry_get_pw(url).or_else(|| terminal_get_pw(*ui.lock().unwrap(), url))
    };
    callbacks.get_password = Some(&mut get_pw);
    let mut get_user_pw = |url: &str| {
        let ui = &mut *ui.lock().unwrap();
        Some((terminal_get_username(ui, url)?, terminal_get_pw(ui, url)?))
    };
    callbacks.get_username_password = Some(&mut get_user_pw);
    f(callbacks)
}

fn terminal_get_username(ui: &mut Ui, url: &str) -> Option<String> {
    ui.prompt(&format!("Username for {url}")).ok()
}

fn terminal_get_pw(ui: &mut Ui, url: &str) -> Option<String> {
    ui.prompt_password(&format!("Passphrase for {url}: ")).ok()
}

fn pinentry_get_pw(url: &str) -> Option<String> {
    let mut pinentry = Command::new("pinentry")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    #[rustfmt::skip]
    pinentry
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!(
                "SETTITLE jj passphrase\n\
                 SETDESC Enter passphrase for {url}\n\
                 SETPROMPT Passphrase:\n\
                 GETPIN\n"
            )
            .as_bytes(),
        )
        .ok()?;
    let mut out = String::new();
    pinentry
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .ok()?;
    _ = pinentry.wait();
    for line in out.split('\n') {
        if !line.starts_with("D ") {
            continue;
        }
        let (_, encoded) = line.split_at(2);
        return decode_assuan_data(encoded);
    }
    None
}

// https://www.gnupg.org/documentation/manuals/assuan/Server-responses.html#Server-responses
fn decode_assuan_data(encoded: &str) -> Option<String> {
    let encoded = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        if encoded[i] != b'%' {
            decoded.push(encoded[i]);
            i += 1;
            continue;
        }
        i += 1;
        let byte =
            u8::from_str_radix(std::str::from_utf8(encoded.get(i..i + 2)?).ok()?, 16).ok()?;
        decoded.push(byte);
        i += 2;
    }
    String::from_utf8(decoded).ok()
}

#[tracing::instrument]
fn get_ssh_keys(_username: &str) -> Vec<PathBuf> {
    let mut paths = vec![];
    if let Some(home_dir) = dirs::home_dir() {
        let ssh_dir = Path::new(&home_dir).join(".ssh");
        for filename in ["id_ed25519_sk", "id_ed25519", "id_rsa"] {
            let key_path = ssh_dir.join(filename);
            if key_path.is_file() {
                tracing::info!(path = ?key_path, "found ssh key");
                paths.push(key_path);
            }
        }
    }
    if paths.is_empty() {
        tracing::info!("no ssh key found");
    }
    paths
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

    let mut tx = workspace_command.start_transaction("");
    let tx_description;
    let mut branch_updates = vec![];
    if args.all {
        for (branch_name, targets) in repo.view().local_remote_branches(&remote) {
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(message) => writeln!(ui.warning(), "{message}")?,
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
                Err(message) => writeln!(ui.warning(), "{message}")?,
            }
        }
        tx_description = format!("push all deleted branches to git remote {remote}");
    } else {
        let mut seen_branches = hashset! {};
        for branch_name in &args.branch {
            if !seen_branches.insert(branch_name.clone()) {
                continue;
            }
            let targets = TrackingRefPair {
                local_target: repo.view().get_local_branch(branch_name),
                remote_target: &repo.view().get_remote_branch(branch_name, &remote).target,
            };
            if targets.local_target.is_absent() && targets.remote_target.is_absent() {
                return Err(user_error(format!("Branch {branch_name} doesn't exist")));
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.clone(), update)),
                Ok(None) => writeln!(
                    ui.stderr(),
                    "Branch {branch_name}@{remote} already matches {branch_name}",
                )?,
                Err(message) => return Err(user_error(message)),
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
                remote_target: &tx
                    .repo()
                    .view()
                    .get_remote_branch(&branch_name, &remote)
                    .target,
            };
            match classify_branch_update(&branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.clone(), update)),
                Ok(None) => writeln!(
                    ui.stderr(),
                    "Branch {branch_name}@{remote} already matches {branch_name}",
                )?,
                Err(message) => return Err(user_error(message)),
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
                Err(message) => writeln!(ui.warning(), "{message}")?,
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

    tx.set_description(&tx_description);

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
    with_remote_callbacks(ui, |cb| {
        git::push_branches(&git_repo, &remote, &targets, cb)
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
    // TODO: mark pushed remote branches as tracking
    let stats = git::import_refs(tx.mut_repo(), &git_repo, &command.settings().git_settings())?;
    print_git_import_stats(ui, &stats)?;
    tx.finish(ui)?;
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

fn classify_branch_update(
    branch_name: &str,
    remote_name: &str,
    targets: TrackingRefPair,
) -> Result<Option<BranchPushUpdate>, String> {
    let push_action = classify_branch_push_action(targets);
    match push_action {
        BranchPushAction::AlreadyMatches => Ok(None),
        BranchPushAction::LocalConflicted => Err(format!("Branch {branch_name} is conflicted")),
        BranchPushAction::RemoteConflicted => {
            Err(format!("Branch {branch_name}@{remote_name} is conflicted"))
        }
        BranchPushAction::Update(update) => Ok(Some(update)),
    }
}

fn cmd_git_import(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitImportArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = workspace_command.start_transaction("import git refs");
    let stats = git::import_refs(tx.mut_repo(), &git_repo, &command.settings().git_settings())?;
    print_git_import_stats(ui, &stats)?;
    tx.finish(ui)?;
    Ok(())
}

fn cmd_git_export(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitExportArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = workspace_command.start_transaction("export git refs");
    let failed_branches = git::export_refs(tx.mut_repo(), &git_repo)?;
    tx.finish(ui)?;
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
    let mut gitmodules_file = match tree.path_value(&gitmodules_path).into_resolved() {
        Ok(None) => {
            writeln!(ui.stderr(), "No submodules!")?;
            return Ok(());
        }
        Ok(Some(TreeValue::File { id, .. })) => repo.store().read_file(&gitmodules_path, &id)?,
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
    subcommand: &GitCommands,
) -> Result<(), CommandError> {
    match subcommand {
        GitCommands::Fetch(command_matches) => cmd_git_fetch(ui, command, command_matches),
        GitCommands::Clone(command_matches) => cmd_git_clone(ui, command, command_matches),
        GitCommands::Remote(GitRemoteCommands::Add(command_matches)) => {
            cmd_git_remote_add(ui, command, command_matches)
        }
        GitCommands::Remote(GitRemoteCommands::Remove(command_matches)) => {
            cmd_git_remote_remove(ui, command, command_matches)
        }
        GitCommands::Remote(GitRemoteCommands::Rename(command_matches)) => {
            cmd_git_remote_rename(ui, command, command_matches)
        }
        GitCommands::Remote(GitRemoteCommands::List(command_matches)) => {
            cmd_git_remote_list(ui, command, command_matches)
        }
        GitCommands::Push(command_matches) => cmd_git_push(ui, command, command_matches),
        GitCommands::Import(command_matches) => cmd_git_import(ui, command, command_matches),
        GitCommands::Export(command_matches) => cmd_git_export(ui, command, command_matches),
        GitCommands::Submodule(GitSubmoduleCommands::PrintGitmodules(command_matches)) => {
            cmd_git_submodule_print_gitmodules(ui, command, command_matches)
        }
    }
}
