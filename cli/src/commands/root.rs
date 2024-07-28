// Copyright 2024 The Jujutsu Authors
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

use std::io::Write as _;

use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Show the current workspace root directory
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct RootArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_root(
    ui: &mut Ui,
    command: &CommandHelper,
    RootArgs {}: &RootArgs,
) -> Result<(), CommandError> {
    let root = command
        .workspace_loader()?
        .workspace_root()
        .to_str()
        .ok_or_else(|| user_error("The workspace root is not valid UTF-8"))?;
    writeln!(ui.stdout(), "{root}")?;
    Ok(())
}
