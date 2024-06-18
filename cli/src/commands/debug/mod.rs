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

pub mod fileset;
pub mod index;
pub mod operation;
pub mod reindex;
pub mod revset;
pub mod template;
pub mod tree;
pub mod watchman;
pub mod working_copy;

use std::any::Any;
use std::fmt::Debug;

use clap::Subcommand;
use jj_lib::local_working_copy::LocalWorkingCopy;

use self::fileset::{cmd_debug_fileset, FilesetArgs};
use self::index::{cmd_debug_index, IndexArgs};
use self::operation::{cmd_debug_operation, OperationArgs};
use self::reindex::{cmd_debug_reindex, ReindexArgs};
use self::revset::{cmd_debug_revset, RevsetArgs};
use self::template::{cmd_debug_template, TemplateArgs};
use self::tree::{cmd_debug_tree, TreeArgs};
use self::watchman::{cmd_debug_watchman, WatchmanCommand};
use self::working_copy::{cmd_debug_working_copy, WorkingCopyArgs};
use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Low-level commands not intended for users
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum DebugCommand {
    Fileset(FilesetArgs),
    Revset(RevsetArgs),
    #[command(name = "workingcopy")]
    WorkingCopy(WorkingCopyArgs),
    Template(TemplateArgs),
    Index(IndexArgs),
    Reindex(ReindexArgs),
    #[command(visible_alias = "view")]
    Operation(OperationArgs),
    Tree(TreeArgs),
    #[command(subcommand)]
    Watchman(WatchmanCommand),
}

pub fn cmd_debug(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugCommand,
) -> Result<(), CommandError> {
    match subcommand {
        DebugCommand::Fileset(args) => cmd_debug_fileset(ui, command, args),
        DebugCommand::Revset(args) => cmd_debug_revset(ui, command, args),
        DebugCommand::WorkingCopy(args) => cmd_debug_working_copy(ui, command, args),
        DebugCommand::Template(args) => cmd_debug_template(ui, command, args),
        DebugCommand::Index(args) => cmd_debug_index(ui, command, args),
        DebugCommand::Reindex(args) => cmd_debug_reindex(ui, command, args),
        DebugCommand::Operation(args) => cmd_debug_operation(ui, command, args),
        DebugCommand::Tree(args) => cmd_debug_tree(ui, command, args),
        DebugCommand::Watchman(args) => cmd_debug_watchman(ui, command, args),
    }
}

fn check_local_disk_wc(x: &dyn Any) -> Result<&LocalWorkingCopy, CommandError> {
    x.downcast_ref()
        .ok_or_else(|| user_error("This command requires a standard local-disk working copy"))
}
