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

#![allow(unused, dead_code)]

use std::fmt::Debug;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use futures::executor::block_on_stream;
use futures::StreamExt;
use jj_lib::backend::{Backend, CopyRecord, CopySource, CopySources};
use jj_lib::default_index::{AsCompositeIndex as _, DefaultIndexStore};
use jj_lib::op_walk;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{internal_error, user_error, CommandError};
use crate::ui::Ui;

/// Rebuild commit index
#[derive(clap::Args, Clone, Debug)]
pub struct CopyDetectionArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(default_value = "@")]
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
    let commit = ws.resolve_single_rev(&args.revision)?;
    let tree = commit.tree()?;

    let paths: Vec<RepoPathBuf> = tree.entries().map(|(path, _)| path).collect();
    let commits = [commit.id().clone()];
    let parents = commit.parent_ids();
    for copy_record in
        block_on_stream(git.get_copy_records(&paths, parents, &commits)?).filter_map(|r| r.ok())
    {
        match copy_record.sources {
            CopySources::Resolved(CopySource { path, .. }) => {
                write!(ui.stdout(), "{}", path.as_internal_file_string());
            }
            CopySources::Conflict(conflicting) => {
                let mut sorted: Vec<_> = conflicting
                    .iter()
                    .map(|s| s.path.as_internal_file_string())
                    .collect();
                sorted.sort();
                write!(ui.stdout(), "{{ {} }}", sorted.join(", "));
            }
        }
        writeln!(
            ui.stdout(),
            " -> {}",
            copy_record.target.as_internal_file_string()
        );
    }
    Ok(())
}
