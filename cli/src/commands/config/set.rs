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
use crate::cli_util::{get_new_config_file_path, CommandHelper};
use crate::command_error::{user_error, CommandError};
use crate::config::{
    parse_toml_value_or_bare_string, write_config_value_to_file, ConfigNamePathBuf,
};
use crate::ui::Ui;

/// Update config file to set the given option to a given value.
#[derive(clap::Args, Clone, Debug)]
pub struct ConfigSetArgs {
    #[arg(required = true)]
    name: ConfigNamePathBuf,
    #[arg(required = true)]
    value: String,
    #[command(flatten)]
    level: ConfigLevelArgs,
}

#[instrument(skip_all)]
pub fn cmd_config_set(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigSetArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.level.expect_source_kind(), command)?;
    if config_path.is_dir() {
        return Err(user_error(format!(
            "Can't set config in path {path} (dirs not supported)",
            path = config_path.display()
        )));
    }
    // TODO(#531): Infer types based on schema (w/ --type arg to override).
    let value = parse_toml_value_or_bare_string(&args.value);
    write_config_value_to_file(&args.name, value, &config_path)
}
