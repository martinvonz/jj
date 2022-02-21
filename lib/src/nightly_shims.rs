use std::collections::{BTreeMap, BTreeSet};

pub trait BTreeMapExt<K, V> {
    fn ext_first_key(&self) -> Option<&K>;
    fn ext_last_key(&self) -> Option<&K>;
    fn ext_pop_first_key(&mut self) -> Option<K>;
    fn ext_pop_last_key(&mut self) -> Option<K>;
    fn ext_pop_first_value(&mut self) -> Option<V>;
    fn ext_pop_last_value(&mut self) -> Option<V>;
}

impl<K: Ord + Clone, V> BTreeMapExt<K, V> for BTreeMap<K, V> {
    fn ext_first_key(&self) -> Option<&K> {
        self.keys().next()
    }

    fn ext_last_key(&self) -> Option<&K> {
        self.keys().next_back()
    }

    fn ext_pop_first_key(&mut self) -> Option<K> {
        let key = self.ext_first_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().0)
    }

    fn ext_pop_last_key(&mut self) -> Option<K> {
        let key = self.ext_last_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().0)
    }

    fn ext_pop_first_value(&mut self) -> Option<V> {
        let key = self.ext_first_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().1)
    }

    fn ext_pop_last_value(&mut self) -> Option<V> {
        let key = self.ext_last_key()?;
        let key = key.clone(); // ownership hack
        Some(self.remove_entry(&key).unwrap().1)
    }
}

pub trait BTreeSetExt<K> {
    fn ext_last(&self) -> Option<&K>;
    fn ext_pop_last(&mut self) -> Option<K>;
}

impl<K: Ord + Clone> BTreeSetExt<K> for BTreeSet<K> {
    fn ext_last(&self) -> Option<&K> {
        self.iter().next_back()
    }

    fn ext_pop_last(&mut self) -> Option<K> {
        let key = self.ext_last()?;
        let key = key.clone(); // ownership hack
        self.take(&key)
    }
}
