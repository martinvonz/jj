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

//! Configuration store helpers.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::slice;
use std::str::FromStr;

use config::Source as _;
use itertools::Itertools as _;

use crate::file_util::IoResultExt as _;

/// Table of config key and value pairs.
pub type ConfigTable = config::Map<String, config::Value>;
/// Generic config value.
pub type ConfigValue = config::Value;

/// Error that can occur when accessing configuration.
// TODO: will be replaced with our custom error type
pub type ConfigError = config::ConfigError;

/// Dotted config name path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigNamePathBuf(Vec<toml_edit::Key>);

impl ConfigNamePathBuf {
    /// Creates an empty path pointing to the root table.
    ///
    /// This isn't a valid TOML key expression, but provided for convenience.
    pub fn root() -> Self {
        ConfigNamePathBuf(vec![])
    }

    /// Returns true if the path is empty (i.e. pointing to the root table.)
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns iterator of path components (or keys.)
    pub fn components(&self) -> slice::Iter<'_, toml_edit::Key> {
        self.0.iter()
    }

    /// Appends the given `key` component.
    pub fn push(&mut self, key: impl Into<toml_edit::Key>) {
        self.0.push(key.into());
    }

    /// Looks up value in the given `config`.
    ///
    /// This is a workaround for the `config.get()` API, which doesn't support
    /// literal path expression. If we implement our own config abstraction,
    /// this method should be moved there.
    pub fn lookup_value(&self, config: &config::Config) -> Result<ConfigValue, ConfigError> {
        // Use config.get() if the TOML keys can be converted to config path
        // syntax. This should be cheaper than cloning the whole config map.
        let (key_prefix, components) = self.split_safe_prefix();
        let value: ConfigValue = match &key_prefix {
            Some(key) => config.get(key)?,
            None => config.collect()?.into(),
        };
        components
            .iter()
            .try_fold(value, |value, key| {
                let mut table = value.into_table().ok()?;
                table.remove(key.get())
            })
            .ok_or_else(|| ConfigError::NotFound(self.to_string()))
    }

    /// Splits path to dotted literal expression and remainder.
    ///
    /// The literal expression part doesn't contain meta characters other than
    /// ".", therefore it can be passed in to `config.get()`.
    /// https://github.com/mehcode/config-rs/issues/110
    fn split_safe_prefix(&self) -> (Option<Cow<'_, str>>, &[toml_edit::Key]) {
        // https://github.com/mehcode/config-rs/blob/v0.13.4/src/path/parser.rs#L15
        let is_ident = |key: &str| {
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        let pos = self.0.iter().take_while(|&k| is_ident(k)).count();
        let safe_key = match pos {
            0 => None,
            1 => Some(Cow::Borrowed(self.0[0].get())),
            _ => Some(Cow::Owned(self.0[..pos].iter().join("."))),
        };
        (safe_key, &self.0[pos..])
    }
}

impl<K: Into<toml_edit::Key>> FromIterator<K> for ConfigNamePathBuf {
    fn from_iter<I: IntoIterator<Item = K>>(iter: I) -> Self {
        let keys = iter.into_iter().map(|k| k.into()).collect();
        ConfigNamePathBuf(keys)
    }
}

impl FromStr for ConfigNamePathBuf {
    type Err = toml_edit::TomlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // TOML parser ensures that the returned vec is not empty.
        toml_edit::Key::parse(s).map(ConfigNamePathBuf)
    }
}

impl AsRef<[toml_edit::Key]> for ConfigNamePathBuf {
    fn as_ref(&self) -> &[toml_edit::Key] {
        &self.0
    }
}

impl fmt::Display for ConfigNamePathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut components = self.0.iter().fuse();
        if let Some(key) = components.next() {
            write!(f, "{key}")?;
        }
        components.try_for_each(|key| write!(f, ".{key}"))
    }
}

/// Source of configuration variables in order of precedence.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ConfigSource {
    /// Default values (which has the lowest precedence.)
    Default,
    /// Base environment variables.
    EnvBase,
    /// User configuration files.
    User,
    /// Repo configuration files.
    Repo,
    /// Override environment variables.
    EnvOverrides,
    /// Command-line arguments (which has the highest precedence.)
    CommandArg,
}

