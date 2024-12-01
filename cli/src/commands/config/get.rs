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

use std::io::Write as _;
use std::path::PathBuf;

use clap_complete::ArgValueCandidates;
use jj_lib::config::ConfigError;
use jj_lib::config::ConfigGetError;
use jj_lib::config::ConfigNamePathBuf;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Get the value of a given config option.
///
/// Unlike `jj config list`, the result of `jj config get` is printed without
/// extra formatting and therefore is usable in scripting. For example:
///
/// $ jj config list user.name
/// user.name="Martin von Zweigbergk"
/// $ jj config get user.name
/// Martin von Zweigbergk
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub struct ConfigGetArgs {
    #[arg(required = true, add = ArgValueCandidates::new(complete::leaf_config_keys))]
    name: ConfigNamePathBuf,
}

#[instrument(skip_all)]
pub fn cmd_config_get(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigGetArgs,
) -> Result<(), CommandError> {
    let value = command.settings().get_value(&args.name)?;
    let stringified = value.into_string().map_err(|err| -> CommandError {
        match err {
            ConfigError::Type {
                origin, unexpected, ..
            } => ConfigGetError::Type {
                name: args.name.to_string(),
                error: format!("Expected a value convertible to a string, but is {unexpected}")
                    .into(),
                source_path: origin.map(PathBuf::from),
            }
            .into(),
            err => err.into(),
        }
    })?;
    writeln!(ui.stdout(), "{stringified}")?;
    Ok(())
}
