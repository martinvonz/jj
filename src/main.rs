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

use std::env;

use jujutsu::commands::dispatch;
use jujutsu::ui::Ui;
use jujutsu_lib::settings::UserSettings;

const TOO_MUCH_CONFIG_ERROR: &str =
    "Both `$HOME/.jjconfig` and `$XDG_CONFIG_HOME/jj/config.toml` were found, please remove one.";

fn read_config() -> Result<UserSettings, config::ConfigError> {
    let mut config_builder = config::Config::builder();

    let loaded_from_config_dir = match dirs::config_dir() {
        None => false,
        Some(config_dir) => {
            let p = config_dir.join("jj/config.toml");
            let exists = p.exists();
            config_builder = config_builder.add_source(
                config::File::from(p)
                    .required(false)
                    .format(config::FileFormat::Toml),
            );
            exists
        }
    };

    if let Some(home_dir) = dirs::home_dir() {
        let p = home_dir.join(".jjconfig");
        // we already loaded from the new location, prevent user confusion and make them
        // remove the old one:
        if loaded_from_config_dir && p.exists() {
            return Err(config::ConfigError::Message(
                TOO_MUCH_CONFIG_ERROR.to_string(),
            ));
        }
        config_builder = config_builder.add_source(
            config::File::from(p)
                .required(false)
                .format(config::FileFormat::Toml),
        );
    }

    // TODO: Make the config from environment a separate source instead? Seems
    // cleaner to separate it like that, especially if the config::Config instance
    // can keep track of where the config comes from then (it doesn't seem like it
    // can, however - we don't give a name or anything to the Config object).
    if let Ok(value) = env::var("JJ_USER") {
        config_builder = config_builder.set_override("user.name", value)?;
    }
    if let Ok(value) = env::var("JJ_EMAIL") {
        config_builder = config_builder.set_override("user.email", value)?;
    }
    if let Ok(value) = env::var("JJ_TIMESTAMP") {
        config_builder = config_builder.set_override("user.timestamp", value)?;
    }

    let config = config_builder.build()?;
    Ok(UserSettings::from_config(config))
}

fn main() {
    // TODO: We need to do some argument parsing here, at least for things like
    // --config,       and for reading user configs from the repo pointed to by
    // -R.
    match read_config() {
        Ok(user_settings) => {
            let ui = Ui::for_terminal(user_settings);
            let status = dispatch(ui, &mut std::env::args_os());
            std::process::exit(status);
        }
        Err(err) => {
            let mut ui = Ui::for_terminal(UserSettings::default());
            ui.write_error(&format!("Invalid config: {}\n", err))
                .unwrap();
            std::process::exit(1);
        }
    }
}
