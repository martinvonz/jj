// Copyright 2023 The Jujutsu Authors
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

#[cfg(feature = "map_first_last")]
pub trait BTreeMapExt<K, V> {
    fn first_key(&self) -> Option<&K>;
    fn last_key(&self) -> Option<&K>;
    fn pop_first_value(&mut self) -> Option<V>;
    fn pop_last_value(&mut self) -> Option<V>;
}

#[cfg(feature = "map_first_last")]
impl<K: Ord + Clone, V> BTreeMapExt<K, V> for std::collections::BTreeMap<K, V> {
    fn first_key(&self) -> Option<&K> {
        self.keys().next()
    }

    fn last_key(&self) -> Option<&K> {
        self.keys().next_back()
    }
    fn pop_first_value(&mut self) -> Option<V> {
        self.first_entry().map(|x| x.remove())
    }

    fn pop_last_value(&mut self) -> Option<V> {
        self.last_entry().map(|x| x.remove())
    }
}

#[cfg(not(feature = "map_first_last"))]
pub trait BTreeMapExt<K, V> {
    fn first_key(&self) -> Option<&K>;
    fn last_key(&self) -> Option<&K>;
    fn pop_first_key(&mut self) -> Option<K>;
    fn pop_last_key(&mut self) -> Option<K>;
    fn pop_first_value(&mut self) -> Option<V>;
    fn pop_last_value(&mut self) -> Option<V>;
}

#[cfg(not(feature = "map_first_last"))]
impl<K: Ord + Clone, V> BTreeMapExt<K, V> for std::collections::BTreeMap<K, V> {
    fn first_key(&self) -> Option<&K> {
        self.keys().next()
    }

    fn last_key(&self) -> Option<&K> {
        self.keys().next_back()
    }

    fn pop_first_key(&mut self) -> Option<K> {
        let key = self.first_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().0)
    }

    fn pop_last_key(&mut self) -> Option<K> {
        let key = self.last_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().0)
    }

    fn pop_first_value(&mut self) -> Option<V> {
        let key = self.first_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().1)
    }

    fn pop_last_value(&mut self) -> Option<V> {
        let key = self.last_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().1)
    }
}

#[cfg(feature = "map_first_last")]
pub trait BTreeSetExt<K> {}

#[cfg(not(feature = "map_first_last"))]
pub trait BTreeSetExt<K> {
    fn last(&self) -> Option<&K>;
    fn pop_last(&mut self) -> Option<K>;
}

#[cfg(not(feature = "map_first_last"))]
impl<K: Ord + Clone> BTreeSetExt<K> for std::collections::BTreeSet<K> {
    fn last(&self) -> Option<&K> {
        self.iter().next_back()
    }

    fn pop_last(&mut self) -> Option<K> {
        #[allow(unstable_name_collisions)]
        let key = self.last()?;
        let key = key.clone(); // ownership hack
        self.take(&key)
    }
}
