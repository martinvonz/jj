// Copyright 2023 The Jujutsu Authors
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

pub mod copy_detection;
pub mod fileset;
pub mod index;
pub mod local_working_copy;
pub mod operation;
pub mod reindex;
pub mod revset;
pub mod snapshot;
pub mod template;
pub mod tree;
pub mod watchman;
pub mod working_copy;

use std::any::Any;
use std::fmt::Debug;

use clap::Subcommand;
use jj_lib::local_working_copy::LocalWorkingCopy;

use self::copy_detection::cmd_debug_copy_detection;
use self::copy_detection::CopyDetectionArgs;
use self::fileset::cmd_debug_fileset;
use self::fileset::DebugFilesetArgs;
use self::index::cmd_debug_index;
use self::index::DebugIndexArgs;
use self::local_working_copy::cmd_debug_local_working_copy;
use self::local_working_copy::DebugLocalWorkingCopyArgs;
use self::operation::cmd_debug_operation;
use self::operation::DebugOperationArgs;
use self::reindex::cmd_debug_reindex;
use self::reindex::DebugReindexArgs;
use self::revset::cmd_debug_revset;
use self::revset::DebugRevsetArgs;
use self::snapshot::cmd_debug_snapshot;
use self::snapshot::DebugSnapshotArgs;
use self::template::cmd_debug_template;
use self::template::DebugTemplateArgs;
use self::tree::cmd_debug_tree;
use self::tree::DebugTreeArgs;
use self::watchman::cmd_debug_watchman;
use self::watchman::DebugWatchmanCommand;
use self::working_copy::cmd_debug_working_copy;
use self::working_copy::DebugWorkingCopyArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Low-level commands not intended for users
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum DebugCommand {
    CopyDetection(CopyDetectionArgs),
    Fileset(DebugFilesetArgs),
    Index(DebugIndexArgs),
    LocalWorkingCopy(DebugLocalWorkingCopyArgs),
    #[command(visible_alias = "view")]
    Operation(DebugOperationArgs),
    Reindex(DebugReindexArgs),
    Revset(DebugRevsetArgs),
    Snapshot(DebugSnapshotArgs),
    Template(DebugTemplateArgs),
    Tree(DebugTreeArgs),
    #[command(subcommand)]
    Watchman(DebugWatchmanCommand),
    WorkingCopy(DebugWorkingCopyArgs),
}

pub fn cmd_debug(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugCommand,
) -> Result<(), CommandError> {
    match subcommand {
        DebugCommand::Fileset(args) => cmd_debug_fileset(ui, command, args),
        DebugCommand::Index(args) => cmd_debug_index(ui, command, args),
        DebugCommand::LocalWorkingCopy(args) => cmd_debug_local_working_copy(ui, command, args),
        DebugCommand::Operation(args) => cmd_debug_operation(ui, command, args),
        DebugCommand::Reindex(args) => cmd_debug_reindex(ui, command, args),
        DebugCommand::CopyDetection(args) => cmd_debug_copy_detection(ui, command, args),
        DebugCommand::Revset(args) => cmd_debug_revset(ui, command, args),
        DebugCommand::Snapshot(args) => cmd_debug_snapshot(ui, command, args),
        DebugCommand::Template(args) => cmd_debug_template(ui, command, args),
        DebugCommand::Tree(args) => cmd_debug_tree(ui, command, args),
        DebugCommand::Watchman(args) => cmd_debug_watchman(ui, command, args),
        DebugCommand::WorkingCopy(args) => cmd_debug_working_copy(ui, command, args),
    }
}

fn check_local_disk_wc(x: &dyn Any) -> Result<&LocalWorkingCopy, CommandError> {
    x.downcast_ref()
        .ok_or_else(|| user_error("This command requires a standard local-disk working copy"))
}
