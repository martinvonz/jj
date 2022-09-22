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

use jujutsu::cli_util::{create_ui, parse_args, report_command_error, CommandError};
use jujutsu::commands::{default_app, run_command};
use jujutsu::ui::Ui;

fn run(ui: &mut Ui) -> Result<(), CommandError> {
    let app = default_app();
    let (command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    run_command(ui, &command_helper, &matches)
}

fn main() {
    let mut ui = create_ui();
    let exit_code = match run(&mut ui) {
        Ok(()) => 0,
        Err(err) => report_command_error(&mut ui, err),
    };
    std::process::exit(exit_code);
}
