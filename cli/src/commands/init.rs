// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;

use clap::ArgGroup;
use jj_lib::file_util;
use jj_lib::workspace::Workspace;
use tracing::instrument;

use super::git;
use crate::cli_util::{user_error_with_hint, user_error_with_message, CommandError, CommandHelper};
use crate::ui::Ui;

/// Create a new repo in the given directory
///
/// If the given directory does not exist, it will be created. If no directory
/// is given, the current directory is used.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("backend").args(&["git", "git_repo"])))]
pub(crate) struct InitArgs {
    /// The destination directory
    #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
    destination: String,
    /// DEPRECATED: Use `jj git init`
    /// Use the Git backend, creating a jj repo backed by a Git repo
    #[arg(long, hide = true)]
    git: bool,
    /// DEPRECATED: Use `jj git init`
    /// Path to a git repo the jj repo will be backed by
    #[arg(long, hide = true, value_hint = clap::ValueHint::DirPath)]
    git_repo: Option<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_init(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &InitArgs,
) -> Result<(), CommandError> {
    let cwd = command.cwd().canonicalize().unwrap();
    let wc_path = cwd.join(&args.destination);
    let wc_path = file_util::create_or_reuse_dir(&wc_path)
        .and_then(|_| wc_path.canonicalize())
        .map_err(|e| user_error_with_message("Failed to create workspace", e))?;

    // Preserve existing behaviour where `jj init` is not able to create
    // a colocated repo.
    let colocate = false;
    if args.git || args.git_repo.is_some() {
        git::git_init(ui, command, &wc_path, colocate, args.git_repo.as_deref())?;
        writeln!(
            ui.warning(),
            "warning: `--git` and `--git-repo` are deprecated.
Use `jj git init` instead"
        )?;
    } else {
        if !command.settings().allow_native_backend() {
            return Err(user_error_with_hint(
                "The native backend is disallowed by default.",
                "Did you mean to call `jj git init`?
Set `ui.allow-init-native` to allow initializing a repo with the native backend.",
            ));
        }
        Workspace::init_local(command.settings(), &wc_path)?;
    }

    let relative_wc_path = file_util::relative_path(&cwd, &wc_path);
    writeln!(
        ui.stderr(),
        "Initialized repo in \"{}\"",
        relative_wc_path.display()
    )?;
    Ok(())
}
