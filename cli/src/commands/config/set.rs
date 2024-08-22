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

use std::io;

use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use tracing::instrument;

use super::ConfigLevelArgs;
use crate::cli_util::get_new_config_file_path;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::config::parse_toml_value_or_bare_string;
use crate::config::write_config_value_to_file;
use crate::config::ConfigNamePathBuf;
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

/// Denotes a type of author change
enum AuthorChange {
    Name,
    Email,
}

#[instrument(skip_all)]
pub fn cmd_config_set(
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

    // TODO(#531): Infer types based on schema (w/ --type arg to override).
    let value = parse_toml_value_or_bare_string(&args.value);

    // If the user is trying to change the author config, we should warn them that
    // it won't affect the working copy author
    if args.name == ConfigNamePathBuf::from_iter(vec!["user", "name"]) {
        check_wc_author(ui, command, &value, AuthorChange::Name)?;
    } else if args.name == ConfigNamePathBuf::from_iter(vec!["user", "email"]) {
        check_wc_author(ui, command, &value, AuthorChange::Email)?;
    };

    write_config_value_to_file(&args.name, value, &config_path)
}

/// Returns the commit of the working copy if it exists.
fn maybe_wc_commit(helper: &WorkspaceCommandHelper) -> Option<Commit> {
    let repo = helper.repo();
    let id = helper.get_wc_commit_id()?;
    repo.store().get_commit(id).ok()
}

/// Check if the working copy author name matches the user's config value
/// If it doesn't, print a warning message
fn check_wc_author(
    ui: &mut Ui,
    command: &CommandHelper,
    new_value: &toml_edit::Value,
    author_change: AuthorChange,
) -> io::Result<()> {
    let helper = match command.workspace_helper(ui) {
        Ok(helper) => helper,
        Err(_) => return Ok(()), // config set should work even if cwd isn't a jj repo
    };
    if let Some(wc_commit) = maybe_wc_commit(&helper) {
        let author = wc_commit.author();
        let orig_value = match author_change {
            AuthorChange::Name => &author.name,
            AuthorChange::Email => &author.email,
        };
        if new_value.as_str() != Some(orig_value) {
            warn_wc_author(ui, &author.name, &author.email)?
        }
    }
    Ok(())
}

/// Prints a warning message about the working copy to the user
fn warn_wc_author(ui: &Ui, user_name: &str, user_email: &str) -> io::Result<()> {
    Ok(writeln!(
        ui.warning_default(),
        "This setting will only impact future commits.\nThe author of the working copy will stay \
         \"{user_name} <{user_email}>\".\nTo change the working copy author, use \"jj describe \
         --reset-author --no-edit\""
    )?)
}
