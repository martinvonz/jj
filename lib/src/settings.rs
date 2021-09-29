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

use std::path::Path;

#[derive(Debug, Clone)]
pub struct UserSettings {
    config: config::Config,
}

#[derive(Debug, Clone)]
pub struct RepoSettings {
    _config: config::Config,
}

impl UserSettings {
    pub fn from_config(config: config::Config) -> Self {
        UserSettings { config }
    }

    pub fn for_user() -> Result<Self, config::ConfigError> {
        let mut config = config::Config::new();

        if let Some(home_dir) = dirs::home_dir() {
            config.merge(
                config::File::from(home_dir.join(".jjconfig"))
                    .required(false)
                    .format(config::FileFormat::Toml),
            )?;
        }

        Ok(UserSettings { config })
    }

    pub fn with_repo(&self, repo_path: &Path) -> Result<RepoSettings, config::ConfigError> {
        let mut config = self.config.clone();
        config.merge(
            config::File::from(repo_path.join("config"))
                .required(false)
                .format(config::FileFormat::Toml),
        )?;

        Ok(RepoSettings { _config: config })
    }

    pub fn user_name(&self) -> String {
        self.config
            .get_str("user.name")
            .unwrap_or_else(|_| "(no name configured)".to_string())
    }

    pub fn user_email(&self) -> String {
        self.config
            .get_str("user.email")
            .unwrap_or_else(|_| "(no email configured)".to_string())
    }

    pub fn config(&self) -> &config::Config {
        &self.config
    }
}
