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
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use itertools::Itertools;
use jj_lib::config::ConfigError;
use jj_lib::config::ConfigLayer;
use jj_lib::config::ConfigNamePathBuf;
use jj_lib::config::ConfigSource;
use jj_lib::config::ConfigValue;
use jj_lib::config::StackedConfig;
use regex::Captures;
use regex::Regex;
use thiserror::Error;
use tracing::instrument;

use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::command_error::CommandError;

// TODO(#879): Consider generating entire schema dynamically vs. static file.
pub const CONFIG_SCHEMA: &str = include_str!("config-schema.json");

/// Parses a TOML value expression. Interprets the given value as string if it
/// can't be parsed.
pub fn parse_toml_value_or_bare_string(value_str: &str) -> toml_edit::Value {
    match value_str.parse() {
        Ok(value) => value,
        // TODO: might be better to reject meta characters. A typo in TOML value
        // expression shouldn't be silently converted to string.
        _ => value_str.into(),
    }
}

pub fn to_toml_value(value: &ConfigValue) -> Result<toml_edit::Value, ConfigError> {
    fn type_error<T: fmt::Display>(message: T) -> ConfigError {
        ConfigError::Message(message.to_string())
    }
    // It's unlikely that the config object contained unsupported values, but
    // there's no guarantee. For example, values coming from environment
    // variables might be big int.
    match value.kind {
        config::ValueKind::Nil => Err(type_error(format!("Unexpected value: {value}"))),
        config::ValueKind::Boolean(v) => Ok(v.into()),
        config::ValueKind::I64(v) => Ok(v.into()),
        config::ValueKind::I128(v) => Ok(i64::try_from(v).map_err(type_error)?.into()),
        config::ValueKind::U64(v) => Ok(i64::try_from(v).map_err(type_error)?.into()),
        config::ValueKind::U128(v) => Ok(i64::try_from(v).map_err(type_error)?.into()),
        config::ValueKind::Float(v) => Ok(v.into()),
        config::ValueKind::String(ref v) => Ok(v.into()),
        // TODO: Remove sorting when config crate maintains deterministic ordering.
        config::ValueKind::Table(ref table) => table
            .iter()
            .sorted_by_key(|(k, _)| *k)
            .map(|(k, v)| Ok((k, to_toml_value(v)?)))
            .collect(),
        config::ValueKind::Array(ref array) => array.iter().map(to_toml_value).collect(),
    }
}

#[derive(Error, Debug)]
pub enum ConfigEnvError {
    #[error(transparent)]
    ConfigReadError(#[from] ConfigError),
    #[error("Both {0} and {1} exist. Please consolidate your configs in one of them.")]
    AmbiguousSource(PathBuf, PathBuf),
    #[error(transparent)]
    ConfigCreateError(#[from] std::io::Error),
}

/// Configuration variable with its source information.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotatedValue {
    /// Dotted name path to the configuration variable.
    pub name: ConfigNamePathBuf,
    /// Configuration value.
    pub value: ConfigValue,
    /// Source of the configuration value.
    pub source: ConfigSource,
    // TODO: add source file path
    /// True if this value is overridden in higher precedence layers.
    pub is_overridden: bool,
}

/// Collects values under the given `filter_prefix` name recursively, from all
/// layers.
pub fn resolved_config_values(
    stacked_config: &StackedConfig,
    filter_prefix: &ConfigNamePathBuf,
) -> Vec<AnnotatedValue> {
    // Collect annotated values from each config.
    let mut config_vals = vec![];
    for layer in stacked_config.layers() {
        // TODO: Err(item) means all descendant paths are overridden by the
        // current layer. For example, the default ui.pager.<field> should be
        // marked as overridden if user had ui.pager = [...] set.
        let Ok(Some(top_item)) = layer.look_up_item(filter_prefix) else {
            continue;
        };
        let mut config_stack = vec![(filter_prefix.clone(), top_item)];
        while let Some((name, item)) = config_stack.pop() {
            match &item.kind {
                config::ValueKind::Table(table) => {
                    // TODO: Remove sorting when config crate maintains deterministic ordering.
                    for (k, v) in table.iter().sorted_by_key(|(k, _)| *k).rev() {
                        let mut sub_name = name.clone();
                        sub_name.push(k);
                        config_stack.push((sub_name, v));
                    }
                }
                _ => {
                    config_vals.push(AnnotatedValue {
                        name,
                        value: item.to_owned(),
                        source: layer.source,
                        // Note: Value updated below.
                        is_overridden: false,
                    });
                }
            }
        }
    }

    // Walk through config values in reverse order and mark each overridden value as
    // overridden.
    let mut names_found = HashSet::new();
    for val in config_vals.iter_mut().rev() {
        val.is_overridden = !names_found.insert(&val.name);
    }

    config_vals
}

#[derive(Clone, Debug)]
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
struct UnresolvedConfigEnv {
    config_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    jj_config: Option<String>,
}

impl UnresolvedConfigEnv {
    fn resolve(self) -> Result<ConfigPath, ConfigEnvError> {
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
                ConfigEnvError::AmbiguousSource(platform_config_path, home_config_path),
            ),
            (Existing(path), _) | (_, Existing(path)) => Ok(Existing(path)),
            (New(path), _) | (_, New(path)) => Ok(New(path)),
            (Unavailable, Unavailable) => Ok(Unavailable),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigEnv {
    user_config_path: ConfigPath,
    repo_config_path: ConfigPath,
}

impl ConfigEnv {
    /// Initializes configuration loader based on environment variables.
    pub fn from_environment() -> Result<Self, ConfigEnvError> {
        let env = UnresolvedConfigEnv {
            config_dir: dirs::config_dir(),
            home_dir: dirs::home_dir(),
            jj_config: env::var("JJ_CONFIG").ok(),
        };
        Ok(ConfigEnv {
            user_config_path: env.resolve()?,
            repo_config_path: ConfigPath::Unavailable,
        })
    }

