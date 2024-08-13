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

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::movement_util::{move_to_commit, Direction, MovementArgs};
use crate::ui::Ui;
/// Change the working copy revision relative to the parent revision
///
/// The command creates a new empty working copy revision that is the child of
/// an ancestor `offset` revisions behind the parent of the current working
/// copy.
///
/// For example, when the offset is 1:
///
/// ```text
/// D @      D
/// |/       |
/// A   =>   A @
/// |        |/
/// B        B
/// ```
///
/// If `--edit` is passed, the working copy revision is changed to the parent of
/// the current working copy revision.
///
/// ```text
/// D @      D
/// |/       |
/// C   =>   @
/// |        |
/// B        B
/// |        |
/// A        A
/// ```
/// If the working copy revision already has visible children, then `--edit` is
/// implied
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct PrevArgs {
    /// How many revisions to move backward. Moves to the parent by default
    #[arg(default_value = "1")]
    offset: u64,
    /// Edit the parent directly, instead of moving the working-copy commit
    ///
    /// Takes precedence over config in `ui.movement.edit`; i.e.
    /// will negate `ui.movement.edit = false`
    #[arg(long, short)]
    edit: bool,
    /// The inverse of `--edit`
    ///
    /// Takes precedence over config in `ui.movement.edit`; i.e.
    /// will negate `ui.movement.edit = true`
    #[arg(long, short, conflicts_with = "edit")]
    no_edit: bool,
    /// Jump to the previous conflicted ancestor
    #[arg(long, conflicts_with = "offset")]
    conflict: bool,
}

impl From<&PrevArgs> for MovementArgs {
    fn from(val: &PrevArgs) -> Self {
        MovementArgs {
            offset: val.offset,
            edit: val.edit,
            no_edit: val.no_edit,
            conflict: val.conflict,
        }
    }
}

pub(crate) fn cmd_prev(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PrevArgs,
) -> Result<(), CommandError> {
    move_to_commit(ui, command, Direction::Prev, &MovementArgs::from(args))
}
