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

fn parse_glob(src: &str) -> Result<glob::Pattern, StringPatternParseError> {
    glob::Pattern::new(src).map_err(StringPatternParseError::GlobPattern)
}

/// Pattern to be tested against string property like commit description or
/// branch name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StringPattern {
    /// Matches strings exactly.
    Exact(String),
    /// Matches strings case‐insensitively.
    ExactI(String),
    /// Matches strings that contain a substring.
    Substring(String),
    /// Matches strings that case‐insensitively contain a substring.
    SubstringI(String),
    /// Matches with a Unix‐style shell wildcard pattern.
    Glob(glob::Pattern),
    /// Matches with a case‐insensitive Unix‐style shell wildcard pattern.
    GlobI(glob::Pattern),
}

impl StringPattern {
    /// Pattern that matches any string.
    pub const fn everything() -> Self {
        StringPattern::Substring(String::new())
    }

    /// Parses the given string as a [`StringPattern`]. Everything before the
    /// first ":" is considered the string's prefix. If the prefix is
    /// "exact[-i]:", "glob[-i]:", or "substring[-i]:", a pattern of the
    /// specified kind is returned. Returns an error if the string has an
    /// unrecognized prefix. Otherwise, a `StringPattern::Exact` is
    /// returned.
    pub fn parse(src: &str) -> Result<StringPattern, StringPatternParseError> {
        if let Some((kind, pat)) = src.split_once(':') {
            StringPattern::from_str_kind(pat, kind)
        } else {
            Ok(StringPattern::exact(src))
        }
    }

    /// Constructs a pattern that matches exactly.
    pub fn exact(src: impl Into<String>) -> Self {
        StringPattern::Exact(src.into())
    }

    /// Constructs a pattern that matches case‐insensitively.
    pub fn exact_i(src: impl Into<String>) -> Self {
        StringPattern::ExactI(src.into())
    }

    /// Constructs a pattern that matches a substring.
    pub fn substring(src: impl Into<String>) -> Self {
        StringPattern::Substring(src.into())
    }

    /// Constructs a pattern that case‐insensitively matches a substring.
    pub fn substring_i(src: impl Into<String>) -> Self {
        StringPattern::SubstringI(src.into())
    }

    /// Parses the given string as a glob pattern.
    pub fn glob(src: &str) -> Result<Self, StringPatternParseError> {
        // TODO: might be better to do parsing and compilation separately since
        // not all backends would use the compiled pattern object.
        // TODO: if no meta character found, it can be mapped to Exact.
        Ok(StringPattern::Glob(parse_glob(src)?))
    }

    /// Parses the given string as a case‐insensitive glob pattern.
    pub fn glob_i(src: &str) -> Result<Self, StringPatternParseError> {
        Ok(StringPattern::GlobI(parse_glob(src)?))
    }

    /// Parses the given string as a pattern of the specified `kind`.
    pub fn from_str_kind(src: &str, kind: &str) -> Result<Self, StringPatternParseError> {
        match kind {
            "exact" => Ok(StringPattern::exact(src)),
            "exact-i" => Ok(StringPattern::exact_i(src)),
            "substring" => Ok(StringPattern::substring(src)),
            "substring-i" => Ok(StringPattern::substring_i(src)),
            "glob" => StringPattern::glob(src),
            "glob-i" => StringPattern::glob_i(src),
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
        // TODO: Handle trivial case‐insensitive patterns here? It might make people
        // expect they can use case‐insensitive patterns in contexts where they
        // generally can’t.
        match self {
            StringPattern::Exact(literal) => Some(literal),
            _ => None,
        }
    }

    /// Returns the original string of this pattern.
    pub fn as_str(&self) -> &str {
        match self {
            StringPattern::Exact(literal) => literal,
            StringPattern::ExactI(literal) => literal,
            StringPattern::Substring(needle) => needle,
            StringPattern::SubstringI(needle) => needle,
            StringPattern::Glob(pattern) => pattern.as_str(),
            StringPattern::GlobI(pattern) => pattern.as_str(),
        }
    }

    /// Converts this pattern to a glob string. Returns `None` if the pattern
    /// can't be represented as a glob.
    pub fn to_glob(&self) -> Option<Cow<'_, str>> {
        // TODO: If we add Regex pattern, it will return None.
        //
        // TODO: Handle trivial case‐insensitive patterns here? It might make people
        // expect they can use case‐insensitive patterns in contexts where they
        // generally can’t.
        match self {
            StringPattern::Exact(literal) => Some(glob::Pattern::escape(literal).into()),
            StringPattern::Substring(needle) => {
                if needle.is_empty() {
                    Some("*".into())
                } else {
                    Some(format!("*{}*", glob::Pattern::escape(needle)).into())
                }
            }
            StringPattern::Glob(pattern) => Some(pattern.as_str().into()),
            StringPattern::ExactI(_) => None,
            StringPattern::SubstringI(_) => None,
            StringPattern::GlobI(_) => None,
        }
    }

