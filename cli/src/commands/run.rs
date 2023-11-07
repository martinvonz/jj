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

use std::num::NonZeroUsize;

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
    /// Multiple revsets are accepted and the work will be done on a
    /// intersection of them.
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
    let _jobs = if let Some(_jobs) = args.jobs {
        _jobs
    } else {
        // Use all available cores

        // SAFETY:
        // We use a internal constant of 4 threads, if it fails
        let available =
            std::thread::available_parallelism().unwrap_or(NonZeroUsize::new(4).unwrap());
        available.into()
    };
    Err(user_error("This is a stub, do not use"))
}
