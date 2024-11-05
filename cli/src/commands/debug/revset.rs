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

use jj_lib::object_id::ObjectId;
use jj_lib::revset;
use jj_lib::revset::RevsetDiagnostics;

use crate::cli_util::CommandHelper;
use crate::command_error::print_parse_diagnostics;
use crate::command_error::CommandError;
use crate::revset_util;
use crate::ui::Ui;

/// Evaluate revset to full commit IDs
#[derive(clap::Args, Clone, Debug)]
pub struct DebugRevsetArgs {
    revision: String,
}

pub fn cmd_debug_revset(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugRevsetArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let workspace_ctx = workspace_command.revset_parse_context();
    let repo = workspace_command.repo().as_ref();

    let mut diagnostics = RevsetDiagnostics::new();
    let expression = revset::parse(&mut diagnostics, &args.revision, &workspace_ctx)?;
    print_parse_diagnostics(ui, "In revset expression", &diagnostics)?;
    writeln!(ui.stdout(), "-- Parsed:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let symbol_resolver = revset_util::default_symbol_resolver(
        repo,
        command.revset_extensions().symbol_resolvers(),
        workspace_command.id_prefix_context(),
    );
    let expression = expression.resolve_user_expression(repo, &symbol_resolver)?;
    writeln!(ui.stdout(), "-- Resolved:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let expression = revset::optimize(expression);
    writeln!(ui.stdout(), "-- Optimized:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let backend_expression = expression.to_backend_expression(repo);
    writeln!(ui.stdout(), "-- Backend:")?;
    writeln!(ui.stdout(), "{backend_expression:#?}")?;
    writeln!(ui.stdout())?;

    let revset = expression.evaluate_unoptimized(repo)?;
    writeln!(ui.stdout(), "-- Evaluated:")?;
    writeln!(ui.stdout(), "{revset:#?}")?;
    writeln!(ui.stdout())?;

    writeln!(ui.stdout(), "-- Commit IDs:")?;
    for commit_id in revset.iter() {
        writeln!(ui.stdout(), "{}", commit_id?.hex())?;
    }
    Ok(())
}