/// Set of configuration variables with source information.
#[derive(Clone, Debug)]
pub struct ConfigLayer {
    /// Source type of this layer.
    pub source: ConfigSource,
    /// Source file path of this layer if any.
    pub path: Option<PathBuf>,
    /// Configuration variables.
    pub data: config::Config,
}

impl ConfigLayer {
    /// Creates new layer with the configuration variables `data`.
    pub fn with_data(source: ConfigSource, data: config::Config) -> Self {
        ConfigLayer {
            source,
            path: None,
            data,
        }
    }

    /// Parses TOML document `text` into new layer.
    pub fn parse(source: ConfigSource, text: &str) -> Result<Self, ConfigError> {
        let data = config::Config::builder()
            .add_source(config::File::from_str(text, config::FileFormat::Toml))
            .build()?;
        Ok(Self::with_data(source, data))
    }

    fn load_from_file(source: ConfigSource, path: PathBuf) -> Result<Self, ConfigError> {
        // TODO: will be replaced with toml_edit::DocumentMut or ImDocument
        let data = config::Config::builder()
            .add_source(
                config::File::from(path.clone())
                    // TODO: The path should exist, but the config crate refuses
                    // to read a special file (e.g. /dev/null) as TOML.
                    .required(false)
                    .format(config::FileFormat::Toml),
            )
            .build()?;
        Ok(ConfigLayer {
            source,
            path: Some(path),
            data,
        })
    }

    fn load_from_dir(source: ConfigSource, path: &Path) -> Result<Vec<Self>, ConfigError> {
        // TODO: Walk the directory recursively?
        let mut file_paths: Vec<_> = path
            .read_dir()
            .and_then(|dir_entries| {
                dir_entries
                    .map(|entry| Ok(entry?.path()))
                    // TODO: Accept only certain file extensions?
                    .filter_ok(|path| path.is_file())
                    .try_collect()
            })
            .context(path)
            .map_err(|err| ConfigError::Foreign(err.into()))?;
        file_paths.sort_unstable();
        file_paths
            .into_iter()
            .map(|path| Self::load_from_file(source, path))
            .try_collect()
    }
}

/// Stack of configuration layers which can be merged as needed.
#[derive(Clone, Debug)]
pub struct StackedConfig {
    /// Layers sorted by `source` (the lowest precedence one first.)
    layers: Vec<ConfigLayer>,
}

impl StackedConfig {
    /// Creates an empty stack of configuration layers.
    pub fn empty() -> Self {
        StackedConfig { layers: vec![] }
    }

    /// Loads config file from the specified `path`, inserts it at the position
    /// specified by `source`. The file should exist.
    pub fn load_file(
        &mut self,
        source: ConfigSource,
        path: impl Into<PathBuf>,
    ) -> Result<(), ConfigError> {
        let layer = ConfigLayer::load_from_file(source, path.into())?;
        self.add_layer(layer);
        Ok(())
    }

    /// Loads config files from the specified directory `path`, inserts them at
    /// the position specified by `source`. The directory should exist.
    pub fn load_dir(
        &mut self,
        source: ConfigSource,
        path: impl AsRef<Path>,
    ) -> Result<(), ConfigError> {
        let layers = ConfigLayer::load_from_dir(source, path.as_ref())?;
        let index = self.insert_point(source);
        self.layers.splice(index..index, layers);
        Ok(())
    }

    /// Inserts new layer at the position specified by `layer.source`.
    pub fn add_layer(&mut self, layer: ConfigLayer) {
        let index = self.insert_point(layer.source);
        self.layers.insert(index, layer);
    }

    /// Removes layers of the specified `source`.
    pub fn remove_layers(&mut self, source: ConfigSource) {
        self.layers.drain(self.layer_range(source));
    }

    fn layer_range(&self, source: ConfigSource) -> Range<usize> {
        // Linear search since the size of Vec wouldn't be large.
        let start = self
            .layers
            .iter()
            .take_while(|layer| layer.source < source)
            .count();
        let count = self.layers[start..]
            .iter()
            .take_while(|layer| layer.source == source)
            .count();
        start..(start + count)
    }

