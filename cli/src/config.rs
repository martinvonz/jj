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
use jj_lib::config::ConfigLayer;
use jj_lib::config::ConfigLoadError;
use jj_lib::config::ConfigNamePathBuf;
use jj_lib::config::ConfigSource;
use jj_lib::config::ConfigValue;
use jj_lib::config::StackedConfig;
use regex::Captures;
use regex::Regex;
use thiserror::Error;
use tracing::instrument;

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

#[derive(Error, Debug)]
pub enum ConfigEnvError {
    #[error("Both {0} and {1} exist. Please consolidate your configs in one of them.")]
    AmbiguousSource(PathBuf, PathBuf),
    #[error(transparent)]
    CreateFile(std::io::Error),
}

/// Configuration variable with its source information.
#[derive(Clone, Debug)]
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
            if let Some(table) = item.as_table() {
                // table.iter() does not implement DoubleEndedIterator as of
                // toml_edit 0.22.22.
                let frame = config_stack.len();
                for (k, v) in table {
                    let mut sub_name = name.clone();
                    sub_name.push(k);
                    config_stack.push((sub_name, v));
                }
                config_stack[frame..].reverse();
            } else {
                let value = item
                    .clone()
                    .into_value()
                    .expect("Item::None should not exist in table");
                config_vals.push(AnnotatedValue {
                    name,
                    value,
                    source: layer.source,
                    // Note: Value updated below.
                    is_overridden: false,
                });
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

    fn as_path(&self) -> Option<&Path> {
        match self {
            ConfigPath::Existing(path) | ConfigPath::New(path) => Some(path),
            ConfigPath::Unavailable => None,
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

    /// Returns a path to the user-specific config file or directory.
    pub fn user_config_path(&self) -> Option<&Path> {
        self.user_config_path.as_path()
    }

    /// Returns a path to the existing user-specific config file or directory.
    fn existing_user_config_path(&self) -> Option<&Path> {
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
                // callers need an empty file. "jj config set" doesn't have
                // to create an empty file to be overwritten. Since it's unclear
                // who and when to update ConfigPath::New(_) to ::Existing(_),
                // it's probably better to not cache the path existence.
                create_config_file(path).map_err(ConfigEnvError::CreateFile)?;
                Ok(Some(path))
            }
            ConfigPath::Unavailable => Ok(None),
        }
    }

    /// Loads user-specific config files into the given `config`. The old
    /// user-config layers will be replaced if any.
    #[instrument]
    pub fn reload_user_config(&self, config: &mut StackedConfig) -> Result<(), ConfigLoadError> {
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

    /// Returns a path to the repo-specific config file.
    pub fn repo_config_path(&self) -> Option<&Path> {
        self.repo_config_path.as_path()
    }

    /// Returns a path to the existing repo-specific config file.
    fn existing_repo_config_path(&self) -> Option<&Path> {
        match &self.repo_config_path {
            ConfigPath::Existing(path) => Some(path),
            _ => None,
        }
    }

    /// Loads repo-specific config file into the given `config`. The old
    /// repo-config layer will be replaced if any.
    #[instrument]
    pub fn reload_repo_config(&self, config: &mut StackedConfig) -> Result<(), ConfigLoadError> {
        config.remove_layers(ConfigSource::Repo);
        if let Some(path) = self.existing_repo_config_path() {
            config.load_file(ConfigSource::Repo, path)?;
        }
        Ok(())
    }
}

/// Initializes stacked config with the given `default_layers` and infallible
/// sources.
///
/// Sources from the lowest precedence:
/// 1. Default
/// 2. Base environment variables
/// 3. [User config](https://martinvonz.github.io/jj/latest/config/)
/// 4. Repo config `.jj/repo/config.toml`
/// 5. TODO: Workspace config `.jj/config.toml`
/// 6. Override environment variables
/// 7. Command-line arguments `--config-toml`, `--config-file`
///
/// This function sets up 1, 2, and 6.
pub fn config_from_environment(
    default_layers: impl IntoIterator<Item = ConfigLayer>,
) -> StackedConfig {
    let mut config = StackedConfig::empty();
    config.extend_layers(default_layers);
    config.add_layer(env_base_layer());
    config.add_layer(env_overrides_layer());
    config
}

/// Environment variables that should be overridden by config values
fn env_base_layer() -> ConfigLayer {
    let mut layer = ConfigLayer::empty(ConfigSource::EnvBase);
    if !env::var("NO_COLOR").unwrap_or_default().is_empty() {
        // "User-level configuration files and per-instance command-line arguments
        // should override $NO_COLOR." https://no-color.org/
        layer.set_value("ui.color", "never").unwrap();
    }
    if let Ok(value) = env::var("PAGER") {
        layer.set_value("ui.pager", value).unwrap();
    }
    if let Ok(value) = env::var("VISUAL") {
        layer.set_value("ui.editor", value).unwrap();
    } else if let Ok(value) = env::var("EDITOR") {
        layer.set_value("ui.editor", value).unwrap();
    }
    layer
}

pub fn default_config_layers() -> Vec<ConfigLayer> {
    // Syntax error in default config isn't a user error. That's why defaults are
    // loaded by separate builder.
    let parse = |text: &'static str| ConfigLayer::parse(ConfigSource::Default, text).unwrap();
    let mut layers = vec![
        parse(include_str!("config/colors.toml")),
        parse(include_str!("config/merge_tools.toml")),
        parse(include_str!("config/misc.toml")),
        parse(include_str!("config/revsets.toml")),
        parse(include_str!("config/templates.toml")),
    ];
    if cfg!(unix) {
        layers.push(parse(include_str!("config/unix.toml")));
    }
    if cfg!(windows) {
        layers.push(parse(include_str!("config/windows.toml")));
    }
    layers
}

/// Environment variables that override config values
fn env_overrides_layer() -> ConfigLayer {
    let mut layer = ConfigLayer::empty(ConfigSource::EnvOverrides);
    if let Ok(value) = env::var("JJ_USER") {
        layer.set_value("user.name", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_EMAIL") {
        layer.set_value("user.email", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_TIMESTAMP") {
        layer.set_value("debug.commit-timestamp", value).unwrap();
    }
    if let Ok(Ok(value)) = env::var("JJ_RANDOMNESS_SEED").map(|s| s.parse::<i64>()) {
        layer.set_value("debug.randomness-seed", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_OP_TIMESTAMP") {
        layer.set_value("debug.operation-timestamp", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_OP_HOSTNAME") {
        layer.set_value("operation.hostname", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_OP_USERNAME") {
        layer.set_value("operation.username", value).unwrap();
    }
    if let Ok(value) = env::var("JJ_EDITOR") {
        layer.set_value("ui.editor", value).unwrap();
    }
    layer
}

/// Configuration source/data provided as command-line argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigArg {
    /// `--config-toml=TOML`
    Toml(String),
    /// `--config-file=PATH`
    File(String),
}

/// Parses `--config-toml` arguments.
pub fn parse_config_args(toml_strs: &[ConfigArg]) -> Result<Vec<ConfigLayer>, ConfigLoadError> {
    // It might look silly that a layer is constructed per argument, but
    // --config-toml argument can contain a full TOML document, and it makes
    // sense to preserve line numbers within the doc. If we add
    // --config=KEY=VALUE, multiple values might be loaded into one layer.
    let source = ConfigSource::CommandArg;
    toml_strs
        .iter()
        .map(|arg| match arg {
            ConfigArg::Toml(text) => ConfigLayer::parse(source, text),
            ConfigArg::File(path) => ConfigLayer::load_from_file(source, path.into()),
        })
        .try_collect()
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

    fn insta_settings() -> insta::Settings {
        let mut settings = insta::Settings::clone_current();
        // Suppress Decor { .. } which is uninteresting
        settings.add_filter(r"\bDecor \{[^}]*\}", "Decor { .. }");
        settings
    }

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
        assert!(resolved_config_values(&config, &ConfigNamePathBuf::root()).is_empty());
    }

    #[test]
    fn test_resolved_config_values_single_key() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let mut env_base_layer = ConfigLayer::empty(ConfigSource::EnvBase);
        env_base_layer
            .set_value("user.name", "base-user-name")
            .unwrap();
        env_base_layer
            .set_value("user.email", "base@user.email")
            .unwrap();
        let mut repo_layer = ConfigLayer::empty(ConfigSource::Repo);
        repo_layer
            .set_value("user.email", "repo@user.email")
            .unwrap();
        let mut config = StackedConfig::empty();
        config.add_layer(env_base_layer);
        config.add_layer(repo_layer);
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
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                        Key {
                            key: "name",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                    ],
                ),
                value: String(
                    Formatted {
                        value: "base-user-name",
                        repr: "default",
                        decor: Decor { .. },
                    },
                ),
                source: EnvBase,
                is_overridden: false,
            },
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "user",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                        Key {
                            key: "email",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                    ],
                ),
                value: String(
                    Formatted {
                        value: "base@user.email",
                        repr: "default",
                        decor: Decor { .. },
                    },
                ),
                source: EnvBase,
                is_overridden: true,
            },
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "user",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                        Key {
                            key: "email",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                    ],
                ),
                value: String(
                    Formatted {
                        value: "repo@user.email",
                        repr: "default",
                        decor: Decor { .. },
                    },
                ),
                source: Repo,
                is_overridden: false,
            },
        ]
        "#
        );
    }

    #[test]
    fn test_resolved_config_values_filter_path() {
        let settings = insta_settings();
        let _guard = settings.bind_to_scope();
        let mut user_layer = ConfigLayer::empty(ConfigSource::User);
        user_layer.set_value("test-table1.foo", "user-FOO").unwrap();
        user_layer.set_value("test-table2.bar", "user-BAR").unwrap();
        let mut repo_layer = ConfigLayer::empty(ConfigSource::Repo);
        repo_layer.set_value("test-table1.bar", "repo-BAR").unwrap();
        let mut config = StackedConfig::empty();
        config.add_layer(user_layer);
        config.add_layer(repo_layer);
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
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                        Key {
                            key: "foo",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                    ],
                ),
                value: String(
                    Formatted {
                        value: "user-FOO",
                        repr: "default",
                        decor: Decor { .. },
                    },
                ),
                source: User,
                is_overridden: false,
            },
            AnnotatedValue {
                name: ConfigNamePathBuf(
                    [
                        Key {
                            key: "test-table1",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                        Key {
                            key: "bar",
                            repr: None,
                            leaf_decor: Decor { .. },
                            dotted_decor: Decor { .. },
                        },
                    ],
                ),
                value: String(
                    Formatted {
                        value: "repo-BAR",
                        repr: "default",
                        decor: Decor { .. },
                    },
                ),
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
