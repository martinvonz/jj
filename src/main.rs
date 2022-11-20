// Copyright 2020 Google LLC
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

use jujutsu::cli_util::{create_ui, handle_command_result, parse_args, CommandError};
use jujutsu::commands::{default_app, run_command};
use jujutsu::ui::Ui;
use tracing::metadata::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload::Handle;
use tracing_subscriber::EnvFilter;

fn run(
    ui: &mut Ui,
    reload_log_filter: Handle<EnvFilter, impl tracing::Subscriber>,
) -> Result<(), CommandError> {
    let app = default_app();
    let (command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    if command_helper.global_args().verbose {
        reload_log_filter
            .modify(|filter| {
                *filter = EnvFilter::builder()
                    .with_default_directive(LevelFilter::DEBUG.into())
                    .from_env_lossy()
            })
            .map_err(|err| {
                CommandError::InternalError(format!("failed to enable verbose logging: {:?}", err))
            })?;
        tracing::debug!("verbose logging enabled");
    }
    run_command(ui, &command_helper, &matches)
}

fn main() {
    // TODO(@rslabbert): restructure logging filter setup to better handle
    // having verbose logging set up as early as possible, and to support
    // custom commands. See discussion on:
    // https://github.com/martinvonz/jj/pull/771
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let (filter, reload_log_filter) = tracing_subscriber::reload::Layer::new(filter);
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::Layer::default().with_writer(std::io::stderr))
        .init();

    jujutsu::cleanup_guard::init();
    let (mut ui, result) = create_ui();
    let result = result.and_then(|()| run(&mut ui, reload_log_filter));
    let exit_code = handle_command_result(&mut ui, result);
    std::process::exit(exit_code);
}