    /// Returns a path to the existing user-specific config file or directory.
    pub fn existing_user_config_path(&self) -> Option<&Path> {
        match &self.user_config_path {
            ConfigPath::Existing(path) => Some(path),
            _ => None,
        }
    }

    /// Returns a path to the user-specific config file.
    ///
    /// If no config file is found, tries to guess a reasonable new location for
    /// it. If a path to a new config file is returned, the parent directory may
    /// be created as a result of this call.
    pub fn new_user_config_path(&self) -> Result<Option<&Path>, ConfigEnvError> {
        match &self.user_config_path {
            ConfigPath::Existing(path) => Ok(Some(path)),
            ConfigPath::New(path) => {
                // TODO: Maybe we shouldn't create new file here. Not all
                // callers need an empty file. For example, "jj config path"
                // should be a readonly operation. "jj config set" doesn't have
                // to create an empty file to be overwritten. Since it's unclear
                // who and when to update ConfigPath::New(_) to ::Existing(_),
                // it's probably better to not cache the path existence.
                create_config_file(path)?;
                Ok(Some(path))
            }
            ConfigPath::Unavailable => Ok(None),
        }
    }

    /// Loads user-specific config files into the given `config`. The old
    /// user-config layers will be replaced if any.
    #[instrument]
    pub fn reload_user_config(&self, config: &mut StackedConfig) -> Result<(), ConfigError> {
        config.remove_layers(ConfigSource::User);
        if let Some(path) = self.existing_user_config_path() {
            if path.is_dir() {
                config.load_dir(ConfigSource::User, path)?;
            } else {
                config.load_file(ConfigSource::User, path)?;
            }
        }
        Ok(())
    }

    /// Sets the directory where repo-specific config file is stored. The path
    /// is usually `.jj/repo`.
    pub fn reset_repo_path(&mut self, path: &Path) {
        self.repo_config_path = ConfigPath::new(Some(path.join("config.toml")));
    }

    /// Returns a path to the existing repo-specific config file.
    pub fn existing_repo_config_path(&self) -> Option<&Path> {
        match &self.repo_config_path {
            ConfigPath::Existing(path) => Some(path),
            _ => None,
        }
    }

    /// Returns a path to the repo-specific config file.
    pub fn new_repo_config_path(&self) -> Option<&Path> {
        match &self.repo_config_path {
            ConfigPath::Existing(path) => Some(path),
            ConfigPath::New(path) => Some(path),
            ConfigPath::Unavailable => None,
        }
    }

