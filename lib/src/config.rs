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
use std::slice;
use std::str::FromStr;

use config::Source as _;
use itertools::Itertools as _;

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
    pub fn lookup_value(&self, config: &config::Config) -> Result<config::Value, ConfigError> {
        // Use config.get() if the TOML keys can be converted to config path
        // syntax. This should be cheaper than cloning the whole config map.
        let (key_prefix, components) = self.split_safe_prefix();
        let value: config::Value = match &key_prefix {
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
}
