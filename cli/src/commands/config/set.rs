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

use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use tracing::instrument;

use super::ConfigLevelArgs;
use crate::cli_util::{get_new_config_file_path, CommandHelper, WorkspaceCommandHelper};
use crate::command_error::{user_error, CommandError};
use crate::config::{write_config_value_to_file, ConfigNamePathBuf};
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
pub(crate) fn cmd_config_set(
    ui: &mut Ui,
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

    // If the user is trying to change the author config, we should warn them that
    // it won't affect the working copy author
    if args.name == ConfigNamePathBuf::from_iter(vec!["user", "name"]) {
        check_wc_user_name(command, ui, &args.value)?;
    } else if args.name == ConfigNamePathBuf::from_iter(vec!["user", "email"]) {
        check_wc_user_email(command, ui, &args.value)?;
    };

    write_config_value_to_file(&args.name, &args.value, &config_path)
}

/// Returns the commit of the working copy if it exists.
fn maybe_wc_commit(helper: &WorkspaceCommandHelper) -> Option<Commit> {
    let repo = helper.repo();
    let maybe_wc_commit = helper
        .get_wc_commit_id()
        .map(|id| repo.store().get_commit(id))
        .transpose()
        .unwrap();
    maybe_wc_commit
}

/// Check if the working copy author name matches the user's config value
/// If it doesn't, print a warning message
fn check_wc_user_name(
    command: &CommandHelper,
    ui: &mut Ui,
    user_name: &str,
) -> Result<(), CommandError> {
    let helper = command.workspace_helper(ui)?;
    if let Some(wc_commit) = maybe_wc_commit(&helper) {
        let author = wc_commit.author();
        if author.name != user_name {
            warn_wc_author(&author.name, &author.email, ui)?
        }
    };
    Ok(())
}

/// Check if the working copy author email matches the user's config value
/// If it doesn't, print a warning message
fn check_wc_user_email(
    command: &CommandHelper,
    ui: &mut Ui,
    user_email: &str,
) -> Result<(), CommandError> {
    let helper = command.workspace_helper(ui)?;
    if let Some(wc_commit) = maybe_wc_commit(&helper) {
        let author = wc_commit.author();
        if author.email != user_email {
            warn_wc_author(&author.name, &author.email, ui)?
        }
    };
    Ok(())
}

/// Prints a warning message about the working copy to the user
fn warn_wc_author(user_name: &str, user_email: &str, ui: &mut Ui) -> Result<(), CommandError> {
    Ok(writeln!(
        ui.warning_default(),
        "This setting will only impact future commits.\nThe author of the working copy will stay \
         \"{user_name} <{user_email}>\". \nTo change the working copy author, use \"jj describe \
         --reset-author --no-edit\""
    )?)
}
