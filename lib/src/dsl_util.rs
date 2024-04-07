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
