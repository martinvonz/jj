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

use jj_lib::fileset;
use jj_lib::fileset::FilesetDiagnostics;

use crate::cli_util::CommandHelper;
use crate::command_error::print_parse_diagnostics;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Parse fileset expression
#[derive(clap::Args, Clone, Debug)]
pub struct DebugFilesetArgs {
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    path: String,
}

pub fn cmd_debug_fileset(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugFilesetArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let path_converter = workspace_command.path_converter();

    let mut diagnostics = FilesetDiagnostics::new();
    let expression = fileset::parse_maybe_bare(&mut diagnostics, &args.path, path_converter)?;
    print_parse_diagnostics(ui, "In fileset expression", &diagnostics)?;
    writeln!(ui.stdout(), "-- Parsed:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let matcher = expression.to_matcher();
    writeln!(ui.stdout(), "-- Matcher:")?;
    writeln!(ui.stdout(), "{matcher:#?}")?;
    Ok(())
}
