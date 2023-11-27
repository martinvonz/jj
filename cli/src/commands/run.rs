// Copyright 2020 The Jujutsu Authors
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

//! This file contains the internal implementation of `run`.

use crate::cli_util::{
    resolve_multiple_nonempty_revsets, user_error, CommandError, CommandHelper, RevisionArg,
};
use crate::ui::Ui;

/// Run a command across a set of revisions.
///
///
/// All recorded state will be persisted in the `.jj` directory, so occasionally
/// a `jj run --clean` is needed to clean up disk space.
///
/// # Example
///
/// # Run pre-commit on your local work
/// $ jj run 'pre-commit run .github/pre-commit.yaml' -r (trunk()..@) -j 4
///
/// This allows pre-commit integration and other funny stuff.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub struct RunArgs {
    /// The command to run across all selected revisions.
    shell_command: String,
    /// The revisions to change.
    #[arg(long, short, default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// A no-op option to match the interface of `git rebase -x`.
    #[arg(short = 'x', hide = true)]
    unused_command: bool,
    /// How many processes should run in parallel, uses by default all cores.
    #[arg(long, short)]
    jobs: Option<usize>,
}

pub fn cmd_run(ui: &mut Ui, command: &CommandHelper, args: &RunArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let _resolved_commits =
        resolve_multiple_nonempty_revsets(&args.revisions, &workspace_command, ui)?;
    // Jobs are resolved in this order:
    // 1. Commandline argument iff > 0.
    // 2. the amount of cores available.
    // 3. a single job, if all of the above fails.
    let _jobs = match args.jobs {
        Some(0) => return Err(user_error("must pass at least one job")),
        Some(jobs) => Some(jobs),
        None => std::thread::available_parallelism().map(|t| t.into()).ok(),
    }
    // Fallback to a single user-visible job.
    .unwrap_or(1usize);
    Err(user_error("This is a stub, do not use"))
}
