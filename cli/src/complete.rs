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

use clap::FromArgMatches as _;
use clap_complete::CompletionCandidate;
use jj_lib::workspace::DefaultWorkspaceLoaderFactory;
use jj_lib::workspace::WorkspaceLoaderFactory as _;

use crate::cli_util::expand_args;
use crate::cli_util::find_workspace_dir;
use crate::cli_util::GlobalArgs;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::config::default_config;
use crate::config::LayeredConfigs;
use crate::ui::Ui;

pub fn local_bookmarks() -> Vec<CompletionCandidate> {
    with_jj(|mut jj| {
        let output = jj
            .arg("bookmark")
            .arg("list")
            .arg("--template")
            .arg(r#"if(!remote, name ++ "\n")"#)
            .output()
            .map_err(user_error)?;

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(CompletionCandidate::new)
            .collect())
    })
}

/// Shell out to jj during dynamic completion generation
///
/// In case of errors, print them and early return an empty vector.
fn with_jj<F>(completion_fn: F) -> Vec<CompletionCandidate>
where
    F: FnOnce(std::process::Command) -> Result<Vec<CompletionCandidate>, CommandError>,
{
    get_jj_command()
        .and_then(completion_fn)
        .unwrap_or_else(|e| {
            eprintln!("{}", e.error);
            Vec::new()
        })
}

/// Shell out to jj during dynamic completion generation
///
/// This is necessary because dynamic completion code needs to be aware of
/// global configuration like custom storage backends. Dynamic completion
/// code via clap_complete doesn't accept arguments, so they cannot be passed
/// that way. Another solution would've been to use global mutable state, to
/// give completion code access to custom backends. Shelling out was chosen as
/// the preferred method, because it's more maintainable and the performance
/// requirements of completions aren't very high.
fn get_jj_command() -> Result<std::process::Command, CommandError> {
    let current_exe = std::env::current_exe().map_err(user_error)?;
    let mut command = std::process::Command::new(current_exe);

    // Snapshotting could make completions much slower in some situations
    // and be undesired by the user.
    command.arg("--ignore-working-copy");
    command.arg("--color=never");
    command.arg("--no-pager");

    // Parse some of the global args we care about for passing along to the
    // child process. This shouldn't fail, since none of the global args are
    // required.
    let app = crate::commands::default_app();
    let config = config::Config::builder()
        .add_source(default_config())
        .build()
        .expect("default config should be valid");
    let mut layered_configs = LayeredConfigs::from_environment(config);
    let ui = Ui::with_config(&layered_configs.merge()).expect("default config should be valid");
    let cwd = std::env::current_dir()
        .and_then(|cwd| cwd.canonicalize())
        .map_err(user_error)?;
    let maybe_cwd_workspace_loader = DefaultWorkspaceLoaderFactory.create(find_workspace_dir(&cwd));
    layered_configs.read_user_config().map_err(user_error)?;
    if let Ok(loader) = &maybe_cwd_workspace_loader {
        layered_configs
            .read_repo_config(loader.repo_path())
            .map_err(user_error)?;
    }
    let config = layered_configs.merge();
    // skip 2 because of the clap_complete prelude: jj -- jj <actual args...>
    let args = std::env::args_os().skip(2);
    let args = expand_args(&ui, &app, args, &config)?;
    let args = app
        .clone()
        .disable_version_flag(true)
        .disable_help_flag(true)
        .ignore_errors(true)
        .try_get_matches_from(args)?;
    let args: GlobalArgs = GlobalArgs::from_arg_matches(&args)?;

    if let Some(repository) = args.repository {
        command.arg("--repository");
        command.arg(repository);
    }
    if let Some(at_operation) = args.at_operation {
        command.arg("--at-operation");
        command.arg(at_operation);
    }
    for config_toml in args.early_args.config_toml {
        command.arg("--config-toml");
        command.arg(config_toml);
    }

    Ok(command)
}
