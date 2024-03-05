// Copyright 2024 The Jujutsu Authors
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

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Type-safe map that stores objects of arbitrary types.
///
/// This allows extensions to store and retrieve their own types unknown to
/// jj_lib safely.
#[derive(Default)]
pub struct ExtensionsMap {
    values: HashMap<TypeId, Box<dyn Any>>,
}

impl ExtensionsMap {
    /// Creates an empty ExtensionsMap.
    pub fn empty() -> Self {
        Default::default()
    }

    /// Returns the specified type if it has already been inserted.
    pub fn get<V: Any>(&self) -> Option<&V> {
        self.values
            .get(&TypeId::of::<V>())
            .map(|v| v.downcast_ref::<V>().unwrap())
    }

    /// Inserts a new instance of the specified type.
    ///
    /// Requires that this type has not been inserted before.
    pub fn insert<V: Any>(&mut self, value: V) {
        assert!(self
            .values
            .insert(TypeId::of::<V>(), Box::new(value))
            .is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTypeA;
    impl TestTypeA {
        fn get_a(&self) -> &'static str {
            "a"
        }
    }

    struct TestTypeB;
    impl TestTypeB {
        fn get_b(&self) -> &'static str {
            "b"
        }
    }

    #[test]
    fn test_empty() {
        let extensions_map = ExtensionsMap::empty();
        assert!(extensions_map.get::<TestTypeA>().is_none());
        assert!(extensions_map.get::<TestTypeB>().is_none());
    }

    #[test]
    fn test_retrieval() {
        let mut extensions_map = ExtensionsMap::empty();
        extensions_map.insert(TestTypeA);
        extensions_map.insert(TestTypeB);
        assert_eq!(
            extensions_map
                .get::<TestTypeA>()
                .map(|a| a.get_a())
                .unwrap_or(""),
            "a"
        );
        assert_eq!(
            extensions_map
                .get::<TestTypeB>()
                .map(|b| b.get_b())
                .unwrap_or(""),
            "b"
        );
    }
}
