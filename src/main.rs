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

use jujutsu::commands::{dispatch, CommandError};
use jujutsu::config::read_config;
use jujutsu::ui::Ui;
use jujutsu_lib::settings::UserSettings;

fn main() {
    // TODO: We need to do some argument parsing here, at least for things like
    // --config,       and for reading user configs from the repo pointed to by
    // -R.
    match read_config() {
        Ok(user_settings) => {
            let mut ui = Ui::for_terminal(user_settings);
            match dispatch(&mut ui, std::env::args_os()) {
                Ok(_) => {
                    std::process::exit(0);
                }
                Err(CommandError::UserError(message)) => {
                    ui.write_error(&format!("Error: {}\n", message)).unwrap();
                    std::process::exit(1);
                }
                Err(CommandError::CliError(message)) => {
                    ui.write_error(&format!("Error: {}\n", message)).unwrap();
                    std::process::exit(2);
                }
                Err(CommandError::BrokenPipe) => {
                    std::process::exit(3);
                }
                Err(CommandError::InternalError(message)) => {
                    ui.write_error(&format!("Internal error: {}\n", message))
                        .unwrap();
                    std::process::exit(255);
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
