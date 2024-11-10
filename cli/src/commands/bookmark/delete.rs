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

use clap_complete::ArgValueCandidates;
use itertools::Itertools as _;
use jj_lib::op_store::RefTarget;
use jj_lib::str_util::StringPattern;

use super::find_local_bookmarks;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Delete an existing bookmark and propagate the deletion to remotes on the
/// next push
#[derive(clap::Args, Clone, Debug)]
pub struct BookmarkDeleteArgs {
    /// The bookmarks to delete
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select bookmarks by wildcard pattern. For details, see
    /// https://martinvonz.github.io/jj/latest/revsets/#string-patterns.       
    #[arg(
        required = true,
        value_parser = StringPattern::parse,
        add = ArgValueCandidates::new(complete::local_bookmarks),
    )]
    names: Vec<StringPattern>,
}

pub fn cmd_bookmark_delete(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BookmarkDeleteArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();
    let matched_bookmarks = find_local_bookmarks(repo.view(), &args.names)?;
    let mut tx = workspace_command.start_transaction();
    for (name, _) in &matched_bookmarks {
        tx.repo_mut()
            .set_local_bookmark_target(name, RefTarget::absent());
    }
    writeln!(
        ui.status(),
        "Deleted {} bookmarks.",
        matched_bookmarks.len()
    )?;
    tx.finish(
        ui,
        format!(
            "delete bookmark {}",
            matched_bookmarks.iter().map(|(name, _)| name).join(", ")
        ),
    )?;
    Ok(())
}