    fn insert_point(&self, source: ConfigSource) -> usize {
        // Search from end since layers are usually added in order, and the size
        // of Vec wouldn't be large enough to do binary search.
        let skip = self
            .layers
            .iter()
            .rev()
            .take_while(|layer| layer.source > source)
            .count();
        self.layers.len() - skip
    }

    /// Layers sorted by precedence.
    pub fn layers(&self) -> &[ConfigLayer] {
        &self.layers
    }

    /// Creates new merged config.
    pub fn merge(&self) -> config::Config {
        self.layers
            .iter()
            .fold(config::Config::builder(), |builder, layer| {
                builder.add_source(layer.data.clone())
            })
            .build()
            .expect("loaded configs should be merged without error")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_safe_config_name_path() {
        let parse = |s| ConfigNamePathBuf::from_str(s).unwrap();
        let key = |s: &str| toml_edit::Key::new(s);

        // Empty (or root) path isn't recognized by config::Config::get()
        assert_eq!(
            ConfigNamePathBuf::root().split_safe_prefix(),
            (None, [].as_slice())
        );

        assert_eq!(
            parse("Foo-bar_1").split_safe_prefix(),
            (Some("Foo-bar_1".into()), [].as_slice())
        );
        assert_eq!(
            parse("'foo()'").split_safe_prefix(),
            (None, [key("foo()")].as_slice())
        );
        assert_eq!(
            parse("foo.'bar()'").split_safe_prefix(),
            (Some("foo".into()), [key("bar()")].as_slice())
        );
        assert_eq!(
            parse("foo.'bar()'.baz").split_safe_prefix(),
            (Some("foo".into()), [key("bar()"), key("baz")].as_slice())
        );
        assert_eq!(
            parse("foo.bar").split_safe_prefix(),
            (Some("foo.bar".into()), [].as_slice())
        );
        assert_eq!(
            parse("foo.bar.'baz()'").split_safe_prefix(),
            (Some("foo.bar".into()), [key("baz()")].as_slice())
        );
    }

    #[test]
    fn test_stacked_config_layer_order() {
        let empty_data = || config::Config::builder().build().unwrap();
        let layer_sources = |config: &StackedConfig| {
            config
                .layers()
                .iter()
                .map(|layer| layer.source)
                .collect_vec()
        };

        // Insert in reverse order
        let mut config = StackedConfig::empty();
        config.add_layer(ConfigLayer::with_data(ConfigSource::Repo, empty_data()));
        config.add_layer(ConfigLayer::with_data(ConfigSource::User, empty_data()));
        config.add_layer(ConfigLayer::with_data(ConfigSource::Default, empty_data()));
        assert_eq!(
            layer_sources(&config),
            vec![
                ConfigSource::Default,
                ConfigSource::User,
                ConfigSource::Repo,
            ]
        );

        // Insert some more
        config.add_layer(ConfigLayer::with_data(
            ConfigSource::CommandArg,
            empty_data(),
        ));
        config.add_layer(ConfigLayer::with_data(ConfigSource::EnvBase, empty_data()));
        config.add_layer(ConfigLayer::with_data(ConfigSource::User, empty_data()));
        assert_eq!(
            layer_sources(&config),
            vec![
                ConfigSource::Default,
                ConfigSource::EnvBase,
                ConfigSource::User,
                ConfigSource::User,
                ConfigSource::Repo,
                ConfigSource::CommandArg,
            ]
        );

        // Remove last, first, middle
        config.remove_layers(ConfigSource::CommandArg);
        config.remove_layers(ConfigSource::Default);
        config.remove_layers(ConfigSource::User);
        assert_eq!(
            layer_sources(&config),
            vec![ConfigSource::EnvBase, ConfigSource::Repo]
        );

        // Remove unknown
        config.remove_layers(ConfigSource::Default);
        config.remove_layers(ConfigSource::EnvOverrides);
        assert_eq!(
            layer_sources(&config),
            vec![ConfigSource::EnvBase, ConfigSource::Repo]
        );

        // Remove remainders
        config.remove_layers(ConfigSource::EnvBase);
        config.remove_layers(ConfigSource::Repo);
        assert_eq!(layer_sources(&config), vec![]);
    }
}