    /// Loads repo-specific config file into the given `config`. The old
    /// repo-config layer will be replaced if any.
    #[instrument]
    pub fn reload_repo_config(&self, config: &mut StackedConfig) -> Result<(), ConfigError> {
        config.remove_layers(ConfigSource::Repo);
        if let Some(path) = self.existing_repo_config_path() {
            config.load_file(ConfigSource::Repo, path)?;
        }
        Ok(())
    }
}

/// Initializes stacked config with the given `default` and infallible sources.
///
/// Sources from the lowest precedence:
/// 1. Default
/// 2. Base environment variables
/// 3. [User config](https://martinvonz.github.io/jj/latest/config/)
/// 4. Repo config `.jj/repo/config.toml`
/// 5. TODO: Workspace config `.jj/config.toml`
/// 6. Override environment variables
/// 7. Command-line arguments `--config-toml`
///
/// This function sets up 1, 2, and 6.
pub fn config_from_environment(default: config::Config) -> StackedConfig {
    let mut config = StackedConfig::empty();
    config.add_layer(ConfigLayer::with_data(ConfigSource::Default, default));
    config.add_layer(ConfigLayer::with_data(ConfigSource::EnvBase, env_base()));
    config.add_layer(ConfigLayer::with_data(
        ConfigSource::EnvOverrides,
        env_overrides(),
    ));
    config
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
        builder = builder.add_source(from_toml!("config/unix.toml"));
    }
    if cfg!(windows) {
        builder = builder.add_source(from_toml!("config/windows.toml"));
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
    if let Ok(Ok(value)) = env::var("JJ_RANDOMNESS_SEED").map(|s| s.parse::<i64>()) {
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

/// Parses `--config-toml` arguments.
pub fn parse_config_args(toml_strs: &[String]) -> Result<ConfigLayer, ConfigError> {
    let config = toml_strs
        .iter()
        .fold(config::Config::builder(), |builder, s| {
            builder.add_source(config::File::from_str(s, config::FileFormat::Toml))
        })
        .build()?;
    Ok(ConfigLayer::with_data(ConfigSource::CommandArg, config))
}

fn read_config(path: &Path) -> Result<toml_edit::ImDocument<String>, CommandError> {
    let config_toml = std::fs::read_to_string(path).or_else(|err| {
        match err.kind() {
            // If config doesn't exist yet, read as empty and we'll write one.
            std::io::ErrorKind::NotFound => Ok("".to_string()),
            _ => Err(user_error_with_message(
                format!("Failed to read file {path}", path = path.display()),
                err,
            )),
        }
    })?;
    config_toml.parse().map_err(|err| {
        user_error_with_message(
            format!("Failed to parse file {path}", path = path.display()),
            err,
        )
    })
}

fn write_config(path: &Path, doc: &toml_edit::DocumentMut) -> Result<(), CommandError> {
    std::fs::write(path, doc.to_string()).map_err(|err| {
        user_error_with_message(
            format!("Failed to write file {path}", path = path.display()),
            err,
        )
    })
}

pub fn write_config_value_to_file(
    key: &ConfigNamePathBuf,
    value: toml_edit::Value,
    path: &Path,
) -> Result<(), CommandError> {
    let mut doc = read_config(path)?.into_mut();

    // Apply config value
    let mut target_table = doc.as_table_mut();
    let mut key_parts_iter = key.components();
    let last_key_part = key_parts_iter.next_back().expect("key must not be empty");
    for key_part in key_parts_iter {
        target_table = target_table
            .entry(key_part)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                user_error(format!(
                    "Failed to set {key}: would overwrite non-table value with parent table"
                ))
            })?;
    }
    // Error out if overwriting non-scalar value for key (table or array) with
    // scalar.
    match target_table.get(last_key_part) {
        None | Some(toml_edit::Item::None | toml_edit::Item::Value(_)) => {}
        Some(toml_edit::Item::Table(_) | toml_edit::Item::ArrayOfTables(_)) => {
            return Err(user_error(format!(
                "Failed to set {key}: would overwrite entire table"
            )));
        }
    }
    target_table[last_key_part] = toml_edit::Item::Value(value);

    write_config(path, &doc)
}

pub fn remove_config_value_from_file(
    key: &ConfigNamePathBuf,
    path: &Path,
) -> Result<(), CommandError> {
    let mut doc = read_config(path)?.into_mut();

    // Find target table
    let mut key_iter = key.components();
    let last_key = key_iter.next_back().expect("key must not be empty");
    let target_table = key_iter.try_fold(doc.as_table_mut(), |table, key| {
        table
            .get_mut(key)
            .ok_or_else(|| ConfigError::NotFound(key.to_string()))
            .and_then(|table| {
                table
                    .as_table_mut()
                    .ok_or_else(|| ConfigError::Message(format!(r#""{key}" is not a table"#)))
            })
    })?;

    // Remove config value
    match target_table.entry(last_key) {
        toml_edit::Entry::Occupied(entry) => {
            if entry.get().is_table() {
                return Err(user_error(format!("Won't remove table {key}")));
            }
            entry.remove();
        }
        toml_edit::Entry::Vacant(_) => {
            return Err(ConfigError::NotFound(key.to_string()).into());
        }
    }

    write_config(path, &doc)
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
        let empty: HashMap<&str, &str> = HashMap::new();
        self.to_command_with_variables(&empty)
    }

    /// Returns process builder configured with this after interpolating
    /// variables into the arguments.
    pub fn to_command_with_variables<V: AsRef<str>>(
        &self,
        variables: &HashMap<&str, V>,
    ) -> Command {
        let (name, args) = self.split_name_and_args();
        let mut cmd = Command::new(name.as_ref());
        if let CommandNameAndArgs::Structured { env, .. } = self {
            cmd.envs(env);
        }
        cmd.args(interpolate_variables(&args, variables));
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
                for (k, v) in env {
                    write!(f, "{k}={v} ")?;
                }
                write!(f, "{}", command.0.join(" "))
            }
        }
    }
}

// Not interested in $UPPER_CASE_VARIABLES
static VARIABLE_REGEX: once_cell::sync::Lazy<Regex> =
    once_cell::sync::Lazy::new(|| Regex::new(r"\$([a-z0-9_]+)\b").unwrap());

pub fn interpolate_variables<V: AsRef<str>>(
    args: &[String],
    variables: &HashMap<&str, V>,
) -> Vec<String> {
    args.iter()
        .map(|arg| {
            VARIABLE_REGEX
                .replace_all(arg, |caps: &Captures| {
                    let name = &caps[1];
                    if let Some(subst) = variables.get(name) {
                        subst.as_ref().to_owned()
                    } else {
                        caps[0].to_owned()
                    }
                })
                .into_owned()
        })
        .collect()
}

/// Return all variable names found in the args, without the dollar sign
pub fn find_all_variables(args: &[String]) -> impl Iterator<Item = &str> {
    let regex = &*VARIABLE_REGEX;
    args.iter()
        .flat_map(|arg| regex.find_iter(arg))
        .map(|single_match| {
            let s = single_match.as_str();
            &s[1..]
        })
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
    use anyhow::anyhow;
    use assert_matches::assert_matches;
    use indoc::indoc;
    use maplit::hashmap;

    use super::*;

    #[test]
    fn test_command_args() {
        let mut config = StackedConfig::empty();
        config.add_layer(
            ConfigLayer::parse(
                ConfigSource::User,
                indoc! {"
                    empty_array = []
                    empty_string = ''
                    array = ['emacs', '-nw']
                    string = 'emacs -nw'
                    structured.env = { KEY1 = 'value1', KEY2 = 'value2' }
                    structured.command = ['emacs', '-nw']
                "},
            )
            .unwrap(),
        );

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
    fn test_resolved_config_values_empty() {
        let config = StackedConfig::empty();
        assert_eq!(
            resolved_config_values(&config, &ConfigNamePathBuf::root()),
            []
        );
    }

    #[test]
    fn test_resolved_config_values_single_key() {
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
        let mut config = StackedConfig::empty();
        config.add_layer(ConfigLayer::with_data(
            ConfigSource::EnvBase,
            env_base_config,
        ));
        config.add_layer(ConfigLayer::with_data(ConfigSource::Repo, repo_config));
        // Note: "email" is alphabetized, before "name" from same layer.
        insta::assert_debug_snapshot!(
            resolved_config_values(&config, &ConfigNamePathBuf::root()),
            @r#"
        [
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "user",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                        Key {
                            key: "email",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                    ],
                ),
                value: Value {
                    origin: None,
                    kind: String(
                        "base@user.email",
                    ),
                },
                source: EnvBase,
                is_overridden: true,
            },
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "user",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                        Key {
                            key: "name",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                    ],
                ),
                value: Value {
                    origin: None,
                    kind: String(
                        "base-user-name",
                    ),
                },
                source: EnvBase,
                is_overridden: false,
            },
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "user",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                        Key {
                            key: "email",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                    ],
                ),
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
        "#
        );
    }

