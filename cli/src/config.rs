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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fmt};

use config::Source;
use itertools::Itertools;
use jj_lib::settings::ConfigResultExt as _;
use thiserror::Error;
use tracing::instrument;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error(transparent)]
    ConfigReadError(#[from] config::ConfigError),
    #[error("Both {0} and {1} exist. Please consolidate your configs in one of them.")]
    AmbiguousSource(PathBuf, PathBuf),
    #[error(transparent)]
    ConfigCreateError(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    Default,
    Env,
    // TODO: Track explicit file paths, especially for when user config is a dir.
    User,
    Repo,
    CommandArg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotatedValue {
    pub path: Vec<String>,
    pub value: config::Value,
    pub source: ConfigSource,
    pub is_overridden: bool,
}

/// Set of configs which can be merged as needed.
///
/// Sources from the lowest precedence:
/// 1. Default
/// 2. Base environment variables
/// 3. [User config](https://github.com/martinvonz/jj/blob/main/docs/config.md#configuration)
/// 4. Repo config `.jj/repo/config.toml`
/// 5. TODO: Workspace config `.jj/config.toml`
/// 6. Override environment variables
/// 7. Command-line arguments `--config-toml`
#[derive(Clone, Debug)]
pub struct LayeredConfigs {
    default: config::Config,
    env_base: config::Config,
    user: Option<config::Config>,
    repo: Option<config::Config>,
    env_overrides: config::Config,
    arg_overrides: Option<config::Config>,
}

impl LayeredConfigs {
    /// Initializes configs with infallible sources.
    pub fn from_environment(default: config::Config) -> Self {
        LayeredConfigs {
            default,
            env_base: env_base(),
            user: None,
            repo: None,
            env_overrides: env_overrides(),
            arg_overrides: None,
        }
    }

    #[instrument]
    pub fn read_user_config(&mut self) -> Result<(), ConfigError> {
        self.user = existing_config_path()?
            .map(|path| read_config_path(&path))
            .transpose()?;
        Ok(())
    }

    #[instrument]
    pub fn read_repo_config(&mut self, repo_path: &Path) -> Result<(), ConfigError> {
        self.repo = Some(read_config_file(&repo_path.join("config.toml"))?);
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
        self.sources()
            .into_iter()
            .map(|(_, config)| config)
            .fold(config::Config::builder(), |builder, source| {
                builder.add_source(source.clone())
            })
            .build()
            .expect("loaded configs should be merged without error")
    }

    pub fn sources(&self) -> Vec<(ConfigSource, &config::Config)> {
        let config_sources = [
            (ConfigSource::Default, Some(&self.default)),
            (ConfigSource::Env, Some(&self.env_base)),
            (ConfigSource::User, self.user.as_ref()),
            (ConfigSource::Repo, self.repo.as_ref()),
            (ConfigSource::Env, Some(&self.env_overrides)),
            (ConfigSource::CommandArg, self.arg_overrides.as_ref()),
        ];
        config_sources
            .into_iter()
            .filter_map(|(source, config)| config.map(|c| (source, c)))
            .collect_vec()
    }

    pub fn resolved_config_values(
        &self,
        filter_prefix: &[&str],
    ) -> Result<Vec<AnnotatedValue>, ConfigError> {
        // Collect annotated values from each config.
        let mut config_vals = vec![];

        let prefix_key = match filter_prefix {
            &[] => None,
            _ => Some(filter_prefix.join(".")),
        };
        for (source, config) in self.sources() {
            let top_value = match prefix_key {
                Some(ref key) => {
                    if let Some(val) = config.get(key).optional()? {
                        val
                    } else {
                        continue;
                    }
                }
                None => config.collect()?.into(),
            };
            let mut config_stack: Vec<(Vec<&str>, &config::Value)> =
                vec![(filter_prefix.to_vec(), &top_value)];
            while let Some((path, value)) = config_stack.pop() {
                match &value.kind {
                    config::ValueKind::Table(table) => {
                        // TODO: Remove sorting when config crate maintains deterministic ordering.
                        for (k, v) in table.iter().sorted_by_key(|(k, _)| *k).rev() {
                            let mut key_path = path.to_vec();
                            key_path.push(k);
                            config_stack.push((key_path, v));
                        }
                    }
                    _ => {
                        config_vals.push(AnnotatedValue {
                            path: path.iter().map(|&s| s.to_owned()).collect_vec(),
                            value: value.to_owned(),
                            source: source.to_owned(),
                            // Note: Value updated below.
                            is_overridden: false,
                        });
                    }
                }
            }
        }

        // Walk through config values in reverse order and mark each overridden value as
        // overridden.
        let mut keys_found = HashSet::new();
        for val in config_vals.iter_mut().rev() {
            val.is_overridden = !keys_found.insert(&val.path);
        }

        Ok(config_vals)
    }
}

enum ConfigPath {
    /// Existing config file path.
    Existing(PathBuf),
    /// Could not find any config file, but a new file can be created at the
    /// specified location.
    New(PathBuf),
    /// Could not find any config file.
    Unavailable,
}

impl ConfigPath {
    fn new(path: Option<PathBuf>) -> Self {
        match path {
            Some(path) if path.exists() => ConfigPath::Existing(path),
            Some(path) => ConfigPath::New(path),
            None => ConfigPath::Unavailable,
        }
    }
}

/// Like std::fs::create_dir_all but creates new directories to be accessible to
/// the user only on Unix (chmod 700).
fn create_dir_all(path: &Path) -> std::io::Result<()> {
    let mut dir = std::fs::DirBuilder::new();
    dir.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dir.mode(0o700);
    }
    dir.create(path)
}

fn create_config_file(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    // TODO: Use File::create_new once stabilized.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

// The struct exists so that we can mock certain global values in unit tests.
#[derive(Clone, Default, Debug)]
struct ConfigEnv {
    config_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    jj_config: Option<String>,
}

impl ConfigEnv {
    fn new() -> Self {
        ConfigEnv {
            config_dir: dirs::config_dir(),
            home_dir: dirs::home_dir(),
            jj_config: env::var("JJ_CONFIG").ok(),
        }
    }

    fn config_path(self) -> Result<ConfigPath, ConfigError> {
        if let Some(path) = self.jj_config {
            // TODO: We should probably support colon-separated (std::env::split_paths)
            return Ok(ConfigPath::new(Some(PathBuf::from(path))));
        }
        // TODO: Should we drop the final `/config.toml` and read all files in the
        // directory?
        let platform_config_path = ConfigPath::new(self.config_dir.map(|mut config_dir| {
            config_dir.push("jj");
            config_dir.push("config.toml");
            config_dir
        }));
        let home_config_path = ConfigPath::new(self.home_dir.map(|mut home_dir| {
            home_dir.push(".jjconfig.toml");
            home_dir
        }));
        use ConfigPath::*;
        match (platform_config_path, home_config_path) {
            (Existing(platform_config_path), Existing(home_config_path)) => Err(
                ConfigError::AmbiguousSource(platform_config_path, home_config_path),
            ),
            (Existing(path), _) | (_, Existing(path)) => Ok(Existing(path)),
            (New(path), _) | (_, New(path)) => Ok(New(path)),
            (Unavailable, Unavailable) => Ok(Unavailable),
        }
    }

    fn existing_config_path(self) -> Result<Option<PathBuf>, ConfigError> {
        match self.config_path()? {
            ConfigPath::Existing(path) => Ok(Some(path)),
            _ => Ok(None),
        }
    }

    fn new_config_path(self) -> Result<Option<PathBuf>, ConfigError> {
        match self.config_path()? {
            ConfigPath::Existing(path) => Ok(Some(path)),
            ConfigPath::New(path) => {
                create_config_file(&path)?;
                Ok(Some(path))
            }
            ConfigPath::Unavailable => Ok(None),
        }
    }
}

pub fn existing_config_path() -> Result<Option<PathBuf>, ConfigError> {
    ConfigEnv::new().existing_config_path()
}

/// Returns a path to the user-specific config file. If no config file is found,
/// tries to guess a reasonable new location for it. If a path to a new config
/// file is returned, the parent directory may be created as a result of this
/// call.
pub fn new_config_path() -> Result<Option<PathBuf>, ConfigError> {
    ConfigEnv::new().new_config_path()
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
    macro_rules! from_toml {
        ($file:literal) => {
            config::File::from_str(include_str!($file), config::FileFormat::Toml)
        };
    }
    let mut builder = config::Config::builder()
        .add_source(from_toml!("config/colors.toml"))
        .add_source(from_toml!("config/merge_tools.toml"))
        .add_source(from_toml!("config/misc.toml"))
        .add_source(from_toml!("config/revsets.toml"))
        .add_source(from_toml!("config/templates.toml"));
    if cfg!(unix) {
        builder = builder.add_source(from_toml!("config/unix.toml"))
    }
    if cfg!(windows) {
        builder = builder.add_source(from_toml!("config/windows.toml"))
    }
    builder.build().unwrap()
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

fn read_config_file(path: &Path) -> Result<config::Config, config::ConfigError> {
    config::Config::builder()
        .add_source(
            config::File::from(path)
                .required(false)
                .format(config::FileFormat::Toml),
        )
        .build()
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
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(untagged)]
pub enum CommandNameAndArgs {
    String(String),
    Vec(NonEmptyCommandArgsVec),
    Structured {
        env: HashMap<String, String>,
        command: NonEmptyCommandArgsVec,
    },
}

impl CommandNameAndArgs {
    /// Returns command name without arguments.
    pub fn split_name(&self) -> Cow<str> {
        let (name, _) = self.split_name_and_args();
        name
    }

    /// Returns command name and arguments.
    ///
    /// The command name may be an empty string (as well as each argument.)
    pub fn split_name_and_args(&self) -> (Cow<str>, Cow<[String]>) {
        match self {
            CommandNameAndArgs::String(s) => {
                // Handle things like `EDITOR=emacs -nw` (TODO: parse shell escapes)
                let mut args = s.split(' ').map(|s| s.to_owned());
                (args.next().unwrap().into(), args.collect())
            }
            CommandNameAndArgs::Vec(NonEmptyCommandArgsVec(a)) => {
                (Cow::Borrowed(&a[0]), Cow::Borrowed(&a[1..]))
            }
            CommandNameAndArgs::Structured {
                env: _,
                command: cmd,
            } => (Cow::Borrowed(&cmd.0[0]), Cow::Borrowed(&cmd.0[1..])),
        }
    }

    /// Returns process builder configured with this.
    pub fn to_command(&self) -> Command {
        let (name, args) = self.split_name_and_args();
        let mut cmd = Command::new(name.as_ref());
        if let CommandNameAndArgs::Structured { env, .. } = self {
            cmd.envs(env);
        }
        cmd.args(args.as_ref());
        cmd
    }
}

impl<T: AsRef<str> + ?Sized> From<&T> for CommandNameAndArgs {
    fn from(s: &T) -> Self {
        CommandNameAndArgs::String(s.as_ref().to_owned())
    }
}

impl fmt::Display for CommandNameAndArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandNameAndArgs::String(s) => write!(f, "{s}"),
            // TODO: format with shell escapes
            CommandNameAndArgs::Vec(a) => write!(f, "{}", a.0.join(" ")),
            CommandNameAndArgs::Structured { env, command } => {
                for (k, v) in env.iter() {
                    write!(f, "{k}={v} ")?;
                }
                write!(f, "{}", command.0.join(" "))
            }
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
    use maplit::hashmap;

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
            .set_override("structured.env.KEY1", "value1")
            .unwrap()
            .set_override("structured.env.KEY2", "value2")
            .unwrap()
            .set_override("structured.command", vec!["emacs", "-nw"])
            .unwrap()
            .build()
            .unwrap();

        assert!(config.get::<CommandNameAndArgs>("empty_array").is_err());

        let command_args: CommandNameAndArgs = config.get("empty_string").unwrap();
        assert_eq!(command_args, CommandNameAndArgs::String("".to_owned()));
        let (name, args) = command_args.split_name_and_args();
        assert_eq!(name, "");
        assert!(args.is_empty());

        let command_args: CommandNameAndArgs = config.get("array").unwrap();
        assert_eq!(
            command_args,
            CommandNameAndArgs::Vec(NonEmptyCommandArgsVec(
                ["emacs", "-nw",].map(|s| s.to_owned()).to_vec()
            ))
        );
        let (name, args) = command_args.split_name_and_args();
        assert_eq!(name, "emacs");
        assert_eq!(args, ["-nw"].as_ref());

        let command_args: CommandNameAndArgs = config.get("string").unwrap();
        assert_eq!(
            command_args,
            CommandNameAndArgs::String("emacs -nw".to_owned())
        );
        let (name, args) = command_args.split_name_and_args();
        assert_eq!(name, "emacs");
        assert_eq!(args, ["-nw"].as_ref());

        let command_args: CommandNameAndArgs = config.get("structured").unwrap();
        assert_eq!(
            command_args,
            CommandNameAndArgs::Structured {
                env: hashmap! {
                    "KEY1".to_string() => "value1".to_string(),
                    "KEY2".to_string() => "value2".to_string(),
                },
                command: NonEmptyCommandArgsVec(["emacs", "-nw",].map(|s| s.to_owned()).to_vec())
            }
        );
        let (name, args) = command_args.split_name_and_args();
        assert_eq!(name, "emacs");
        assert_eq!(args, ["-nw"].as_ref());
    }

    #[test]
    fn test_layered_configs_resolved_config_values_empty() {
        let empty_config = config::Config::default();
        let layered_configs = LayeredConfigs {
            default: empty_config.to_owned(),
            env_base: empty_config.to_owned(),
            user: None,
            repo: None,
            env_overrides: empty_config,
            arg_overrides: None,
        };
        assert_eq!(layered_configs.resolved_config_values(&[]).unwrap(), []);
    }

    #[test]
    fn test_layered_configs_resolved_config_values_single_key() {
        let empty_config = config::Config::default();
        let env_base_config = config::Config::builder()
            .set_override("user.name", "base-user-name")
            .unwrap()
            .set_override("user.email", "base@user.email")
            .unwrap()
            .build()
            .unwrap();
        let repo_config = config::Config::builder()
            .set_override("user.email", "repo@user.email")
            .unwrap()
            .build()
            .unwrap();
        let layered_configs = LayeredConfigs {
            default: empty_config.to_owned(),
            env_base: env_base_config,
            user: None,
            repo: Some(repo_config),
            env_overrides: empty_config,
            arg_overrides: None,
        };
        // Note: "email" is alphabetized, before "name" from same layer.
        insta::assert_debug_snapshot!(
            layered_configs.resolved_config_values(&[]).unwrap(),
            @r###"
        [
            AnnotatedValue {
                path: [
                    "user",
                    "email",
                ],
                value: Value {
                    origin: None,
                    kind: String(
                        "base@user.email",
                    ),
                },
                source: Env,
                is_overridden: true,
            },
            AnnotatedValue {
                path: [
                    "user",
                    "name",
                ],
                value: Value {
                    origin: None,
                    kind: String(
                        "base-user-name",
                    ),
                },
                source: Env,
                is_overridden: false,
            },
            AnnotatedValue {
                path: [
                    "user",
                    "email",
                ],
                value: Value {
                    origin: None,
                    kind: String(
                        "repo@user.email",
                    ),
                },
                source: Repo,
                is_overridden: false,
            },
        ]
        "###
        );
    }

    #[test]
    fn test_layered_configs_resolved_config_values_filter_path() {
        let empty_config = config::Config::default();
        let user_config = config::Config::builder()
            .set_override("test-table1.foo", "user-FOO")
            .unwrap()
            .set_override("test-table2.bar", "user-BAR")
            .unwrap()
            .build()
            .unwrap();
        let repo_config = config::Config::builder()
            .set_override("test-table1.bar", "repo-BAR")
            .unwrap()
            .build()
            .unwrap();
        let layered_configs = LayeredConfigs {
            default: empty_config.to_owned(),
            env_base: empty_config.to_owned(),
            user: Some(user_config),
            repo: Some(repo_config),
            env_overrides: empty_config,
            arg_overrides: None,
        };
        insta::assert_debug_snapshot!(
            layered_configs
                .resolved_config_values(&["test-table1"])
                .unwrap(),
            @r###"
        [
            AnnotatedValue {
                path: [
                    "test-table1",
                    "foo",
                ],
                value: Value {
                    origin: None,
                    kind: String(
                        "user-FOO",
                    ),
                },
                source: User,
                is_overridden: false,
            },
            AnnotatedValue {
                path: [
                    "test-table1",
                    "bar",
                ],
                value: Value {
                    origin: None,
                    kind: String(
                        "repo-BAR",
                    ),
                },
                source: Repo,
                is_overridden: false,
            },
        ]
        "###
        );
    }

    #[test]
    fn test_config_path_home_dir_existing() -> anyhow::Result<()> {
        TestCase {
            files: vec!["home/.jjconfig.toml"],
            cfg: ConfigEnv {
                home_dir: Some("home".into()),
                ..Default::default()
            },
            want: Want::ExistingAndNew("home/.jjconfig.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_home_dir_new() -> anyhow::Result<()> {
        TestCase {
            files: vec![],
            cfg: ConfigEnv {
                home_dir: Some("home".into()),
                ..Default::default()
            },
            want: Want::New("home/.jjconfig.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_config_dir_existing() -> anyhow::Result<()> {
        TestCase {
            files: vec!["config/jj/config.toml"],
            cfg: ConfigEnv {
                config_dir: Some("config".into()),
                ..Default::default()
            },
            want: Want::ExistingAndNew("config/jj/config.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_config_dir_new() -> anyhow::Result<()> {
        TestCase {
            files: vec![],
            cfg: ConfigEnv {
                config_dir: Some("config".into()),
                ..Default::default()
            },
            want: Want::New("config/jj/config.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_new_prefer_config_dir() -> anyhow::Result<()> {
        TestCase {
            files: vec![],
            cfg: ConfigEnv {
                config_dir: Some("config".into()),
                home_dir: Some("home".into()),
                ..Default::default()
            },
            want: Want::New("config/jj/config.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_jj_config_existing() -> anyhow::Result<()> {
        TestCase {
            files: vec!["custom.toml"],
            cfg: ConfigEnv {
                jj_config: Some("custom.toml".into()),
                ..Default::default()
            },
            want: Want::ExistingAndNew("custom.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_jj_config_new() -> anyhow::Result<()> {
        TestCase {
            files: vec![],
            cfg: ConfigEnv {
                jj_config: Some("custom.toml".into()),
                ..Default::default()
            },
            want: Want::New("custom.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_config_pick_config_dir() -> anyhow::Result<()> {
        TestCase {
            files: vec!["config/jj/config.toml"],
            cfg: ConfigEnv {
                home_dir: Some("home".into()),
                config_dir: Some("config".into()),
                ..Default::default()
            },
            want: Want::ExistingAndNew("config/jj/config.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_config_pick_home_dir() -> anyhow::Result<()> {
        TestCase {
            files: vec!["home/.jjconfig.toml"],
            cfg: ConfigEnv {
                home_dir: Some("home".into()),
                config_dir: Some("config".into()),
                ..Default::default()
            },
            want: Want::ExistingAndNew("home/.jjconfig.toml"),
        }
        .run()
    }

    #[test]
    fn test_config_path_none() -> anyhow::Result<()> {
        TestCase {
            files: vec![],
            cfg: Default::default(),
            want: Want::None,
        }
        .run()
    }

    #[test]
    fn test_config_path_ambiguous() -> anyhow::Result<()> {
        let tmp = setup_config_fs(&vec!["home/.jjconfig.toml", "config/jj/config.toml"])?;
        let cfg = ConfigEnv {
            home_dir: Some(tmp.path().join("home")),
            config_dir: Some(tmp.path().join("config")),
            ..Default::default()
        };
        use assert_matches::assert_matches;
        assert_matches!(
            cfg.clone().existing_config_path(),
            Err(ConfigError::AmbiguousSource(_, _))
        );
        assert_matches!(
            cfg.clone().new_config_path(),
            Err(ConfigError::AmbiguousSource(_, _))
        );
        Ok(())
    }

    fn setup_config_fs(files: &Vec<&'static str>) -> anyhow::Result<tempfile::TempDir> {
        let tmp = testutils::new_temp_dir();
        for file in files {
            let path = tmp.path().join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::File::create(path)?;
        }
        Ok(tmp)
    }

    enum Want {
        None,
        New(&'static str),
        ExistingAndNew(&'static str),
    }

    struct TestCase {
        files: Vec<&'static str>,
        cfg: ConfigEnv,
        want: Want,
    }

    impl TestCase {
        fn config(&self, root: &Path) -> ConfigEnv {
            ConfigEnv {
                config_dir: self.cfg.config_dir.as_ref().map(|p| root.join(p)),
                home_dir: self.cfg.home_dir.as_ref().map(|p| root.join(p)),
                jj_config: self
                    .cfg
                    .jj_config
                    .as_ref()
                    .map(|p| root.join(p).to_str().unwrap().to_string()),
            }
        }

        fn run(self) -> anyhow::Result<()> {
            use anyhow::anyhow;
            let tmp = setup_config_fs(&self.files)?;

            let check = |name, f: fn(ConfigEnv) -> Result<_, _>, want: Option<_>| {
                let got = f(self.config(tmp.path())).map_err(|e| anyhow!("{name}: {e}"))?;
                let want = want.map(|p| tmp.path().join(p));
                if got != want {
                    Err(anyhow!("{name}: got {got:?}, want {want:?}"))
                } else {
                    Ok(got)
                }
            };

            let (want_existing, want_new) = match self.want {
                Want::None => (None, None),
                Want::New(want) => (None, Some(want)),
                Want::ExistingAndNew(want) => (Some(want), Some(want)),
            };

            check(
                "existing_config_path",
                ConfigEnv::existing_config_path,
                want_existing,
            )?;
            let got = check("new_config_path", ConfigEnv::new_config_path, want_new)?;
            if let Some(path) = got {
                if !Path::new(&path).is_file() {
                    return Err(anyhow!(
                        "new_config_path returned {path:?} which is not a file"
                    ));
                }
            }
            Ok(())
        }
    }
}
