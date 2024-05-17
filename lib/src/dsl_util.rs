// Copyright 2020-2024 The Jujutsu Authors
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

//! Domain-specific language helpers.

use std::collections::HashMap;
use std::fmt;

use itertools::Itertools as _;
use pest::iterators::Pairs;
use pest::RuleType;

/// Helper to parse string literal.
#[derive(Debug)]
pub struct StringLiteralParser<R> {
    /// String content part.
    pub content_rule: R,
    /// Escape sequence part including backslash character.
    pub escape_rule: R,
}

impl<R: RuleType> StringLiteralParser<R> {
    /// Parses the given string literal `pairs` into string.
    pub fn parse(&self, pairs: Pairs<R>) -> String {
        let mut result = String::new();
        for part in pairs {
            if part.as_rule() == self.content_rule {
                result.push_str(part.as_str());
            } else if part.as_rule() == self.escape_rule {
                match &part.as_str()[1..] {
                    "\"" => result.push('"'),
                    "\\" => result.push('\\'),
                    "t" => result.push('\t'),
                    "r" => result.push('\r'),
                    "n" => result.push('\n'),
                    "0" => result.push('\0'),
                    char => panic!("invalid escape: \\{char:?}"),
                }
            } else {
                panic!("unexpected part of string: {part:?}");
            }
        }
        result
    }
}

/// Map of symbol and function aliases.
#[derive(Clone, Debug, Default)]
pub struct AliasesMap<P> {
    symbol_aliases: HashMap<String, String>,
    function_aliases: HashMap<String, (Vec<String>, String)>,
    // Parser type P helps prevent misuse of AliasesMap of different language.
    parser: P,
}

impl<P> AliasesMap<P> {
    /// Creates an empty aliases map with default-constructed parser.
    pub fn new() -> Self
    where
        P: Default,
    {
        Self::default()
    }

    /// Adds new substitution rule `decl = defn`.
    ///
    /// Returns error if `decl` is invalid. The `defn` part isn't checked. A bad
    /// `defn` will be reported when the alias is substituted.
    pub fn insert(&mut self, decl: impl AsRef<str>, defn: impl Into<String>) -> Result<(), P::Error>
    where
        P: AliasDeclarationParser,
    {
        match self.parser.parse_declaration(decl.as_ref())? {
            AliasDeclaration::Symbol(name) => {
                self.symbol_aliases.insert(name, defn.into());
            }
            AliasDeclaration::Function(name, params) => {
                self.function_aliases.insert(name, (params, defn.into()));
            }
        }
        Ok(())
    }

    /// Iterates function names in arbitrary order.
    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.function_aliases.keys().map(|n| n.as_ref())
    }

    /// Looks up symbol alias by name. Returns identifier and definition text.
    pub fn get_symbol(&self, name: &str) -> Option<(AliasId<'_>, &str)> {
        self.symbol_aliases
            .get_key_value(name)
            .map(|(name, defn)| (AliasId::Symbol(name), defn.as_ref()))
    }

    /// Looks up function alias by name. Returns identifier, list of parameter
    /// names, and definition text.
    pub fn get_function(&self, name: &str) -> Option<(AliasId<'_>, &[String], &str)> {
        self.function_aliases
            .get_key_value(name)
            .map(|(name, (params, defn))| (AliasId::Function(name), params.as_ref(), defn.as_ref()))
    }
}

/// Borrowed reference to identify alias expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AliasId<'a> {
    /// Symbol name.
    Symbol(&'a str),
    /// Function name.
    Function(&'a str),
}

impl fmt::Display for AliasId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AliasId::Symbol(name) => write!(f, "{name}"),
            AliasId::Function(name) => write!(f, "{name}()"),
        }
    }
}

/// Parsed declaration part of alias rule.
#[derive(Clone, Debug)]
pub enum AliasDeclaration {
    /// Symbol name.
    Symbol(String),
    /// Function name and parameters.
    Function(String, Vec<String>),
}

/// Parser for symbol and function alias declaration.
pub trait AliasDeclarationParser {
    /// Parse error type.
    type Error;

    /// Parses symbol or function name and parameters.
    fn parse_declaration(&self, source: &str) -> Result<AliasDeclaration, Self::Error>;
}

/// Collects similar names from the `candidates` list.
pub fn collect_similar<I>(name: &str, candidates: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    candidates
        .into_iter()
        .filter(|cand| {
            // The parameter is borrowed from clap f5540d26
            strsim::jaro(name, cand.as_ref()) > 0.7
        })
        .map(|s| s.as_ref().to_owned())
        .sorted_unstable()
        .dedup()
        .collect()
}