    #[test]
    fn test_resolved_config_values_filter_path() {
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
        let mut config = StackedConfig::empty();
        config.add_layer(ConfigLayer::with_data(ConfigSource::User, user_config));
        config.add_layer(ConfigLayer::with_data(ConfigSource::Repo, repo_config));
        insta::assert_debug_snapshot!(
            resolved_config_values(&config, &ConfigNamePathBuf::from_iter(["test-table1"])),
            @r#"
        [
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "test-table1",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                        Key {
                            key: "foo",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                    ],
                ),
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
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "test-table1",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                        Key {
                            key: "bar",
                            repr: None,
                            leaf_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                            dotted_decor: Decor {
                                prefix: "default",
                                suffix: "default",
                            },
                        },
                    ],
                ),
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
        "#
        );
    }

    #[test]
    fn test_config_path_home_dir_existing() -> anyhow::Result<()> {
        TestCase {
            files: vec!["home/.jjconfig.toml"],
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: UnresolvedConfigEnv {
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
            env: Default::default(),
            want: Want::None,
        }
        .run()
    }

    #[test]
    fn test_config_path_ambiguous() -> anyhow::Result<()> {
        let tmp = setup_config_fs(&vec!["home/.jjconfig.toml", "config/jj/config.toml"])?;
        let env = UnresolvedConfigEnv {
            home_dir: Some(tmp.path().join("home")),
            config_dir: Some(tmp.path().join("config")),
            ..Default::default()
        };
        assert_matches!(env.resolve(), Err(ConfigEnvError::AmbiguousSource(_, _)));
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
        env: UnresolvedConfigEnv,
        want: Want,
    }

    impl TestCase {
        fn resolve(&self, root: &Path) -> Result<ConfigEnv, ConfigEnvError> {
            let env = UnresolvedConfigEnv {
                config_dir: self.env.config_dir.as_ref().map(|p| root.join(p)),
                home_dir: self.env.home_dir.as_ref().map(|p| root.join(p)),
                jj_config: self
                    .env
                    .jj_config
                    .as_ref()
                    .map(|p| root.join(p).to_str().unwrap().to_string()),
            };
            Ok(ConfigEnv {
                user_config_path: env.resolve()?,
                repo_config_path: ConfigPath::Unavailable,
            })
        }

        fn run(&self) -> anyhow::Result<()> {
            let tmp = setup_config_fs(&self.files)?;
            self.check_existing(&tmp)?;
            self.check_new(&tmp)?;
            Ok(())
        }

        fn check_existing(&self, tmp: &tempfile::TempDir) -> anyhow::Result<()> {
            let want = match self.want {
                Want::None => None,
                Want::New(_) => None,
                Want::ExistingAndNew(want) => Some(want),
            }
            .map(|p| tmp.path().join(p));
            let env = self
                .resolve(tmp.path())
                .map_err(|e| anyhow!("existing_config_path: {e}"))?;
            let got = env.existing_user_config_path();
            if got != want.as_deref() {
                return Err(anyhow!("existing_config_path: got {got:?}, want {want:?}"));
            }
            Ok(())
        }

        fn check_new(&self, tmp: &tempfile::TempDir) -> anyhow::Result<()> {
            let want = match self.want {
                Want::None => None,
                Want::New(want) => Some(want),
                Want::ExistingAndNew(want) => Some(want),
            }
            .map(|p| tmp.path().join(p));
            let env = self
                .resolve(tmp.path())
                .map_err(|e| anyhow!("new_config_path: {e}"))?;
            let got = env
                .new_user_config_path()
                .map_err(|e| anyhow!("new_config_path: {e}"))?;
            if got != want.as_deref() {
                return Err(anyhow!("new_config_path: got {got:?}, want {want:?}"));
            }
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
