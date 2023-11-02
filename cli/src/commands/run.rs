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

use crate::cli_util::{user_error, CommandError, CommandHelper, RevisionArg};
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
/// $ jj run 'pre-commit.py .github/pre-commit.yaml' -r (main..@) -j 4
///
/// This allows pre-commit integration and other funny stuff.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct RunArgs {
    /// The command to run across all selected revisions.
    #[arg(long, short, alias = "x")]
    command: String,
    /// The revisions to change.
    #[arg(long, short, default_value = "@")]
    revisions: Vec<RevisionArg>,
}

pub(crate) fn cmd_run(
    _ui: &mut Ui,
    _command: &CommandHelper,
    _args: &RunArgs,
) -> Result<(), CommandError> {
    Err(user_error("This is a stub, do not use"))
}
