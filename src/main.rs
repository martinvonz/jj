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

use jujutsu::cli_util::{handle_command_result, parse_args, CommandError, TracingSubscription};
use jujutsu::commands::{default_app, run_command};
use jujutsu::config::read_config;
use jujutsu::ui::Ui;

fn run(ui: &mut Ui, tracing_subscription: &TracingSubscription) -> Result<(), CommandError> {
    ui.reset(read_config()?);
    let app = default_app();
    let (command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    if command_helper.global_args().verbose {
        tracing_subscription.enable_verbose_logging()?;
    }
    run_command(ui, &command_helper, &matches)
}

fn main() {
    // TODO(@rslabbert): restructure logging filter setup to better handle
    // having verbose logging set up as early as possible, and to support
    // custom commands. See discussion on:
    // https://github.com/martinvonz/jj/pull/771
    let tracing_subscription = TracingSubscription::init();
    jujutsu::cleanup_guard::init();
    let mut ui = Ui::new();
    let result = run(&mut ui, &tracing_subscription);
    let exit_code = handle_command_result(&mut ui, result);
    ui.finalize_writes();
    std::process::exit(exit_code);
}
