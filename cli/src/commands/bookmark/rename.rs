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
use jj_lib::op_store::RefTarget;

use super::has_tracked_remote_bookmarks;
use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Rename `old` bookmark name to `new` bookmark name
///
/// The new bookmark name points at the same commit as the old bookmark name.
#[derive(clap::Args, Clone, Debug)]
pub struct BookmarkRenameArgs {
    /// The old name of the bookmark
    #[arg(add = ArgValueCandidates::new(complete::local_bookmarks))]
    old: String,

    /// The new name of the bookmark
    new: String,
}

pub fn cmd_bookmark_rename(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BookmarkRenameArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    let old_bookmark = &args.old;
    let ref_target = view.get_local_bookmark(old_bookmark).clone();
    if ref_target.is_absent() {
        return Err(user_error(format!("No such bookmark: {old_bookmark}")));
    }

    let new_bookmark = &args.new;
    if view.get_local_bookmark(new_bookmark).is_present() {
        return Err(user_error(format!(
            "Bookmark already exists: {new_bookmark}"
        )));
    }

    let mut tx = workspace_command.start_transaction();
    tx.repo_mut()
        .set_local_bookmark_target(new_bookmark, ref_target);
    tx.repo_mut()
        .set_local_bookmark_target(old_bookmark, RefTarget::absent());
    tx.finish(
        ui,
        format!("rename bookmark {old_bookmark} to {new_bookmark}"),
    )?;

    let view = workspace_command.repo().view();
    if has_tracked_remote_bookmarks(view, old_bookmark) {
        writeln!(
            ui.warning_default(),
            "Tracked remote bookmarks for bookmark {old_bookmark} were not renamed.",
        )?;
        writeln!(
            ui.hint_default(),
            "To rename the bookmark on the remote, you can `jj git push --bookmark \
             {old_bookmark}` first (to delete it on the remote), and then `jj git push --bookmark \
             {new_bookmark}`. `jj git push --all` would also be sufficient."
        )?;
    }
    if has_tracked_remote_bookmarks(view, new_bookmark) {
        // This isn't an error because bookmark renaming can't be propagated to
        // the remote immediately. "rename old new && rename new old" should be
        // allowed even if the original old bookmark had tracked remotes.
        writeln!(
            ui.warning_default(),
            "Tracked remote bookmarks for bookmark {new_bookmark} exist."
        )?;
        writeln!(
            ui.hint_default(),
            "Run `jj bookmark untrack 'glob:{new_bookmark}@*'` to disassociate them."
        )?;
    }

    Ok(())
}
