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
use std::path::Path;

use chrono::DateTime;

use crate::backend::{Signature, Timestamp};

#[derive(Debug, Clone, Default)]
pub struct UserSettings {
    config: config::Config,
    timestamp: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct RepoSettings {
    _config: config::Config,
}

const TOO_MUCH_CONFIG_ERROR: &str =
    "Both `$HOME/.jjconfig` and `$XDG_CONFIG_HOME/jj/config.toml` were found, please remove one.";

impl UserSettings {
    pub fn from_config(config: config::Config) -> Self {
        let timestamp = match config.get_string("user.timestamp") {
            Ok(timestamp_str) => match DateTime::parse_from_rfc3339(&timestamp_str) {
                Ok(datetime) => Some(Timestamp::from_datetime(datetime)),
                Err(_) => None,
            },
            Err(_) => None,
        };
        UserSettings { config, timestamp }
    }

    pub fn for_user() -> Result<Self, config::ConfigError> {
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
        Ok(Self::from_config(config))
    }

    pub fn with_repo(&self, repo_path: &Path) -> Result<RepoSettings, config::ConfigError> {
        let config = config::Config::builder()
            .add_source(self.config.clone())
            .add_source(
                config::File::from(repo_path.join("config"))
                    .required(false)
                    .format(config::FileFormat::Toml),
            )
            .build()?;
        Ok(RepoSettings { _config: config })
    }

    pub fn user_name(&self) -> String {
        self.config
            .get_string("user.name")
            .unwrap_or_else(|_| "(no name configured)".to_string())
    }

    pub fn user_email(&self) -> String {
        self.config
            .get_string("user.email")
            .unwrap_or_else(|_| "(no email configured)".to_string())
    }

    pub fn signature(&self) -> Signature {
        let timestamp = self.timestamp.clone().unwrap_or_else(Timestamp::now);
        Signature {
            name: self.user_name(),
            email: self.user_email(),
            timestamp,
        }
    }

    pub fn config(&self) -> &config::Config {
        &self.config
    }
}
