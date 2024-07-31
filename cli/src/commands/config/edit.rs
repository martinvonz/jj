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

use tracing::instrument;

use super::ConfigLevelArgs;
use crate::cli_util::{get_new_config_file_path, run_ui_editor, CommandHelper};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Start an editor on a jj config file.
///
/// Creates the file if it doesn't already exist regardless of what the editor
/// does.
#[derive(clap::Args, Clone, Debug)]
pub struct ConfigEditArgs {
    #[command(flatten)]
    pub level: ConfigLevelArgs,
}

#[instrument(skip_all)]
pub fn cmd_config_edit(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigEditArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.level.expect_source_kind(), command)?;
    run_ui_editor(command.settings(), &config_path)
}
