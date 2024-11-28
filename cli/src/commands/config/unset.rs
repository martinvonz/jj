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

use clap_complete::ArgValueCandidates;
use jj_lib::config::ConfigNamePathBuf;
use tracing::instrument;

use super::ConfigLevelArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
use crate::config::remove_config_value_from_file;
use crate::ui::Ui;

/// Update config file to unset the given option.
#[derive(clap::Args, Clone, Debug)]
pub struct ConfigUnsetArgs {
    #[arg(required = true, add = ArgValueCandidates::new(complete::leaf_config_keys))]
    name: ConfigNamePathBuf,
    #[command(flatten)]
    level: ConfigLevelArgs,
}

#[instrument(skip_all)]
pub fn cmd_config_unset(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigUnsetArgs,
) -> Result<(), CommandError> {
    let config_path = args.level.new_config_file_path(command.config_env())?;
    if config_path.is_dir() {
        return Err(user_error(format!(
            "Can't set config in path {path} (dirs not supported)",
            path = config_path.display()
        )));
    }

    remove_config_value_from_file(&args.name, config_path)
}
