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

/// Case‐sensitivity option for [`StringPattern`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaseSensitivity {
    /// Match case‐sensitively.
    Sensitive,
    /// Match case‐insensitively. Only ASCII case differences are currently
    /// folded.
    Insensitive,
}

/// Pattern to be tested against string property like commit description or
/// branch name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StringPattern {
    /// Matches strings equal to `string`.
    Exact(String, CaseSensitivity),
    /// Matches strings that contain `substring`.
    Substring(String, CaseSensitivity),
    /// Unix-style shell wildcard pattern.
    Glob(glob::Pattern, CaseSensitivity),
}

impl StringPattern {
    /// Pattern that matches any string.
    pub const fn everything() -> Self {
        StringPattern::Substring(String::new(), CaseSensitivity::Sensitive)
    }

    /// Parses the given string as a [`StringPattern`]. Everything before the
    /// first “:” is considered the string’s prefix. If the prefix is
    /// `exact[-i]:`, `substring[-i]`, `glob[-i]:`, or or `i:` (short for
    /// `{default_kind}-i:`), a pattern of the specified kind is returned.
    /// Returns an error if the string has an unrecognized prefix. Otherwise,
    /// a pattern of kind `default_kind` is returned.
    pub fn parse_with_default_kind(
        src: &str,
        default_kind: &str,
    ) -> Result<Self, StringPatternParseError> {
        let (maybe_kind, pat) = match src.split_once(':') {
            Some((kind, pat)) => (Some(kind), pat),
            None => (None, src),
        };
        Self::from_str_maybe_kind(pat, maybe_kind, default_kind)
    }

    /// Helper for `StringPattern::parse_with_default_kind(src, "exact')`. See
    /// [`StringPattern::parse_with_default_kind()`] for details.
    pub fn parse(src: &str) -> Result<StringPattern, StringPatternParseError> {
        Self::parse_with_default_kind(src, "exact")
    }

    /// Creates pattern that matches exactly.
    pub fn exact(src: impl Into<String>, case_sensitivity: CaseSensitivity) -> Self {
        StringPattern::Exact(src.into(), case_sensitivity)
    }

    /// Parses the given string as glob pattern.
    pub fn glob(
        src: &str,
        case_sensitivity: CaseSensitivity,
    ) -> Result<Self, StringPatternParseError> {
        // TODO: might be better to do parsing and compilation separately since
        // not all backends would use the compiled pattern object.
        // TODO: if no meta character found, it can be mapped to Exact.
        let pattern = glob::Pattern::new(src).map_err(StringPatternParseError::GlobPattern)?;
        Ok(StringPattern::Glob(pattern, case_sensitivity))
    }

    /// Parses the given string as a pattern of the specified `kind`.
    pub fn from_str_kind(src: &str, kind: &str) -> Result<Self, StringPatternParseError> {
        let (base_kind, case_sensitivity) = match kind.strip_suffix("-i") {
            Some(base_kind) => (base_kind, CaseSensitivity::Insensitive),
            None => (kind, CaseSensitivity::Sensitive),
        };
        match base_kind {
            "exact" => Ok(StringPattern::exact(src, case_sensitivity)),
            "substring" => Ok(StringPattern::Substring(src.to_owned(), case_sensitivity)),
            "glob" => StringPattern::glob(src, case_sensitivity),
            _ => Err(StringPatternParseError::InvalidKind(kind.to_owned())),
        }
    }

    /// Parses the given string as a pattern of the specified `maybe_kind` if
    /// present, or `default_kind` otherwise.
    pub fn from_str_maybe_kind(
        src: &str,
        maybe_kind: Option<&str>,
        default_kind: &str,
    ) -> Result<Self, StringPatternParseError> {
        match maybe_kind {
            None => Self::from_str_kind(src, default_kind),
            Some("i") => Self::from_str_kind(src, &(default_kind.to_owned() + "-i")),
            Some(kind) => Self::from_str_kind(src, kind),
        }
    }

    /// Returns true if this pattern matches input strings exactly.
    pub fn is_case_sensitive_exact(&self) -> bool {
        self.as_case_sensitive_exact().is_some()
    }

