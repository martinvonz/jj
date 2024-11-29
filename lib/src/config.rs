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

use std::borrow::Borrow;
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::slice;
use std::str::FromStr;

use itertools::Itertools as _;
use serde::Deserialize;

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

/// Value that can be converted to a dotted config name path.
///
/// This is an abstraction to specify a config name path in either a string or a
/// parsed form. It's similar to `Into<T>`, but the output type `T` is
/// constrained by the source type.
pub trait ToConfigNamePath: Sized {
    /// Path type to be converted from `Self`.
    type Output: Borrow<ConfigNamePathBuf>;

    /// Converts this object into a dotted config name path.
    fn into_name_path(self) -> Self::Output;
}

impl ToConfigNamePath for ConfigNamePathBuf {
    type Output = Self;

    fn into_name_path(self) -> Self::Output {
        self
    }
}

impl ToConfigNamePath for &ConfigNamePathBuf {
    type Output = Self;

    fn into_name_path(self) -> Self::Output {
        self
    }
}

impl ToConfigNamePath for &'static str {
    // This can be changed to ConfigNamePathStr(str) if allocation cost matters.
    type Output = ConfigNamePathBuf;

    /// Parses this string into a dotted config name path.
    ///
    /// The string must be a valid TOML dotted key. A static str is required to
    /// prevent API misuse.
    fn into_name_path(self) -> Self::Output {
        self.parse()
            .expect("valid TOML dotted key must be provided")
    }
}

impl<const N: usize> ToConfigNamePath for [&str; N] {
    type Output = ConfigNamePathBuf;

    fn into_name_path(self) -> Self::Output {
        self.into_iter().collect()
    }
}

impl<const N: usize> ToConfigNamePath for &[&str; N] {
    type Output = ConfigNamePathBuf;

    fn into_name_path(self) -> Self::Output {
        self.as_slice().into_name_path()
    }
}

impl ToConfigNamePath for &[&str] {
    type Output = ConfigNamePathBuf;

    fn into_name_path(self) -> Self::Output {
        self.iter().copied().collect()
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

    // Add .get_value(name) if needed. look_up_*() are low-level API.

    /// Looks up item by the `name` path. Returns `Some(item)` if an item
    /// found at the path. Returns `Err(item)` if middle node wasn't a table.
    pub fn look_up_item(
        &self,
        name: impl ToConfigNamePath,
    ) -> Result<Option<&ConfigValue>, &ConfigValue> {
        look_up_item(&self.data.cache, name.into_name_path().borrow())
    }
}

/// Looks up item from the `root_item`. Returns `Some(item)` if an item found at
/// the path. Returns `Err(item)` if middle node wasn't a table.
fn look_up_item<'a>(
    root_item: &'a ConfigValue,
    name: &ConfigNamePathBuf,
) -> Result<Option<&'a ConfigValue>, &'a ConfigValue> {
    let mut cur_item = root_item;
    for key in name.components().map(toml_edit::Key::get) {
        let config::ValueKind::Table(table) = &cur_item.kind else {
            return Err(cur_item);
        };
        cur_item = match table.get(key) {
            Some(item) => item,
            None => return Ok(None),
        };
    }
    Ok(Some(cur_item))
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

    /// Looks up value of the specified type `T` from all layers, merges sub
    /// fields as needed.
    pub fn get<'de, T: Deserialize<'de>>(
        &self,
        name: impl ToConfigNamePath,
    ) -> Result<T, ConfigError> {
        self.get_item_with(name, T::deserialize)
    }

    /// Looks up value from all layers, merges sub fields as needed.
    pub fn get_value(&self, name: impl ToConfigNamePath) -> Result<ConfigValue, ConfigError> {
        self.get_item_with(name, Ok)
    }

    /// Looks up sub table from all layers, merges fields as needed.
    // TODO: redesign this to attach better error indication?
    pub fn get_table(&self, name: impl ToConfigNamePath) -> Result<ConfigTable, ConfigError> {
        self.get(name)
    }

    fn get_item_with<T>(
        &self,
        name: impl ToConfigNamePath,
        convert: impl FnOnce(ConfigValue) -> Result<T, ConfigError>,
    ) -> Result<T, ConfigError> {
        let name = name.into_name_path();
        let name = name.borrow();
        let (item, _layer_index) = get_merged_item(&self.layers, name)
            .ok_or_else(|| ConfigError::NotFound(name.to_string()))?;
        // TODO: Add source type/path to the error message. If the value is
        // a table, the error might come from lower layers. We cannot report
        // precise source information in that case. However, toml_edit captures
        // dotted keys in the error object. If the keys field were public, we
        // can look up the source information. This is probably simpler than
        // reimplementing Deserializer.
        convert(item).map_err(|err| err.extend_with_key(&name.to_string()))
    }
}

