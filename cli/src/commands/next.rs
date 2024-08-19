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

/// Move the working-copy commit to the child revision
///
/// The command creates a new empty working copy revision that is the child of a
/// descendant `offset` revisions ahead of the parent of the current working
/// copy.
///
/// For example, when the offset is 1:
///
/// ```text
/// D        D @
/// |        |/
/// C @  =>  C
/// |/       |
/// B        B
/// ```
///
/// If `--edit` is passed, the working copy revision is changed to the child of
/// the current working copy revision.
///
/// ```text
/// D        D
/// |        |
/// C        C
/// |        |
/// B   =>   @
/// |        |
/// @        A
/// ```
/// If your working-copy commit already has visible children, then `--edit` is
/// implied.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct NextArgs {
    /// How many revisions to move forward. Advances to the next child by
    /// default
    #[arg(default_value = "1")]
    offset: u64,
    /// Instead of creating a new working-copy commit on top of the target
    /// commit (like `jj new`), edit the target commit directly (like `jj
    /// edit`)
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
    /// Jump to the next conflicted descendant
    #[arg(long, conflicts_with = "offset")]
    conflict: bool,
}

impl From<&NextArgs> for MovementArgs {
    fn from(val: &NextArgs) -> Self {
        MovementArgs {
            offset: val.offset,
            edit: val.edit,
            no_edit: val.no_edit,
            conflict: val.conflict,
        }
    }
}

pub(crate) fn cmd_next(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &NextArgs,
) -> Result<(), CommandError> {
    move_to_commit(ui, command, Direction::Next, &MovementArgs::from(args))
}
