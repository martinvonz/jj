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
use jj_lib::op_store::BookmarkTarget;
use jj_lib::op_store::RefTarget;
use jj_lib::op_store::RemoteRef;
use jj_lib::str_util::StringPattern;
use jj_lib::view::View;

use super::find_bookmarks_with;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Forget everything about a bookmark, including its local and remote
/// targets
///
/// A forgotten bookmark will not impact remotes on future pushes. It will be
/// recreated on future pulls if it still exists in the remote.
#[derive(clap::Args, Clone, Debug)]
pub struct BookmarkForgetArgs {
    /// The bookmarks to forget
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select bookmarks by wildcard pattern. For details, see
    /// https://jj-vcs.github.io/jj/latest/revsets/#string-patterns.    
    #[arg(
        required = true,
        value_parser = StringPattern::parse,
        add = ArgValueCandidates::new(complete::bookmarks),
    )]
    names: Vec<StringPattern>,
}

pub fn cmd_bookmark_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BookmarkForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();
    let matched_bookmarks = find_forgettable_bookmarks(repo.view(), &args.names)?;
    let mut tx = workspace_command.start_transaction();
    for (name, bookmark_target) in &matched_bookmarks {
        tx.repo_mut()
            .set_local_bookmark_target(name, RefTarget::absent());
        for (remote_name, _) in &bookmark_target.remote_refs {
            tx.repo_mut()
                .set_remote_bookmark(name, remote_name, RemoteRef::absent());
        }
    }
    writeln!(ui.status(), "Forgot {} bookmarks.", matched_bookmarks.len())?;
    tx.finish(
        ui,
        format!(
            "forget bookmark {}",
            matched_bookmarks.iter().map(|(name, _)| name).join(", ")
        ),
    )?;
    Ok(())
}

fn find_forgettable_bookmarks<'a>(
    view: &'a View,
    name_patterns: &[StringPattern],
) -> Result<Vec<(&'a str, BookmarkTarget<'a>)>, CommandError> {
    find_bookmarks_with(name_patterns, |pattern| {
        view.bookmarks()
            .filter(|(name, _)| pattern.matches(name))
            .map(Ok)
    })
}
