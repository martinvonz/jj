// Copyright 2021-2023 The Jujutsu Authors
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

//! String helpers.

use std::borrow::{Borrow, Cow};
use std::collections::BTreeMap;
use std::fmt;

use either::Either;
use thiserror::Error;

/// Error occurred during pattern string parsing.
#[derive(Debug, Error)]
pub enum StringPatternParseError {
    /// Unknown pattern kind is specified.
    #[error(r#"Invalid string pattern kind "{0}:""#)]
    InvalidKind(String),
    /// Failed to parse glob pattern.
    #[error(transparent)]
    GlobPattern(glob::PatternError),
}

/// Pattern to be tested against string property like commit description or
/// branch name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StringPattern {
    /// Matches strings exactly equal to `string`.
    Exact(String),
    /// Unix-style shell wildcard pattern.
    Glob(glob::Pattern),
    /// Matches strings that contain `substring`.
    Substring(String),
}

impl StringPattern {
    /// Pattern that matches any string.
    pub const fn everything() -> Self {
        StringPattern::Substring(String::new())
    }

    /// Parses the given string as a `StringPattern`. Everything before the
    /// first ":" is considered the string's prefix. If the prefix is "exact:",
    /// "glob:", or "substring:", a pattern of the specified kind is returned.
    /// Returns an error if the string has an unrecognized prefix. Otherwise, a
    /// `StringPattern::Exact` is returned.
    pub fn parse(src: &str) -> Result<StringPattern, StringPatternParseError> {
        if let Some((kind, pat)) = src.split_once(':') {
            StringPattern::from_str_kind(pat, kind)
        } else {
            Ok(StringPattern::exact(src))
        }
    }

    /// Creates pattern that matches exactly.
    pub fn exact(src: impl Into<String>) -> Self {
        StringPattern::Exact(src.into())
    }

    /// Parses the given string as glob pattern.
    pub fn glob(src: &str) -> Result<Self, StringPatternParseError> {
        // TODO: might be better to do parsing and compilation separately since
        // not all backends would use the compiled pattern object.
        // TODO: if no meta character found, it can be mapped to Exact.
        let pattern = glob::Pattern::new(src).map_err(StringPatternParseError::GlobPattern)?;
        Ok(StringPattern::Glob(pattern))
    }

    /// Parses the given string as pattern of the specified `kind`.
    pub fn from_str_kind(src: &str, kind: &str) -> Result<Self, StringPatternParseError> {
        match kind {
            "exact" => Ok(StringPattern::exact(src)),
            "glob" => StringPattern::glob(src),
            "substring" => Ok(StringPattern::Substring(src.to_owned())),
            _ => Err(StringPatternParseError::InvalidKind(kind.to_owned())),
        }
    }

    /// Returns true if this pattern matches input strings exactly.
    pub fn is_exact(&self) -> bool {
        self.as_exact().is_some()
    }

    /// Returns a literal pattern if this should match input strings exactly.
    ///
    /// This can be used to optimize map lookup by exact key.
    pub fn as_exact(&self) -> Option<&str> {
        match self {
            StringPattern::Exact(literal) => Some(literal),
            StringPattern::Glob(_) | StringPattern::Substring(_) => None,
        }
    }

    /// Returns the original string of this pattern.
    pub fn as_str(&self) -> &str {
        match self {
            StringPattern::Exact(literal) => literal,
            StringPattern::Glob(pattern) => pattern.as_str(),
            StringPattern::Substring(needle) => needle,
        }
    }

    /// Converts this pattern to a glob string. Returns `None` if the pattern
    /// can't be represented as a glob.
    pub fn to_glob(&self) -> Option<Cow<'_, str>> {
        // TODO: If we add Regex pattern, it will return None.
        match self {
            StringPattern::Exact(literal) => Some(glob::Pattern::escape(literal).into()),
            StringPattern::Glob(pattern) => Some(pattern.as_str().into()),
            StringPattern::Substring(needle) if needle.is_empty() => Some("*".into()),
            StringPattern::Substring(needle) => {
                Some(format!("*{}*", glob::Pattern::escape(needle)).into())
            }
        }
    }

    /// Returns true if this pattern matches the `haystack`.
    pub fn matches(&self, haystack: &str) -> bool {
        match self {
            StringPattern::Exact(literal) => haystack == literal,
            StringPattern::Glob(pattern) => pattern.matches(haystack),
            StringPattern::Substring(needle) => haystack.contains(needle),
        }
    }

    /// Iterates entries of the given `map` whose keys matches this pattern.
    pub fn filter_btree_map<'a: 'b, 'b, K: Borrow<str> + Ord, V>(
        &'b self,
        map: &'a BTreeMap<K, V>,
    ) -> impl Iterator<Item = (&'a K, &'a V)> + 'b {
        if let Some(key) = self.as_exact() {
            Either::Left(map.get_key_value(key).into_iter())
        } else {
            Either::Right(map.iter().filter(|&(key, _)| self.matches(key.borrow())))
        }
    }
}

impl fmt::Display for StringPattern {
    /// Shows the original string of this pattern.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_pattern_to_glob() {
        assert_eq!(StringPattern::everything().to_glob(), Some("*".into()));
        assert_eq!(StringPattern::exact("a").to_glob(), Some("a".into()));
        assert_eq!(StringPattern::exact("*").to_glob(), Some("[*]".into()));
        assert_eq!(
            StringPattern::glob("*").unwrap().to_glob(),
            Some("*".into())
        );
        assert_eq!(
            StringPattern::Substring("a".into()).to_glob(),
            Some("*a*".into())
        );
        assert_eq!(
            StringPattern::Substring("*".into()).to_glob(),
            Some("*[*]*".into())
        );
    }

    #[test]
    fn test_parse() {
        // Parse specific pattern kinds.
        assert_eq!(
            StringPattern::parse("exact:foo").unwrap(),
            StringPattern::from_str_kind("foo", "exact").unwrap()
        );
        assert_eq!(
            StringPattern::parse("glob:foo*").unwrap(),
            StringPattern::from_str_kind("foo*", "glob").unwrap()
        );
        assert_eq!(
            StringPattern::parse("substring:foo").unwrap(),
            StringPattern::from_str_kind("foo", "substring").unwrap()
        );

        // Parse a pattern that contains a : itself.
        assert_eq!(
            StringPattern::parse("exact:foo:bar").unwrap(),
            StringPattern::from_str_kind("foo:bar", "exact").unwrap()
        );

        // If no kind is specified, the input is treated as an exact pattern.
        assert_eq!(
            StringPattern::parse("foo").unwrap(),
            StringPattern::from_str_kind("foo", "exact").unwrap()
        );

        // Parsing an unknown prefix results in an error.
        assert!(matches! {
            StringPattern::parse("unknown-prefix:foo"),
            Err(StringPatternParseError::InvalidKind(_))
        });
    }
}
