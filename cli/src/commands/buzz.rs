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

use std::io::Write;

use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Display version information
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct BuzzArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_buzz(
    ui: &mut Ui,
    _command: &CommandHelper,
    _args: &BuzzArgs,
) -> Result<(), CommandError> {
    write!(ui.stdout(), "\x07")?; // https://www.ascii-code.com/7
    Ok(())
}
