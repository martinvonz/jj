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

use std::fmt::Debug;
use std::io::Write as _;

use clap_complete::ArgValueCandidates;
use jj_lib::object_id::ObjectId;
use jj_lib::op_walk;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Show information about an operation and its view
#[derive(clap::Args, Clone, Debug)]
pub struct DebugOperationArgs {
    #[arg(default_value = "@", add = ArgValueCandidates::new(complete::operations))]
    operation: String,
    #[arg(long, value_enum, default_value = "all")]
    display: OperationDisplay,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum OperationDisplay {
    /// Show only the operation details.
    Operation,
    /// Show the operation id only
    Id,
    /// Show only the view details
    View,
    /// Show both the view and the operation
    All,
}

pub fn cmd_debug_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugOperationArgs,
) -> Result<(), CommandError> {
    // Resolve the operation without loading the repo, so this command can be used
    // even if e.g. the view object is broken.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op = op_walk::resolve_op_for_load(repo_loader, &args.operation)?;
    if args.display == OperationDisplay::Id {
        writeln!(ui.stdout(), "{}", op.id().hex())?;
        return Ok(());
    }
    if args.display != OperationDisplay::View {
        writeln!(ui.stdout(), "{:#?}", op.store_operation())?;
    }
    if args.display != OperationDisplay::Operation {
        writeln!(ui.stdout(), "{:#?}", op.view()?.store_view())?;
    }
    Ok(())
}