    /// Returns a literal pattern if this should match input strings exactly.
    ///
    /// This can be used to optimize map lookup by exact key.
    pub fn as_case_sensitive_exact(&self) -> Option<&str> {
        // TODO: Handle trivial case‐insensitive patterns here? It might make people
        // expect they can use case‐insensitive patterns in contexts where they
        // generally can’t.
        match self {
            StringPattern::Exact(literal, CaseSensitivity::Sensitive) => Some(literal),
            _ => None,
        }
    }

    /// Returns the original string of this pattern.
    pub fn as_str(&self) -> &str {
        match self {
            StringPattern::Exact(literal, _) => literal,
            StringPattern::Substring(needle, _) => needle,
            StringPattern::Glob(pattern, _) => pattern.as_str(),
        }
    }

    /// Converts this pattern to a glob string. Returns `None` if the pattern
    /// can't be represented as a glob.
    pub fn to_case_sensitive_glob(&self) -> Option<Cow<'_, str>> {
        // TODO: If we add Regex pattern, it will return None.
        // TODO: Handle trivial case‐insensitive patterns here? It might make people
        // expect they can use case‐insensitive patterns in contexts where they
        // generally can’t.
        match self {
            StringPattern::Exact(literal, CaseSensitivity::Sensitive) => {
                Some(glob::Pattern::escape(literal).into())
            }
            StringPattern::Substring(needle, CaseSensitivity::Sensitive) => {
                if needle.is_empty() {
                    Some("*".into())
                } else {
                    Some(format!("*{}*", glob::Pattern::escape(needle)).into())
                }
            }
            StringPattern::Glob(pattern, CaseSensitivity::Sensitive) => {
                Some(pattern.as_str().into())
            }
            _ => None,
        }
    }

    /// Returns true if this pattern matches the `haystack`.
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
            StringPattern::Exact(literal, CaseSensitivity::Sensitive) => haystack == literal,
            StringPattern::Exact(literal, CaseSensitivity::Insensitive) => {
                haystack.to_ascii_lowercase() == literal.to_ascii_lowercase()
            }
            StringPattern::Glob(pattern, case_sensitivity) => pattern.matches_with(
                haystack,
                glob::MatchOptions {
                    case_sensitive: *case_sensitivity == CaseSensitivity::Sensitive,
                    ..glob::MatchOptions::new()
                },
            ),
            StringPattern::Substring(needle, CaseSensitivity::Sensitive) => {
                haystack.contains(needle)
            }
            StringPattern::Substring(needle, CaseSensitivity::Insensitive) => haystack
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase()),
        }
    }

    /// Iterates entries of the given `map` whose keys matches this pattern.
    pub fn filter_btree_map<'a: 'b, 'b, K: Borrow<str> + Ord, V>(
        &'b self,
        map: &'a BTreeMap<K, V>,
    ) -> impl Iterator<Item = (&'a K, &'a V)> + 'b {
        if let Some(key) = self.as_case_sensitive_exact() {
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
        assert_eq!(
            StringPattern::everything().to_case_sensitive_glob(),
            Some("*".into())
        );
        assert_eq!(
            StringPattern::exact("a", CaseSensitivity::Sensitive).to_case_sensitive_glob(),
            Some("a".into())
        );
        assert_eq!(
            StringPattern::exact("*", CaseSensitivity::Sensitive).to_case_sensitive_glob(),
            Some("[*]".into())
        );
        assert_eq!(
            StringPattern::glob("*", CaseSensitivity::Sensitive)
                .unwrap()
                .to_case_sensitive_glob(),
            Some("*".into())
        );
        assert_eq!(
            StringPattern::Substring("a".into(), CaseSensitivity::Sensitive)
                .to_case_sensitive_glob(),
            Some("*a*".into())
        );
        assert_eq!(
            StringPattern::Substring("*".into(), CaseSensitivity::Sensitive)
                .to_case_sensitive_glob(),
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
        assert_eq!(
            StringPattern::parse("i:foo").unwrap(),
            StringPattern::from_str_kind("foo", "exact-i").unwrap()
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
