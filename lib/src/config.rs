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
use std::convert::Infallible;
use std::fmt;
use std::fs;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::slice;
use std::str::FromStr;

use itertools::Itertools as _;
use serde::de::IntoDeserializer as _;
use serde::Deserialize;
use thiserror::Error;
use toml_edit::DocumentMut;
use toml_edit::ImDocument;

use crate::file_util::IoResultExt as _;
use crate::file_util::PathError;

/// Config value or table node.
pub type ConfigItem = toml_edit::Item;
/// Table of config key and value pairs.
pub type ConfigTable = toml_edit::Table;
/// Generic config value.
pub type ConfigValue = toml_edit::Value;

/// Error that can occur when parsing or loading config variables.
#[derive(Debug, Error)]
pub enum ConfigLoadError {
    /// Config file or directory cannot be read.
    #[error("Failed to read configuration file")]
    Read(#[source] PathError),
    /// TOML file or text cannot be parsed.
    #[error("Configuration cannot be parsed as TOML document")]
    Parse {
        /// Source error.
        #[source]
        error: toml_edit::TomlError,
        /// Source file path.
        source_path: Option<PathBuf>,
    },
}

/// Error that can occur when looking up config variable.
#[derive(Debug, Error)]
pub enum ConfigGetError {
    /// Config value is not set.
    #[error("Value not found for {name}")]
    NotFound {
        /// Dotted config name path.
        name: String,
    },
    /// Config value cannot be converted to the expected type.
    #[error("Invalid type or value for {name}")]
    Type {
        /// Dotted config name path.
        name: String,
        /// Source error.
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
        /// Source file path where the value is defined.
        source_path: Option<PathBuf>,
    },
}

/// Error that can occur when updating config variable.
#[derive(Debug, Error)]
pub enum ConfigUpdateError {
    /// Non-table value exists at parent path, which shouldn't be removed.
    #[error("Would overwrite non-table value with parent table {name}")]
    WouldOverwriteValue {
        /// Dotted config name path.
        name: String,
    },
    /// Table exists at the path, which shouldn't be overwritten by a value.
    #[error("Would overwrite entire table {name}")]
    WouldOverwriteTable {
        /// Dotted config name path.
        name: String,
    },
}

/// Extension methods for `Result<T, ConfigGetError>`.
pub trait ConfigGetResultExt<T> {
    /// Converts `NotFound` error to `Ok(None)`, leaving other errors.
    fn optional(self) -> Result<Option<T>, ConfigGetError>;
}

impl<T> ConfigGetResultExt<T> for Result<T, ConfigGetError> {
    fn optional(self) -> Result<Option<T>, ConfigGetError> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(ConfigGetError::NotFound { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

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
    pub data: DocumentMut,
}

impl ConfigLayer {
    /// Creates new layer with empty data.
    pub fn empty(source: ConfigSource) -> Self {
        Self::with_data(source, DocumentMut::new())
    }

    /// Creates new layer with the configuration variables `data`.
    pub fn with_data(source: ConfigSource, data: DocumentMut) -> Self {
        ConfigLayer {
            source,
            path: None,
            data,
        }
    }

    /// Parses TOML document `text` into new layer.
    pub fn parse(source: ConfigSource, text: &str) -> Result<Self, ConfigLoadError> {
        let data = ImDocument::parse(text).map_err(|error| ConfigLoadError::Parse {
            error,
            source_path: None,
        })?;
        Ok(Self::with_data(source, data.into_mut()))
    }

    fn load_from_file(source: ConfigSource, path: PathBuf) -> Result<Self, ConfigLoadError> {
        let text = fs::read_to_string(&path)
            .context(&path)
            .map_err(ConfigLoadError::Read)?;
        let data = ImDocument::parse(text).map_err(|error| ConfigLoadError::Parse {
            error,
            source_path: Some(path.clone()),
        })?;
        Ok(ConfigLayer {
            source,
            path: Some(path),
            data: data.into_mut(),
        })
    }

    fn load_from_dir(source: ConfigSource, path: &Path) -> Result<Vec<Self>, ConfigLoadError> {
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
            .map_err(ConfigLoadError::Read)?;
        file_paths.sort_unstable();
        file_paths
            .into_iter()
            .map(|path| Self::load_from_file(source, path))
            .try_collect()
    }

    // Add .get_value(name) if needed. look_up_*() are low-level API.

    /// Looks up sub non-inline table by the `name` path. Returns `Some(table)`
    /// if a table was found at the path. Returns `Err(item)` if middle or leaf
    /// node wasn't a table.
    pub fn look_up_table(
        &self,
        name: impl ToConfigNamePath,
    ) -> Result<Option<&ConfigTable>, &ConfigItem> {
        match self.look_up_item(name) {
            Ok(Some(item)) => match item.as_table() {
                Some(table) => Ok(Some(table)),
                None => Err(item),
            },
            Ok(None) => Ok(None),
            Err(item) => Err(item),
        }
    }

    /// Looks up item by the `name` path. Returns `Some(item)` if an item
    /// found at the path. Returns `Err(item)` if middle node wasn't a table.
    pub fn look_up_item(
        &self,
        name: impl ToConfigNamePath,
    ) -> Result<Option<&ConfigItem>, &ConfigItem> {
        look_up_item(self.data.as_item(), name.into_name_path().borrow())
    }

    /// Sets `new_value` to the `name` path. Returns old value if any.
    ///
    /// This function errors out if attempted to overwrite a non-table middle
    /// node or a leaf table (in the same way as file/directory operation.)
    pub fn set_value(
        &mut self,
        name: impl ToConfigNamePath,
        new_value: impl Into<ConfigValue>,
    ) -> Result<Option<ConfigValue>, ConfigUpdateError> {
        let name = name.into_name_path();
        let name = name.borrow();
        let (parent_table, leaf_key) = ensure_parent_table(self.data.as_table_mut(), name)
            .map_err(|keys| ConfigUpdateError::WouldOverwriteValue {
                name: keys.join("."),
            })?;
        match parent_table.entry(leaf_key) {
            toml_edit::Entry::Occupied(mut entry) => {
                if !entry.get().is_value() {
                    return Err(ConfigUpdateError::WouldOverwriteTable {
                        name: name.to_string(),
                    });
                }
                let old_item = entry.insert(toml_edit::value(new_value));
                Ok(Some(old_item.into_value().unwrap()))
            }
            toml_edit::Entry::Vacant(entry) => {
                entry.insert(toml_edit::value(new_value));
                Ok(None)
            }
        }
    }
}

/// Looks up item from the `root_item`. Returns `Some(item)` if an item found at
/// the path. Returns `Err(item)` if middle node wasn't a non-inline table.
fn look_up_item<'a>(
    root_item: &'a ConfigItem,
    name: &ConfigNamePathBuf,
) -> Result<Option<&'a ConfigItem>, &'a ConfigItem> {
    let mut cur_item = root_item;
    for key in name.components() {
        let Some(table) = cur_item.as_table() else {
            return Err(cur_item);
        };
        cur_item = match table.get(key) {
            Some(item) => item,
            None => return Ok(None),
        };
    }
    Ok(Some(cur_item))
}

/// Inserts tables down to the parent of the `name` path. Returns `Err(keys)` if
/// middle node exists at the prefix name `keys` and wasn't a table.
fn ensure_parent_table<'a, 'b>(
    root_table: &'a mut ConfigTable,
    name: &'b ConfigNamePathBuf,
) -> Result<(&'a mut ConfigTable, &'b toml_edit::Key), &'b [toml_edit::Key]> {
    let mut keys = name.components();
    let leaf_key = keys.next_back().ok_or(&name.0[..])?;
    let parent_table = keys.enumerate().try_fold(root_table, |table, (i, key)| {
        let sub_item = table.entry(key).or_insert_with(toml_edit::table);
        sub_item.as_table_mut().ok_or(&name.0[..=i])
    })?;
    Ok((parent_table, leaf_key))
}

/// Stack of configuration layers which can be merged as needed.
///
/// A [`StackedConfig`] is something like a read-only `overlayfs`. Tables and
/// values are directories and files respectively, and tables are merged across
/// layers. Tables and values can be addressed by [dotted name
/// paths](ToConfigNamePath).
///
/// There's no tombstone notation to remove items from the lower layers.
///
/// # Inline and non-inline tables
///
/// An inline table is considered a value (or a file in file-system analogy.)
/// It would probably make sense because the syntax looks like an assignment
/// `key = { .. }`, and "no newlines are allowed between the curly braces." It's
/// unlikely that user defined a large inline table like `ui = { .. }`.
///
/// - Inline tables will never be merged across layers, and the uppermost table
///   is always taken.
/// - Inner values of an inline table cannot be addressed by a dotted name path.
///   (e.g. `foo.bar` is not a valid path to `foo = { bar = x }`.)
/// - A lower inline table is shadowed by an upper non-inline table, just like a
///   file is shadowed by a directory of the same name. (e.g. `foo = { bar = x
///   }` is not merged, but shadowed by `foo.baz = y`.)
/// - A non-inline table can be converted to an inline table (or a value) on
///   `.get()`, but not the other way around. This specifically allows parsing
///   of a structured value from a merged table.
///
/// # Array of tables
///
/// If we employ the "array of tables" notation, array items will be gathered
/// from all layers, as if the array were a directory, and each item had a
/// unique file name. This merging strategy is not implemented yet.
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
    ) -> Result<(), ConfigLoadError> {
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
    ) -> Result<(), ConfigLoadError> {
        let layers = ConfigLayer::load_from_dir(source, path.as_ref())?;
        self.extend_layers(layers);
        Ok(())
    }

    /// Inserts new layer at the position specified by `layer.source`.
    pub fn add_layer(&mut self, layer: ConfigLayer) {
        let index = self.insert_point(layer.source);
        self.layers.insert(index, layer);
    }

    /// Inserts multiple layers at the positions specified by `layer.source`.
    pub fn extend_layers(&mut self, layers: impl IntoIterator<Item = ConfigLayer>) {
        for (source, chunk) in &layers.into_iter().chunk_by(|layer| layer.source) {
            let index = self.insert_point(source);
            self.layers.splice(index..index, chunk);
        }
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
    ) -> Result<T, ConfigGetError> {
        self.get_value_with(name, |value| T::deserialize(value.into_deserializer()))
    }

    /// Looks up value from all layers, merges sub fields as needed.
    pub fn get_value(&self, name: impl ToConfigNamePath) -> Result<ConfigValue, ConfigGetError> {
        self.get_value_with::<_, Infallible>(name, Ok)
    }

    /// Looks up value from all layers, merges sub fields as needed, then
    /// converts the value by using the given function.
    pub fn get_value_with<T, E: Into<Box<dyn std::error::Error + Send + Sync>>>(
        &self,
        name: impl ToConfigNamePath,
        convert: impl FnOnce(ConfigValue) -> Result<T, E>,
    ) -> Result<T, ConfigGetError> {
        self.get_item_with(name, |item| {
            // Item variants other than Item::None can be converted to a Value,
            // and Item::None is not a valid TOML type. See also the following
            // thread: https://github.com/toml-rs/toml/issues/299
            let value = item
                .into_value()
                .expect("Item::None should not exist in loaded tables");
            convert(value)
        })
    }

    /// Looks up sub non-inline table from all layers, merges fields as needed.
    ///
    /// Use `table_keys(prefix)` and `get([prefix, key])` instead if table
    /// values have to be converted to non-generic value type.
    pub fn get_table(&self, name: impl ToConfigNamePath) -> Result<ConfigTable, ConfigGetError> {
        // Not using .into_table() because inline table shouldn't be converted.
        self.get_item_with(name, |item| match item {
            ConfigItem::Table(table) => Ok(table),
            _ => Err(format!("Expected a table, but is {}", item.type_name())),
        })
    }

    fn get_item_with<T, E: Into<Box<dyn std::error::Error + Send + Sync>>>(
        &self,
        name: impl ToConfigNamePath,
        convert: impl FnOnce(ConfigItem) -> Result<T, E>,
    ) -> Result<T, ConfigGetError> {
        let name = name.into_name_path();
        let name = name.borrow();
        let (item, layer_index) =
            get_merged_item(&self.layers, name).ok_or_else(|| ConfigGetError::NotFound {
                name: name.to_string(),
            })?;
        // If the value is a table, the error might come from lower layers. We
        // cannot report precise source information in that case. However,
        // toml_edit captures dotted keys in the error object. If the keys field
        // were public, we can look up the source information. This is probably
        // simpler than reimplementing Deserializer.
        convert(item).map_err(|err| ConfigGetError::Type {
            name: name.to_string(),
            error: err.into(),
            source_path: self.layers[layer_index].path.clone(),
        })
    }

    /// Returns iterator over sub non-inline table keys in order of layer
    /// precedence. Duplicated keys are omitted.
    pub fn table_keys(&self, name: impl ToConfigNamePath) -> impl Iterator<Item = &str> {
        let name = name.into_name_path();
        let name = name.borrow();
        let to_merge = get_tables_to_merge(&self.layers, name);
        to_merge
            .into_iter()
            .rev()
            .flat_map(|table| table.iter().map(|(k, _)| k))
            .unique()
    }
}

