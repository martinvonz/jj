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

fn run(ui: &mut Ui) -> Result<(), CommandError> {
    let app = default_app();
    let (command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    run_command(ui, &command_helper, &matches)
}

fn main() {
    let (mut ui, result) = create_ui();
    let result = result.and_then(|()| run(&mut ui));
    let exit_code = handle_command_result(&mut ui, result);
    std::process::exit(exit_code);
}
