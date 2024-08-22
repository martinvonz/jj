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

mod edit;
mod get;
mod list;
mod path;
mod set;

use tracing::instrument;

use self::edit::cmd_config_edit;
use self::edit::ConfigEditArgs;
use self::get::cmd_config_get;
use self::get::ConfigGetArgs;
use self::list::cmd_config_list;
use self::list::ConfigListArgs;
use self::path::cmd_config_path;
use self::path::ConfigPathArgs;
use self::set::cmd_config_set;
use self::set::ConfigSetArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::config::ConfigSource;
use crate::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
#[command(group = clap::ArgGroup::new("config_level").multiple(false).required(true))]
pub(crate) struct ConfigLevelArgs {
    /// Target the user-level config
    #[arg(long, group = "config_level")]
    user: bool,

    /// Target the repo-level config
    #[arg(long, group = "config_level")]
    repo: bool,
}

impl ConfigLevelArgs {
    fn expect_source_kind(&self) -> ConfigSource {
        self.get_source_kind().expect("No config_level provided")
    }

    fn get_source_kind(&self) -> Option<ConfigSource> {
        if self.user {
            Some(ConfigSource::User)
        } else if self.repo {
            Some(ConfigSource::Repo)
        } else {
            None
        }
    }
}

/// Manage config options
///
/// Operates on jj configuration, which comes from the config file and
/// environment variables.
///
/// For file locations, supported config options, and other details about jj
/// config, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
#[derive(clap::Subcommand, Clone, Debug)]
pub(crate) enum ConfigCommand {
    #[command(visible_alias("e"))]
    Edit(ConfigEditArgs),
    #[command(visible_alias("g"))]
    Get(ConfigGetArgs),
    #[command(visible_alias("l"))]
    List(ConfigListArgs),
    #[command(visible_alias("p"))]
    Path(ConfigPathArgs),
    #[command(visible_alias("s"))]
    Set(ConfigSetArgs),
}

#[instrument(skip_all)]
pub(crate) fn cmd_config(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &ConfigCommand,
) -> Result<(), CommandError> {
    match subcommand {
        ConfigCommand::Edit(args) => cmd_config_edit(ui, command, args),
        ConfigCommand::Get(args) => cmd_config_get(ui, command, args),
        ConfigCommand::List(args) => cmd_config_list(ui, command, args),
        ConfigCommand::Path(args) => cmd_config_path(ui, command, args),
        ConfigCommand::Set(args) => cmd_config_set(ui, command, args),
    }
}