/// Looks up item from `layers`, merges sub fields as needed. Returns a merged
/// item and the uppermost layer index where the item was found.
fn get_merged_item(
    layers: &[ConfigLayer],
    name: &ConfigNamePathBuf,
) -> Option<(ConfigItem, usize)> {
    let mut to_merge = Vec::new();
    for (index, layer) in layers.iter().enumerate().rev() {
        let item = match layer.look_up_item(name) {
            Ok(Some(item)) => item,
            Ok(None) => continue, // parent is a table, but no value found
            Err(_) => break,      // parent is not a table, shadows lower layers
        };
        if item.is_table() {
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

/// Looks up non-inline tables to be merged from `layers`, returns in reverse
/// order.
fn get_tables_to_merge<'a>(
    layers: &'a [ConfigLayer],
    name: &ConfigNamePathBuf,
) -> Vec<&'a ConfigTable> {
    let mut to_merge = Vec::new();
    for layer in layers.iter().rev() {
        match layer.look_up_table(name) {
            Ok(Some(table)) => to_merge.push(table),
            Ok(None) => {}   // parent is a table, but no value found
            Err(_) => break, // parent/leaf is not a table, shadows lower layers
        }
    }
    to_merge
}

/// Merges `upper_item` fields into `lower_item` recursively.
fn merge_items(lower_item: &mut ConfigItem, upper_item: &ConfigItem) {
    // Inline table is a value, not a table to be merged.
    let (Some(lower_table), Some(upper_table)) = (lower_item.as_table_mut(), upper_item.as_table())
    else {
        // Not a table, the upper item wins.
        *lower_item = upper_item.clone();
        return;
    };
    for (key, upper) in upper_table {
        match lower_table.entry(key) {
            toml_edit::Entry::Occupied(entry) => {
                merge_items(entry.into_mut(), upper);
            }
            toml_edit::Entry::Vacant(entry) => {
                entry.insert(upper.clone());
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_config_layer_set_value() {
        let mut layer = ConfigLayer::empty(ConfigSource::User);
        // Cannot overwrite the root table
        assert_matches!(
            layer.set_value(ConfigNamePathBuf::root(), 0),
            Err(ConfigUpdateError::WouldOverwriteValue { name }) if name.is_empty()
        );

        // Insert some values
        layer.set_value("foo", 1).unwrap();
        layer.set_value("bar.baz.blah", "2").unwrap();
        layer
            .set_value("bar.qux", ConfigValue::from_iter([("inline", "table")]))
            .unwrap();
        insta::assert_snapshot!(layer.data, @r#"
        foo = 1

        [bar]
        qux = { inline = "table" }

        [bar.baz]
        blah = "2"
        "#);

        // Can overwrite value
        layer
            .set_value("foo", ConfigValue::from_iter(["new", "foo"]))
            .unwrap();
        // Can overwrite inline table
        layer.set_value("bar.qux", "new bar.qux").unwrap();
        // Cannot overwrite table
        assert_matches!(
            layer.set_value("bar", 0),
            Err(ConfigUpdateError::WouldOverwriteTable { name }) if name == "bar"
        );
        // Cannot overwrite value by table
        assert_matches!(
            layer.set_value("bar.baz.blah.blah", 0),
            Err(ConfigUpdateError::WouldOverwriteValue { name }) if name == "bar.baz.blah"
        );
        insta::assert_snapshot!(layer.data, @r#"
        foo = ["new", "foo"]

        [bar]
        qux = "new bar.qux"

        [bar.baz]
        blah = "2"
        "#);
    }

    #[test]
    fn test_stacked_config_layer_order() {
        let empty_data = || DocumentMut::new();
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

        // Insert multiple
        config.extend_layers([
            ConfigLayer::with_data(ConfigSource::Repo, empty_data()),
            ConfigLayer::with_data(ConfigSource::Repo, empty_data()),
            ConfigLayer::with_data(ConfigSource::User, empty_data()),
        ]);
        assert_eq!(
            layer_sources(&config),
            vec![
                ConfigSource::EnvBase,
                ConfigSource::User,
                ConfigSource::Repo,
                ConfigSource::Repo,
                ConfigSource::Repo,
            ]
        );

        // Remove remainders
        config.remove_layers(ConfigSource::EnvBase);
        config.remove_layers(ConfigSource::User);
        config.remove_layers(ConfigSource::Repo);
        assert_eq!(layer_sources(&config), vec![]);
    }

    fn new_user_layer(text: &str) -> ConfigLayer {
        ConfigLayer::parse(ConfigSource::User, text).unwrap()
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
            Err(ConfigGetError::NotFound { name }) if name == "a.b.missing"
        );

        // Node "a.b.c" is not a table
        assert_matches!(
            config.get::<String>("a.b.c.d"),
            Err(ConfigGetError::NotFound { name }) if name == "a.b.c.d"
        );

        // Type error
        assert_matches!(
            config.get::<String>("a.b"),
            Err(ConfigGetError::Type { name, .. }) if name == "a.b"
        );
    }

    #[test]
    fn test_stacked_config_get_table_as_value() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.b = { c = 'a.b.c #0' }
        "}));
        config.add_layer(new_user_layer(indoc! {"
            a.d = ['a.d #1']
        "}));

        // Table can be converted to a value (so it can be deserialized to a
        // structured value.)
        insta::assert_snapshot!(
            config.get_value("a").unwrap(),
            @"{ b = { c = 'a.b.c #0' }, d = ['a.d #1'] }");
    }

    #[test]
    fn test_stacked_config_get_inline_table() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.b = { c = 'a.b.c #0' }
        "}));
        config.add_layer(new_user_layer(indoc! {"
            a.b = { d = 'a.b.d #1' }
        "}));

        // Inline table should override the lower value
        insta::assert_snapshot!(
            config.get_value("a.b").unwrap(),
            @" { d = 'a.b.d #1' }");

        // For API consistency, inner key of inline table cannot be addressed by
        // a dotted name path. This could be supported, but it would be weird if
        // a value could sometimes be accessed as a table.
        assert_matches!(
            config.get_value("a.b.d"),
            Err(ConfigGetError::NotFound { name }) if name == "a.b.d"
        );
        assert_matches!(
            config.get_table("a.b"),
            Err(ConfigGetError::Type { name, .. }) if name == "a.b"
        );
        assert_eq!(config.table_keys("a.b").collect_vec(), vec![""; 0]);
    }

    #[test]
    fn test_stacked_config_get_inline_non_inline_table() {
        let mut config = StackedConfig::empty();
        config.add_layer(new_user_layer(indoc! {"
            a.b = { c = 'a.b.c #0' }
        "}));
        config.add_layer(new_user_layer(indoc! {"
            a.b.d = 'a.b.d #1'
        "}));

        // Non-inline table is not merged with the lower inline table. It might
        // be tempting to merge them, but then the resulting type would become
        // unclear. If the merged type were an inline table, the path "a.b.d"
        // would be shadowed by the lower layer. If the type were a non-inline
        // table, new path "a.b.c" would be born in the upper layer.
        insta::assert_snapshot!(
            config.get_value("a.b").unwrap(),
            @"{ d = 'a.b.d #1' }");
        assert_matches!(
            config.get_value("a.b.c"),
            Err(ConfigGetError::NotFound { name }) if name == "a.b.c"
        );
        insta::assert_snapshot!(
            config.get_value("a.b.d").unwrap(),
            @" 'a.b.d #1'");

        insta::assert_snapshot!(
            config.get_table("a.b").unwrap(),
            @"d = 'a.b.d #1'");
        insta::assert_snapshot!(
            config.get_table("a").unwrap(),
            @"b.d = 'a.b.d #1'");
        assert_eq!(config.table_keys("a.b").collect_vec(), vec!["d"]);
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
            Err(ConfigGetError::NotFound { name }) if name == "a.b.c"
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
        insta::assert_snapshot!(config.get_table("a.b").unwrap(), @"c = 'a.b.c #1'");
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
        insta::assert_snapshot!(config.get_table("a").unwrap(), @r"
        a.a = 'a.a.a #0'
        a.b = 'a.a.b #1'
        a.c = 'a.a.c #1'
        b = 'a.b #0'
        c = 'a.c #1'
        ");
        assert_eq!(config.table_keys("a").collect_vec(), vec!["a", "b", "c"]);
        assert_eq!(config.table_keys("a.a").collect_vec(), vec!["a", "b", "c"]);
        assert_eq!(config.table_keys("a.b").collect_vec(), vec![""; 0]);
        assert_eq!(config.table_keys("a.missing").collect_vec(), vec![""; 0]);
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
        insta::assert_snapshot!(config.get_table("a").unwrap(), @"a.b = 'a.a.b #2'");
        assert_eq!(config.table_keys("a").collect_vec(), vec!["a"]);
        assert_eq!(config.table_keys("a.a").collect_vec(), vec!["b"]);
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
        insta::assert_snapshot!(config.get_table("a").unwrap(), @r"
        a.b = 'a.a.b #2'
        b = 'a.b #0'
        ");
        assert_eq!(config.table_keys("a").collect_vec(), vec!["a", "b"]);
        assert_eq!(config.table_keys("a.a").collect_vec(), vec!["b"]);
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
        // a is not under a.a, but it should still shadow lower layers
        insta::assert_snapshot!(config.get_table("a.a").unwrap(), @"b = 'a.a.b #2'");
        assert_eq!(config.table_keys("a.a").collect_vec(), vec!["b"]);
    }
}
