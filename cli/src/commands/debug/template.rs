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

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::template_parser;
use crate::ui::Ui;

/// Parse a template
#[derive(clap::Args, Clone, Debug)]
pub struct DebugTemplateArgs {
    template: String,
}

pub fn cmd_debug_template(
    ui: &mut Ui,
    _command: &CommandHelper,
    args: &DebugTemplateArgs,
) -> Result<(), CommandError> {
    let node = template_parser::parse_template(&args.template)?;
    writeln!(ui.stdout(), "{node:#?}")?;
    Ok(())
}
