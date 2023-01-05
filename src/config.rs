// Copyright 2022 The Jujutsu Authors
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

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fmt};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error(transparent)]
    ConfigReadError(#[from] config::ConfigError),
    #[error("Both {0} and {1} exist. Please consolidate your configs in one of them.")]
    AmbiguousSource(PathBuf, PathBuf),
}

/// Set of configs which can be merged as needed.
///
/// Sources from the lowest precedence:
/// 1. Default
/// 2. Base environment variables
/// 3. User config `~/.jjconfig.toml` or `$JJ_CONFIG`
/// 4. TODO: Repo config `.jj/repo/config.toml`
/// 5. TODO: Workspace config `.jj/config.toml`
/// 6. Override environment variables
/// 7. Command-line arguments `--config-toml`
#[derive(Clone, Debug)]
pub struct LayeredConfigs {
    default: config::Config,
    env_base: config::Config,
    user: Option<config::Config>,
    env_overrides: config::Config,
    arg_overrides: Option<config::Config>,
}

impl LayeredConfigs {
    /// Initializes configs with infallible sources.
    pub fn from_environment() -> Self {
        LayeredConfigs {
            default: default_config(),
            env_base: env_base(),
            user: None,
            env_overrides: env_overrides(),
            arg_overrides: None,
        }
    }

    pub fn read_user_config(&mut self) -> Result<(), ConfigError> {
        self.user = config_path()?
            .map(|path| read_config_path(&path))
            .transpose()?;
        Ok(())
    }

    pub fn parse_config_args(&mut self, toml_strs: &[String]) -> Result<(), ConfigError> {
        let config = toml_strs
            .iter()
            .fold(config::Config::builder(), |builder, s| {
                builder.add_source(config::File::from_str(s, config::FileFormat::Toml))
            })
            .build()?;
        self.arg_overrides = Some(config);
        Ok(())
    }

    /// Creates new merged config.
    pub fn merge(&self) -> config::Config {
        let config_sources = [
            Some(&self.default),
            Some(&self.env_base),
            self.user.as_ref(),
            Some(&self.env_overrides),
            self.arg_overrides.as_ref(),
        ];
        config_sources
            .into_iter()
            .flatten()
            .fold(config::Config::builder(), |builder, source| {
                builder.add_source(source.clone())
            })
            .build()
            .expect("loaded configs should be merged without error")
    }
}

fn config_path() -> Result<Option<PathBuf>, ConfigError> {
    if let Ok(config_path) = env::var("JJ_CONFIG") {
        // TODO: We should probably support colon-separated (std::env::split_paths)
        // paths here
        Ok(Some(PathBuf::from(config_path)))
    } else {
        // TODO: Should we drop the final `/config.toml` and read all files in the
        // directory?
        let platform_specific_config_path = dirs::config_dir()
            .map(|config_dir| config_dir.join("jj").join("config.toml"))
            .filter(|path| path.exists());
        let home_config_path = dirs::home_dir()
            .map(|home_dir| home_dir.join(".jjconfig.toml"))
            .filter(|path| path.exists());
        match (&platform_specific_config_path, &home_config_path) {
            (Some(xdg_config_path), Some(home_config_path)) => Err(ConfigError::AmbiguousSource(
                xdg_config_path.clone(),
                home_config_path.clone(),
            )),
            _ => Ok(platform_specific_config_path.or(home_config_path)),
        }
    }
}

/// Environment variables that should be overridden by config values
fn env_base() -> config::Config {
    let mut builder = config::Config::builder();
    if env::var("NO_COLOR").is_ok() {
        // "User-level configuration files and per-instance command-line arguments
        // should override $NO_COLOR." https://no-color.org/
        builder = builder.set_override("ui.color", "never").unwrap();
    }
    if let Ok(value) = env::var("PAGER") {
        builder = builder.set_override("ui.pager", value).unwrap();
    }
    if let Ok(value) = env::var("VISUAL") {
        builder = builder.set_override("ui.editor", value).unwrap();
    } else if let Ok(value) = env::var("EDITOR") {
        builder = builder.set_override("ui.editor", value).unwrap();
    }

    builder.build().unwrap()
}

