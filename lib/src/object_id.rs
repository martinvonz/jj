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

#![allow(missing_docs)]

pub trait ObjectId {
    fn object_type(&self) -> String;
    fn as_bytes(&self) -> &[u8];
    fn to_bytes(&self) -> Vec<u8>;
    fn hex(&self) -> String;
}

// Defines a new struct type with visibility `vis` and name `ident` containing
// a single Vec<u8> used to store an identifier (typically the output of a hash
// function) as bytes. Types defined using this macro automatically implement
// the `ObjectId` and `ContentHash` traits.
// Documentation comments written inside the macro definition and will be
// captured and associated with the type defined by the macro.
//
// Example:
// ```no_run
// id_type!(
//     /// My favorite id type.
//     pub MyId
// );
// ```
macro_rules! id_type {
    (   $(#[$attr:meta])*
        $vis:vis $name:ident
    ) => {
        $(#[$attr])*
        #[derive(ContentHash, PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
        $vis struct $name(Vec<u8>);
        $crate::object_id::impl_id_type!($name);
    };
}

macro_rules! impl_id_type {
    ($name:ident) => {
        impl $name {
            pub fn new(value: Vec<u8>) -> Self {
                Self(value)
            }

            pub fn from_bytes(bytes: &[u8]) -> Self {
                Self(bytes.to_vec())
            }

            /// Parses the given hex string into an ObjectId.
            ///
            /// The given string must be valid. A static str is required to
            /// prevent API misuse.
            pub fn from_hex(hex: &'static str) -> Self {
                Self::try_from_hex(hex).unwrap()
            }

            /// Parses the given hex string into an ObjectId.
            pub fn try_from_hex(hex: &str) -> Result<Self, hex::FromHexError> {
                hex::decode(hex).map(Self)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                f.debug_tuple(stringify!($name)).field(&self.hex()).finish()
            }
        }

        impl crate::object_id::ObjectId for $name {
            fn object_type(&self) -> String {
                stringify!($name)
                    .strip_suffix("Id")
                    .unwrap()
                    .to_ascii_lowercase()
                    .to_string()
            }

            fn as_bytes(&self) -> &[u8] {
                &self.0
            }

            fn to_bytes(&self) -> Vec<u8> {
                self.0.clone()
            }

            fn hex(&self) -> String {
                hex::encode(&self.0)
            }
        }
    };
}

pub(crate) use {id_type, impl_id_type};

/// An identifier prefix (typically from a type implementing the [`ObjectId`]
/// trait) with facilities for converting between bytes and a hex string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexPrefix {
    // For odd-length prefixes, the lower 4 bits of the last byte are
    // zero-filled (e.g. the prefix "abc" is stored in two bytes as "abc0").
    min_prefix_bytes: Vec<u8>,
    has_odd_byte: bool,
}

impl HexPrefix {
    /// Returns a new `HexPrefix` or `None` if `prefix` cannot be decoded from
    /// hex to bytes.
    pub fn new(prefix: &str) -> Option<HexPrefix> {
        let has_odd_byte = prefix.len() & 1 != 0;
        let min_prefix_bytes = if has_odd_byte {
            hex::decode(prefix.to_owned() + "0").ok()?
        } else {
            hex::decode(prefix).ok()?
        };
        Some(HexPrefix {
            min_prefix_bytes,
            has_odd_byte,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        HexPrefix {
            min_prefix_bytes: bytes.to_owned(),
            has_odd_byte: false,
        }
    }

    pub fn hex(&self) -> String {
        let mut hex_string = hex::encode(&self.min_prefix_bytes);
        if self.has_odd_byte {
            hex_string.pop().unwrap();
        }
        hex_string
    }

    /// Minimum bytes that would match this prefix. (e.g. "abc0" for "abc")
    ///
    /// Use this to partition a sorted slice, and test `matches(id)` from there.
    pub fn min_prefix_bytes(&self) -> &[u8] {
        &self.min_prefix_bytes
    }

    /// Returns the bytes representation if this prefix can be a full id.
    pub fn as_full_bytes(&self) -> Option<&[u8]> {
        (!self.has_odd_byte).then_some(&self.min_prefix_bytes)
    }

    fn split_odd_byte(&self) -> (Option<u8>, &[u8]) {
        if self.has_odd_byte {
            let (&odd, prefix) = self.min_prefix_bytes.split_last().unwrap();
            (Some(odd), prefix)
        } else {
            (None, &self.min_prefix_bytes)
        }
    }

    /// Returns whether the stored prefix matches the prefix of `id`.
    pub fn matches<Q: ObjectId>(&self, id: &Q) -> bool {
        let id_bytes = id.as_bytes();
        let (maybe_odd, prefix) = self.split_odd_byte();
        if id_bytes.starts_with(prefix) {
            if let Some(odd) = maybe_odd {
                matches!(id_bytes.get(prefix.len()), Some(v) if v & 0xf0 == odd)
            } else {
                true
            }
        } else {
            false
        }
    }
}

/// The result of a prefix search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixResolution<T> {
    NoMatch,
    SingleMatch(T),
    AmbiguousMatch,
}

impl<T> PrefixResolution<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> PrefixResolution<U> {
        match self {
            PrefixResolution::NoMatch => PrefixResolution::NoMatch,
            PrefixResolution::SingleMatch(x) => PrefixResolution::SingleMatch(f(x)),
            PrefixResolution::AmbiguousMatch => PrefixResolution::AmbiguousMatch,
        }
    }
}

impl<T: Clone> PrefixResolution<T> {
    pub fn plus(&self, other: &PrefixResolution<T>) -> PrefixResolution<T> {
        match (self, other) {
            (PrefixResolution::NoMatch, other) => other.clone(),
            (local, PrefixResolution::NoMatch) => local.clone(),
            (PrefixResolution::AmbiguousMatch, _) => PrefixResolution::AmbiguousMatch,
            (_, PrefixResolution::AmbiguousMatch) => PrefixResolution::AmbiguousMatch,
            (PrefixResolution::SingleMatch(_), PrefixResolution::SingleMatch(_)) => {
                PrefixResolution::AmbiguousMatch
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::CommitId;

    #[test]
    fn test_hex_prefix_prefixes() {
        let prefix = HexPrefix::new("").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"");

        let prefix = HexPrefix::new("1").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"\x10");

        let prefix = HexPrefix::new("12").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"\x12");

        let prefix = HexPrefix::new("123").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"\x12\x30");

        let bad_prefix = HexPrefix::new("0x123");
        assert_eq!(bad_prefix, None);

        let bad_prefix = HexPrefix::new("foobar");
        assert_eq!(bad_prefix, None);
    }

    #[test]
    fn test_hex_prefix_matches() {
        let id = CommitId::from_hex("1234");

        assert!(HexPrefix::new("").unwrap().matches(&id));
        assert!(HexPrefix::new("1").unwrap().matches(&id));
        assert!(HexPrefix::new("12").unwrap().matches(&id));
        assert!(HexPrefix::new("123").unwrap().matches(&id));
        assert!(HexPrefix::new("1234").unwrap().matches(&id));
        assert!(!HexPrefix::new("12345").unwrap().matches(&id));

        assert!(!HexPrefix::new("a").unwrap().matches(&id));
        assert!(!HexPrefix::new("1a").unwrap().matches(&id));
        assert!(!HexPrefix::new("12a").unwrap().matches(&id));
        assert!(!HexPrefix::new("123a").unwrap().matches(&id));
    }
}
