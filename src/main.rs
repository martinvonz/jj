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

use jujutsu::cli_util::{parse_args, report_command_error, CommandError};
use jujutsu::commands::{default_app, run_command};
use jujutsu::config::read_config;
use jujutsu::ui::Ui;
use jujutsu_lib::settings::UserSettings;

fn run(ui: &mut Ui) -> Result<(), CommandError> {
    let app = default_app();
    let (command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    run_command(ui, &command_helper, &matches)
}

fn main() {
    // TODO: We need to do some argument parsing here, at least for things like
    // --config,       and for reading user configs from the repo pointed to by
    // -R.
    match read_config() {
        Ok(user_settings) => {
            let mut ui = Ui::for_terminal(user_settings);
            match run(&mut ui) {
                Ok(()) => {
                    std::process::exit(0);
                }
                Err(err) => {
                    std::process::exit(report_command_error(&mut ui, err));
                }
            }
        }
        Err(err) => {
            let mut ui = Ui::for_terminal(UserSettings::default());
            ui.write_error(&format!("Config error: {}\n", err)).unwrap();
            std::process::exit(1);
        }
    }
}
