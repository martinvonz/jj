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

use std::fmt::Debug;
use std::io::Write as _;

use futures::executor::block_on_stream;
use jj_lib::backend::Backend;
use jj_lib::backend::CopyRecord;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Rebuild commit index
#[derive(clap::Args, Clone, Debug)]
pub struct CopyDetectionArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(default_value = "@", value_name = "REVSET")]
    revision: RevisionArg,
}

pub fn cmd_debug_copy_detection(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CopyDetectionArgs,
) -> Result<(), CommandError> {
    let ws = command.workspace_helper(ui)?;
    let Some(git) = ws.git_backend() else {
        writeln!(ui.stderr(), "Not a git backend.")?;
        return Ok(());
    };
    let commit = ws.resolve_single_rev(ui, &args.revision)?;
    for parent_id in commit.parent_ids() {
        for CopyRecord { target, source, .. } in
            block_on_stream(git.get_copy_records(None, parent_id, commit.id())?)
                .filter_map(|r| r.ok())
        {
            writeln!(
                ui.stdout(),
                "{} -> {}",
                source.as_internal_file_string(),
                target.as_internal_file_string()
            )?;
        }
    }
    Ok(())
}