/// Looks up item from `layers`, merges sub fields as needed. Returns a merged
/// item and the uppermost layer index where the item was found.
fn get_merged_item(
    layers: &[ConfigLayer],
    name: &ConfigNamePathBuf,
) -> Option<(ConfigValue, usize)> {
    let mut to_merge = Vec::new();
    for (index, layer) in layers.iter().enumerate().rev() {
        let item = match layer.look_up_item(name) {
            Ok(Some(item)) => item,
            Ok(None) => continue, // parent is a table, but no value found
            Err(_) => break,      // parent is not a table, shadows lower layers
        };
        if matches!(item.kind, config::ValueKind::Table(_)) {
            to_merge.push((item, index));
        } else if to_merge.is_empty() {
            return Some((item.clone(), index)); // no need to allocate vec
        } else {
            break; // shadows lower layers
        }
    }

    // Simply merge tables from the bottom layer. Upper items should override
    // the lower items (including their children) no matter if the upper items
    // are shadowed by the other upper items.
    let (item, mut top_index) = to_merge.pop()?;
    let mut merged = item.clone();
    for (item, index) in to_merge.into_iter().rev() {
        merge_items(&mut merged, item);
        top_index = index;
    }
    Some((merged, top_index))
}

/// Merges `upper_item` fields into `lower_item` recursively.
fn merge_items(lower_item: &mut ConfigValue, upper_item: &ConfigValue) {
    // TODO: If we switch to toml_edit, inline table will probably be treated as
    // a value, not a table to be merged. For example, { fg = "red" } won't
    // inherit other color parameters from the lower table.
    let (config::ValueKind::Table(lower_table), config::ValueKind::Table(upper_table)) =
        (&mut lower_item.kind, &upper_item.kind)
    else {
        // Not a table, the upper item wins.
        *lower_item = upper_item.clone();
        return;
    };
    for (key, upper) in upper_table {
        lower_table
            .entry(key.clone())
            .and_modify(|lower| merge_items(lower, upper))
            .or_insert_with(|| upper.clone());
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    use super::*;

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

    fn new_user_layer(text: &str) -> ConfigLayer {
        ConfigLayer::parse(ConfigSource::User, text).unwrap()
    }

    fn parse_to_table(text: &str) -> ConfigTable {
        new_user_layer(text).data.cache.into_table().unwrap()
    }

    #[test]
    fn test_stacked_config_get_simple_value() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.b.c = 'a.b.c #0'
        "}));
        config.add_layer(new_user_layer(indoc! {"
            a.d = ['a.d #1']
        "}));

        assert_eq!(config.get::<String>("a.b.c").unwrap(), "a.b.c #0");

        assert_eq!(
            config.get::<Vec<String>>("a.d").unwrap(),
            vec!["a.d #1".to_owned()]
        );

        // Table "a.b" exists, but key doesn't
        assert_matches!(
            config.get::<String>("a.b.missing"),
            Err(ConfigError::NotFound(name)) if name == "a.b.missing"
        );

        // Node "a.b.c" is not a table
        assert_matches!(
            config.get::<String>("a.b.c.d"),
            Err(ConfigError::NotFound(name)) if name == "a.b.c.d"
        );

        // Type error
        assert_matches!(
            config.get::<String>("a.b"),
            Err(ConfigError::Type { key: Some(name), .. }) if name == "a.b"
        );
    }

    #[test]
    fn test_stacked_config_get_value_shadowing_table() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.b.c = 'a.b.c #0'
        "}));
        // a.b.c is shadowed by a.b
        config.add_layer(new_user_layer(indoc! {"
            a.b = 'a.b #1'
        "}));

        assert_eq!(config.get::<String>("a.b").unwrap(), "a.b #1");

        assert_matches!(
            config.get::<String>("a.b.c"),
            Err(ConfigError::NotFound(name)) if name == "a.b.c"
        );
    }

    #[test]
    fn test_stacked_config_get_table_shadowing_table() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.b = 'a.b #0'
        "}));
        // a.b is shadowed by a.b.c
        config.add_layer(new_user_layer(indoc! {"
            a.b.c = 'a.b.c #1'
        "}));

        let expected = parse_to_table(indoc! {"
            c = 'a.b.c #1'
        "});
        assert_eq!(config.get_table("a.b").unwrap(), expected);
    }

    #[test]
    fn test_stacked_config_get_merged_table() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.a.a = 'a.a.a #0'
            a.a.b = 'a.a.b #0'
            a.b = 'a.b #0'
        "}));
        config.add_layer(new_user_layer(indoc! {"
            a.a.b = 'a.a.b #1'
            a.a.c = 'a.a.c #1'
            a.c = 'a.c #1'
        "}));
        let expected = parse_to_table(indoc! {"
            a.a = 'a.a.a #0'
            a.b = 'a.a.b #1'
            a.c = 'a.a.c #1'
            b = 'a.b #0'
            c = 'a.c #1'
        "});
        assert_eq!(config.get_table("a").unwrap(), expected);
    }

    #[test]
    fn test_stacked_config_get_merged_table_shadowed_top() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.a.a = 'a.a.a #0'
            a.b = 'a.b #0'
        "}));
        // a.a.a and a.b are shadowed by a
        config.add_layer(new_user_layer(indoc! {"
            a = 'a #1'
        "}));
        // a is shadowed by a.a.b
        config.add_layer(new_user_layer(indoc! {"
            a.a.b = 'a.a.b #2'
        "}));
        let expected = parse_to_table(indoc! {"
            a.b = 'a.a.b #2'
        "});
        assert_eq!(config.get_table("a").unwrap(), expected);
    }

    #[test]
    fn test_stacked_config_get_merged_table_shadowed_child() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.a.a = 'a.a.a #0'
            a.b = 'a.b #0'
        "}));
        // a.a.a is shadowed by a.a
        config.add_layer(new_user_layer(indoc! {"
            a.a = 'a.a #1'
        "}));
        // a.a is shadowed by a.a.b
        config.add_layer(new_user_layer(indoc! {"
            a.a.b = 'a.a.b #2'
        "}));
        let expected = parse_to_table(indoc! {"
            a.b = 'a.a.b #2'
            b = 'a.b #0'
        "});
        assert_eq!(config.get_table("a").unwrap(), expected);
    }

    #[test]
    fn test_stacked_config_get_merged_table_shadowed_parent() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.a.a = 'a.a.a #0'
        "}));
        // a.a.a is shadowed by a
        config.add_layer(new_user_layer(indoc! {"
            a = 'a #1'
        "}));
        // a is shadowed by a.a.b
        config.add_layer(new_user_layer(indoc! {"
            a.a.b = 'a.a.b #2'
        "}));
        let expected = parse_to_table(indoc! {"
            b = 'a.a.b #2'
        "});
        // a is not under a.a, but it should still shadow lower layers
        assert_eq!(config.get_table("a.a").unwrap(), expected);
    }
}
