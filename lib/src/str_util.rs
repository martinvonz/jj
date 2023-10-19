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

/// Pattern to be tested against string property like commit description or
/// branch name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StringPattern {
    /// Matches strings exactly equal to `string`.
    Exact(String),
    /// Matches strings that contain `substring`.
    Substring(String),
}

impl StringPattern {
    /// Pattern that matches any string.
    pub const fn everything() -> Self {
        StringPattern::Substring(String::new())
    }

    /// Returns a literal pattern if this should match input strings exactly.
    ///
    /// This can be used to optimize map lookup by exact key.
    pub fn as_exact(&self) -> Option<&str> {
        match self {
            StringPattern::Exact(literal) => Some(literal),
            StringPattern::Substring(_) => None,
        }
    }

    /// Returns true if this pattern matches the `haystack`.
    pub fn matches(&self, haystack: &str) -> bool {
        match self {
            StringPattern::Exact(literal) => haystack == literal,
            StringPattern::Substring(needle) => haystack.contains(needle),
        }
    }
}