pub fn default_config() -> config::Config {
    // Syntax error in default config isn't a user error. That's why defaults are
    // loaded by separate builder.
    config::Config::builder()
        .add_source(config::File::from_str(
            include_str!("config/colors.toml"),
            config::FileFormat::Toml,
        ))
        .add_source(config::File::from_str(
            include_str!("config/merge_tools.toml"),
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
}

/// Environment variables that override config values
fn env_overrides() -> config::Config {
    let mut builder = config::Config::builder();
    if let Ok(value) = env::var("JJ_USER") {
        builder = builder.set_override("user.name", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_EMAIL") {
        builder = builder.set_override("user.email", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_TIMESTAMP") {
        builder = builder
            .set_override("debug.commit-timestamp", value)
            .unwrap();
    }
    if let Ok(value) = env::var("JJ_RANDOMNESS_SEED") {
        builder = builder
            .set_override("debug.randomness-seed", value)
            .unwrap();
    }
    if let Ok(value) = env::var("JJ_OP_TIMESTAMP") {
        builder = builder
            .set_override("debug.operation-timestamp", value)
            .unwrap();
    }
    if let Ok(value) = env::var("JJ_OP_HOSTNAME") {
        builder = builder.set_override("operation.hostname", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_OP_USERNAME") {
        builder = builder.set_override("operation.username", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_EDITOR") {
        builder = builder.set_override("ui.editor", value).unwrap();
    }
    builder.build().unwrap()
}

fn read_config_path(config_path: &Path) -> Result<config::Config, config::ConfigError> {
    let mut files = vec![];
    if config_path.is_dir() {
        if let Ok(read_dir) = config_path.read_dir() {
            // TODO: Walk the directory recursively?
            for dir_entry in read_dir.flatten() {
                let path = dir_entry.path();
                if path.is_file() {
                    files.push(path);
                }
            }
        }
        files.sort();
    } else {
        files.push(config_path.to_owned());
    }

    files
        .iter()
        .fold(config::Config::builder(), |builder, path| {
            // TODO: Accept other formats and/or accept only certain file extensions?
            builder.add_source(
                config::File::from(path.as_ref())
                    .required(false)
                    .format(config::FileFormat::Toml),
            )
        })
        .build()
}

/// Command name and arguments specified by config.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize)]
#[serde(untagged)]
pub enum FullCommandArgs {
    String(String),
    Vec(NonEmptyCommandArgsVec),
}

impl FullCommandArgs {
    /// Returns arguments including the command name.
    ///
    /// The list is not empty, but each element may be an empty string.
    pub fn args(&self) -> Cow<[String]> {
        match self {
            // Handle things like `EDITOR=emacs -nw` (TODO: parse shell escapes)
            FullCommandArgs::String(s) => s.split(' ').map(|s| s.to_owned()).collect(),
            FullCommandArgs::Vec(a) => Cow::Borrowed(&a.0),
        }
    }

    /// Returns process builder configured with this.
    pub fn to_command(&self) -> Command {
        let full_args = self.args();
        let mut cmd = Command::new(&full_args[0]);
        cmd.args(&full_args[1..]);
        cmd
    }
}

impl<T: AsRef<str> + ?Sized> From<&T> for FullCommandArgs {
    fn from(s: &T) -> Self {
        FullCommandArgs::String(s.as_ref().to_owned())
    }
}

impl fmt::Display for FullCommandArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FullCommandArgs::String(s) => write!(f, "{s}"),
            // TODO: format with shell escapes
            FullCommandArgs::Vec(a) => write!(f, "{}", a.0.join(" ")),
        }
    }
}

/// Wrapper to reject an array without command name.
// Based on https://github.com/serde-rs/serde/issues/939
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize)]
#[serde(try_from = "Vec<String>")]
pub struct NonEmptyCommandArgsVec(Vec<String>);

impl TryFrom<Vec<String>> for NonEmptyCommandArgsVec {
    type Error = &'static str;

    fn try_from(args: Vec<String>) -> Result<Self, Self::Error> {
        if args.is_empty() {
            Err("command arguments should not be empty")
        } else {
            Ok(NonEmptyCommandArgsVec(args))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_args() {
        let config = config::Config::builder()
            .set_override("empty_array", Vec::<String>::new())
            .unwrap()
            .set_override("empty_string", "")
            .unwrap()
            .set_override("array", vec!["emacs", "-nw"])
            .unwrap()
            .set_override("string", "emacs -nw")
            .unwrap()
            .build()
            .unwrap();

        assert!(config.get::<FullCommandArgs>("empty_array").is_err());

        let args: FullCommandArgs = config.get("empty_string").unwrap();
        assert_eq!(args, FullCommandArgs::String("".to_owned()));
        assert_eq!(args.args(), [""].as_ref());

        let args: FullCommandArgs = config.get("array").unwrap();
        assert_eq!(
            args,
            FullCommandArgs::Vec(NonEmptyCommandArgsVec(
                ["emacs", "-nw",].map(|s| s.to_owned()).to_vec()
            ))
        );
        assert_eq!(args.args(), ["emacs", "-nw"].as_ref());

        let args: FullCommandArgs = config.get("string").unwrap();
        assert_eq!(args, FullCommandArgs::String("emacs -nw".to_owned()));
        assert_eq!(args.args(), ["emacs", "-nw"].as_ref());
    }
}