    /// Returns true if this pattern matches the `haystack`.
    ///
    /// When matching against a case‐insensitive pattern, only ASCII case
    /// differences are currently folded. This may change in the future.
    pub fn matches(&self, haystack: &str) -> bool {
        // TODO: Unicode case folding is complicated and can be locale‐specific. The
        // `glob` crate and Gitoxide only deal with ASCII case folding, so we do
        // the same here; a more elaborate case folding system will require
        // making sure those behave in a matching manner where relevant.
        //
        // Care will need to be taken regarding normalization and the choice of an
        // appropriate case‐insensitive comparison scheme (`toNFKC_Casefold`?) to ensure
        // that it is compatible with the standard case‐insensitivity of haystack
        // components (like internationalized domain names in email addresses). The
        // availability of normalization and case folding schemes in database backends
        // will also need to be considered. A locale‐specific case folding
        // scheme would likely not be appropriate for Jujutsu.
        //
        // For some discussion of this topic, see:
        // <https://github.com/unicode-org/icu4x/issues/3151>
        match self {
            StringPattern::Exact(literal) => haystack == literal,
            StringPattern::ExactI(literal) => haystack.eq_ignore_ascii_case(literal),
            StringPattern::Substring(needle) => haystack.contains(needle),
            StringPattern::SubstringI(needle) => haystack
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase()),
            StringPattern::Glob(pattern) => pattern.matches(haystack),
            StringPattern::GlobI(pattern) => pattern.matches_with(
                haystack,
                glob::MatchOptions {
                    case_sensitive: false,
                    ..glob::MatchOptions::new()
                },
            ),
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
    use assert_matches::assert_matches;

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
        assert_matches!(
            StringPattern::parse("exact:foo"),
            Ok(StringPattern::Exact(s)) if s == "foo"
        );
        assert_matches!(
            StringPattern::from_str_kind("foo", "exact"),
            Ok(StringPattern::Exact(s)) if s == "foo"
        );
        assert_matches!(
            StringPattern::parse("glob:foo*"),
            Ok(StringPattern::Glob(p)) if p.as_str() == "foo*"
        );
        assert_matches!(
            StringPattern::from_str_kind("foo*", "glob"),
            Ok(StringPattern::Glob(p)) if p.as_str() == "foo*"
        );
        assert_matches!(
            StringPattern::parse("substring:foo"),
            Ok(StringPattern::Substring(s)) if s == "foo"
        );
        assert_matches!(
            StringPattern::from_str_kind("foo", "substring"),
            Ok(StringPattern::Substring(s)) if s == "foo"
        );
        assert_matches!(
            StringPattern::parse("substring-i:foo"),
            Ok(StringPattern::SubstringI(s)) if s == "foo"
        );
        assert_matches!(
            StringPattern::from_str_kind("foo", "substring-i"),
            Ok(StringPattern::SubstringI(s)) if s == "foo"
        );

        // Parse a pattern that contains a : itself.
        assert_matches!(
            StringPattern::parse("exact:foo:bar"),
            Ok(StringPattern::Exact(s)) if s == "foo:bar"
        );

        // If no kind is specified, the input is treated as an exact pattern.
        assert_matches!(
            StringPattern::parse("foo"),
            Ok(StringPattern::Exact(s)) if s == "foo"
        );

        // Parsing an unknown prefix results in an error.
        assert_matches!(
            StringPattern::parse("unknown-prefix:foo"),
            Err(StringPatternParseError::InvalidKind(_))
        );
    }
}
