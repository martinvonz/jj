// Copyright 2024 The Jujutsu Authors
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

use jj_lib::working_copy::SnapshotOptions;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Start tracking specified paths in the working copy
///
/// Without arguments, all paths that are not ignored will be tracked.
///
/// New files in the working copy can be automatically tracked.  
/// You can configure which paths to automatically track by setting
/// `snapshot.auto-track` (e.g. to `"none()"` or `"glob:**/*.rs"`). Files that
/// don't match the pattern can be manually tracked using this command. The
/// default pattern is `all()` and this command has no effect.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct FileTrackArgs {
    /// Paths to track
    #[arg(required = true, value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_file_track(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &FileTrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let matcher = workspace_command
        .parse_file_patterns(&args.paths)?
        .to_matcher();

    let mut tx = workspace_command.start_transaction().into_inner();
    let base_ignores = workspace_command.base_ignores()?;
    let (mut locked_ws, _wc_commit) = workspace_command.start_working_copy_mutation()?;
    locked_ws.locked_wc().snapshot(&SnapshotOptions {
        base_ignores,
        fsmonitor_settings: command.settings().fsmonitor_settings()?,
        progress: None,
        start_tracking_matcher: &matcher,
        max_new_file_size: command.settings().max_new_file_size()?,
    })?;
    let num_rebased = tx.repo_mut().rebase_descendants(command.settings())?;
    if num_rebased > 0 {
        writeln!(ui.status(), "Rebased {num_rebased} descendant commits")?;
    }
    if command.should_commit_transaction() {
        let repo = tx.commit("track paths");
        locked_ws.finish(repo.op_id().clone())?;
    }
    Ok(())
}
