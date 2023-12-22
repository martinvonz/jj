// Copyright 2021 The Jujutsu Authors
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

//! A persistent table of fixed-size keys to variable-size values. The keys are
//! stored in sorted order, with each key followed by an integer offset into the
//! list of values. The values are concatenated after the keys. A file may have
//! a parent file, and the parent may have its own parent, and so on. The child
//! file then represents the union of the entries.

#![allow(missing_docs)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use blake2::{Blake2b512, Digest};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::file_util::persist_content_addressed_temp_file;
use crate::lock::FileLock;

pub trait TableSegment {
    fn segment_num_entries(&self) -> usize;
    fn segment_parent_file(&self) -> Option<&Arc<ReadonlyTable>>;
    fn segment_get_value(&self, key: &[u8]) -> Option<&[u8]>;
    fn segment_add_entries_to(&self, mut_table: &mut MutableTable);

    fn num_entries(&self) -> usize {
        if let Some(parent_file) = self.segment_parent_file() {
            parent_file.num_entries() + self.segment_num_entries()
        } else {
            self.segment_num_entries()
        }
    }

    fn get_value<'a>(&'a self, key: &[u8]) -> Option<&'a [u8]> {
        self.segment_get_value(key)
            .or_else(|| self.segment_parent_file()?.get_value(key))
    }
}

pub struct ReadonlyTable {
    key_size: usize,
    parent_file: Option<Arc<ReadonlyTable>>,
    name: String,
    // Number of entries not counting the parent file
    num_local_entries: usize,
    // The file's entries in the raw format they're stored in on disk.
    index: Vec<u8>,
    values: Vec<u8>,
}

impl ReadonlyTable {
    fn load_from(
        file: &mut dyn Read,
        store: &TableStore,
        name: String,
        key_size: usize,
    ) -> TableStoreResult<Arc<ReadonlyTable>> {
        let read_u32 = |file: &mut dyn Read| -> io::Result<u32> {
            let mut buf = [0; 4];
            file.read_exact(&mut buf)?;
            Ok(u32::from_le_bytes(buf))
        };
        let parent_filename_len = read_u32(file)?;
        let maybe_parent_file = if parent_filename_len > 0 {
            let mut parent_filename_bytes = vec![0; parent_filename_len as usize];
            file.read_exact(&mut parent_filename_bytes)?;
            let parent_filename = String::from_utf8(parent_filename_bytes).unwrap();
            let parent_file = store.load_table(parent_filename)?;
            Some(parent_file)
        } else {
            None
        };
        let num_local_entries = read_u32(file)? as usize;
        let index_size = num_local_entries * ReadonlyTableIndexEntry::size(key_size);
        let mut data = vec![];
        file.read_to_end(&mut data)?;
        let values = data.split_off(index_size);
        let index = data;
        Ok(Arc::new(ReadonlyTable {
            key_size,
            parent_file: maybe_parent_file,
            name,
            num_local_entries,
            index,
            values,
        }))
    }

    pub fn start_mutation(self: &Arc<Self>) -> MutableTable {
        MutableTable::incremental(self.clone())
    }

    fn segment_value_offset_by_pos(&self, pos: usize) -> usize {
        if pos == self.num_local_entries {
            self.values.len()
        } else {
            ReadonlyTableIndexEntry::new(self, pos).value_offset()
        }
    }

    fn segment_value_by_pos(&self, pos: usize) -> &[u8] {
        &self.values
            [self.segment_value_offset_by_pos(pos)..self.segment_value_offset_by_pos(pos + 1)]
    }
}

impl TableSegment for ReadonlyTable {
    fn segment_num_entries(&self) -> usize {
        self.num_local_entries
    }

    fn segment_parent_file(&self) -> Option<&Arc<ReadonlyTable>> {
        self.parent_file.as_ref()
    }

    fn segment_get_value(&self, key: &[u8]) -> Option<&[u8]> {
        let mut low_pos = 0;
        let mut high_pos = self.num_local_entries;
        loop {
            if high_pos == low_pos {
                return None;
            }
            let mid_pos = (low_pos + high_pos) / 2;
            let mid_entry = ReadonlyTableIndexEntry::new(self, mid_pos);
            match key.cmp(mid_entry.key()) {
                Ordering::Less => {
                    high_pos = mid_pos;
                }
                Ordering::Equal => {
                    return Some(self.segment_value_by_pos(mid_pos));
                }
                Ordering::Greater => {
                    low_pos = mid_pos + 1;
                }
            }
        }
    }

    fn segment_add_entries_to(&self, mut_table: &mut MutableTable) {
        for pos in 0..self.num_local_entries {
            let entry = ReadonlyTableIndexEntry::new(self, pos);
            mut_table.add_entry(
                entry.key().to_vec(),
                self.segment_value_by_pos(pos).to_vec(),
            );
        }
    }
}

struct ReadonlyTableIndexEntry<'table> {
    data: &'table [u8],
}

impl<'table> ReadonlyTableIndexEntry<'table> {
    fn new(table: &'table ReadonlyTable, pos: usize) -> Self {
        let entry_size = ReadonlyTableIndexEntry::size(table.key_size);
        let offset = entry_size * pos;
        let data = &table.index[offset..][..entry_size];
        ReadonlyTableIndexEntry { data }
    }

    fn size(key_size: usize) -> usize {
        key_size + 4
    }

    fn key(&self) -> &'table [u8] {
        &self.data[0..self.data.len() - 4]
    }

    fn value_offset(&self) -> usize {
        u32::from_le_bytes(self.data[self.data.len() - 4..].try_into().unwrap()) as usize
    }
}

pub struct MutableTable {
    key_size: usize,
    parent_file: Option<Arc<ReadonlyTable>>,
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MutableTable {
    fn full(key_size: usize) -> Self {
        Self {
            key_size,
            parent_file: None,
            entries: BTreeMap::new(),
        }
    }

    fn incremental(parent_file: Arc<ReadonlyTable>) -> Self {
        let key_size = parent_file.key_size;
        Self {
            key_size,
            parent_file: Some(parent_file),
            entries: BTreeMap::new(),
        }
    }

    pub fn add_entry(&mut self, key: Vec<u8>, value: Vec<u8>) {
        assert_eq!(key.len(), self.key_size);
        self.entries.insert(key, value);
    }

    fn add_entries_from(&mut self, other: &dyn TableSegment) {
        other.segment_add_entries_to(self);
    }

    fn merge_in(&mut self, other: &Arc<ReadonlyTable>) {
        let mut maybe_own_ancestor = self.parent_file.clone();
        let mut maybe_other_ancestor = Some(other.clone());
        let mut files_to_add = vec![];
        loop {
            if maybe_other_ancestor.is_none() {
                break;
            }
            let other_ancestor = maybe_other_ancestor.as_ref().unwrap();
            if maybe_own_ancestor.is_none() {
                files_to_add.push(other_ancestor.clone());
                maybe_other_ancestor = other_ancestor.parent_file.clone();
                continue;
            }
            let own_ancestor = maybe_own_ancestor.as_ref().unwrap();
            if own_ancestor.name == other_ancestor.name {
                break;
            }
            if own_ancestor.num_entries() < other_ancestor.num_entries() {
                files_to_add.push(other_ancestor.clone());
                maybe_other_ancestor = other_ancestor.parent_file.clone();
            } else {
                maybe_own_ancestor = own_ancestor.parent_file.clone();
            }
        }

        for file in files_to_add.iter().rev() {
            self.add_entries_from(file.as_ref());
        }
    }

    fn serialize(self) -> Vec<u8> {
        let mut buf = vec![];

        if let Some(parent_file) = &self.parent_file {
            buf.extend(u32::try_from(parent_file.name.len()).unwrap().to_le_bytes());
            buf.extend_from_slice(parent_file.name.as_bytes());
        } else {
            buf.extend(0_u32.to_le_bytes());
        }

        buf.extend(u32::try_from(self.entries.len()).unwrap().to_le_bytes());

        let mut value_offset = 0_u32;
        for (key, value) in &self.entries {
            buf.extend_from_slice(key);
            buf.extend(value_offset.to_le_bytes());
            value_offset += u32::try_from(value.len()).unwrap();
        }
        for value in self.entries.values() {
            buf.extend_from_slice(value);
        }
        buf
    }

    /// If the MutableTable has more than half the entries of its parent
    /// ReadonlyTable, return MutableTable with the commits from both. This
    /// is done recursively, so the stack of index files has O(log n) files.
    fn maybe_squash_with_ancestors(self) -> MutableTable {
        let mut num_new_entries = self.entries.len();
        let mut files_to_squash = vec![];
        let mut maybe_parent_file = self.parent_file.clone();
        let mut squashed;
        loop {
            match maybe_parent_file {
                Some(parent_file) => {
                    // TODO: We should probably also squash if the parent file has less than N
                    // commits, regardless of how many (few) are in `self`.
                    if 2 * num_new_entries < parent_file.num_local_entries {
                        squashed = MutableTable::incremental(parent_file);
                        break;
                    }
                    num_new_entries += parent_file.num_local_entries;
                    files_to_squash.push(parent_file.clone());
                    maybe_parent_file = parent_file.parent_file.clone();
                }
                None => {
                    squashed = MutableTable::full(self.key_size);
                    break;
                }
            }
        }

        if files_to_squash.is_empty() {
            return self;
        }

        for parent_file in files_to_squash.iter().rev() {
            squashed.add_entries_from(parent_file.as_ref());
        }
        squashed.add_entries_from(&self);
        squashed
    }

    fn save_in(self, store: &TableStore) -> TableStoreResult<Arc<ReadonlyTable>> {
        if self.entries.is_empty() && self.parent_file.is_some() {
            return Ok(self.parent_file.unwrap());
        }

        let buf = self.maybe_squash_with_ancestors().serialize();
        let mut hasher = Blake2b512::new();
        hasher.update(&buf);
        let file_id_hex = hex::encode(hasher.finalize());
        let file_path = store.dir.join(&file_id_hex);

        let mut temp_file = NamedTempFile::new_in(&store.dir)?;
        let file = temp_file.as_file_mut();
        file.write_all(&buf)?;
        persist_content_addressed_temp_file(temp_file, file_path)?;

        ReadonlyTable::load_from(&mut buf.as_slice(), store, file_id_hex, store.key_size)
    }
}

impl TableSegment for MutableTable {
    fn segment_num_entries(&self) -> usize {
        self.entries.len()
    }

    fn segment_parent_file(&self) -> Option<&Arc<ReadonlyTable>> {
        self.parent_file.as_ref()
    }

    fn segment_get_value(&self, key: &[u8]) -> Option<&[u8]> {
        self.entries.get(key).map(Vec::as_slice)
    }

    fn segment_add_entries_to(&self, mut_table: &mut MutableTable) {
        for (key, value) in &self.entries {
            mut_table.add_entry(key.clone(), value.clone());
        }
    }
}

#[derive(Debug, Error)]
#[error(transparent)]
pub struct TableStoreError(#[from] pub io::Error);

pub type TableStoreResult<T> = Result<T, TableStoreError>;

pub struct TableStore {
    dir: PathBuf,
    key_size: usize,
    cached_tables: RwLock<HashMap<String, Arc<ReadonlyTable>>>,
}

impl TableStore {
    pub fn init(dir: PathBuf, key_size: usize) -> Self {
        std::fs::create_dir(dir.join("heads")).unwrap();
        TableStore {
            dir,
            key_size,
            cached_tables: Default::default(),
        }
    }

    pub fn reinit(&self) {
        std::fs::remove_dir_all(self.dir.join("heads")).unwrap();
        TableStore::init(self.dir.clone(), self.key_size);
    }

    pub fn key_size(&self) -> usize {
        self.key_size
    }

    pub fn load(dir: PathBuf, key_size: usize) -> Self {
        TableStore {
            dir,
            key_size,
            cached_tables: Default::default(),
        }
    }

    pub fn save_table(&self, mut_table: MutableTable) -> TableStoreResult<Arc<ReadonlyTable>> {
        let maybe_parent_table = mut_table.parent_file.clone();
        let table = mut_table.save_in(self)?;
        self.add_head(&table)?;
        if let Some(parent_table) = maybe_parent_table {
            if parent_table.name != table.name {
                self.remove_head(&parent_table);
            }
        }
        {
            let mut locked_cache = self.cached_tables.write().unwrap();
            locked_cache.insert(table.name.clone(), table.clone());
        }
        Ok(table)
    }

    fn add_head(&self, table: &Arc<ReadonlyTable>) -> std::io::Result<()> {
        std::fs::write(self.dir.join("heads").join(&table.name), "")
    }

    fn remove_head(&self, table: &Arc<ReadonlyTable>) {
        // It's fine if the old head was not found. It probably means
        // that we're on a distributed file system where the locking
        // doesn't work. We'll probably end up with two current
        // heads. We'll detect that next time we load the table.
        std::fs::remove_file(self.dir.join("heads").join(&table.name)).ok();
    }

    fn lock(&self) -> FileLock {
        FileLock::lock(self.dir.join("lock"))
    }

    fn load_table(&self, name: String) -> TableStoreResult<Arc<ReadonlyTable>> {
        {
            let read_locked_cached = self.cached_tables.read().unwrap();
            if let Some(table) = read_locked_cached.get(&name).cloned() {
                return Ok(table);
            }
        }
        let table_file_path = self.dir.join(&name);
        let mut table_file = File::open(table_file_path)?;
        let table = ReadonlyTable::load_from(&mut table_file, self, name, self.key_size)?;
        {
            let mut write_locked_cache = self.cached_tables.write().unwrap();
            write_locked_cache.insert(table.name.clone(), table.clone());
        }
        Ok(table)
    }

    fn get_head_tables(&self) -> TableStoreResult<Vec<Arc<ReadonlyTable>>> {
        let mut tables = vec![];
        for head_entry in std::fs::read_dir(self.dir.join("heads"))? {
            let head_file_name = head_entry?.file_name();
            let table = self.load_table(head_file_name.to_str().unwrap().to_string())?;
            tables.push(table);
        }
        Ok(tables)
    }

    pub fn get_head(&self) -> TableStoreResult<Arc<ReadonlyTable>> {
        let mut tables = self.get_head_tables()?;

        if tables.is_empty() {
            let empty_table = MutableTable::full(self.key_size);
            self.save_table(empty_table)
        } else if tables.len() == 1 {
            Ok(tables.pop().unwrap())
        } else {
            // There are multiple heads. We take a lock, then check if there are still
            // multiple heads (it's likely that another process was in the process of
            // deleting on of them). If there are still multiple heads, we attempt to
            // merge all the tables into one. We then save that table and record the new
            // head. Note that the locking isn't necessary for correctness; we
            // take the lock only to avoid other concurrent processes from doing
            // the same work (and producing another set of divergent heads).
            let (table, _) = self.get_head_locked()?;
            Ok(table)
        }
    }

    pub fn get_head_locked(&self) -> TableStoreResult<(Arc<ReadonlyTable>, FileLock)> {
        let lock = self.lock();
        let mut tables = self.get_head_tables()?;

        if tables.is_empty() {
            let empty_table = MutableTable::full(self.key_size);
            let table = self.save_table(empty_table)?;
            return Ok((table, lock));
        }

        if tables.len() == 1 {
            // Return early so we don't write a table with no changes compared to its parent
            return Ok((tables.pop().unwrap(), lock));
        }

        let mut merged_table = MutableTable::incremental(tables[0].clone());
        for other in &tables[1..] {
            merged_table.merge_in(other);
        }
        let merged_table = self.save_table(merged_table)?;
        for table in &tables[1..] {
            self.remove_head(table);
        }
        Ok((merged_table, lock))
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn stacked_table_empty(on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);
        let mut_table = store.get_head().unwrap().start_mutation();
        let mut _saved_table = None;
        let table: &dyn TableSegment = if on_disk {
            _saved_table = Some(store.save_table(mut_table).unwrap());
            _saved_table.as_ref().unwrap().as_ref()
        } else {
            &mut_table
        };

        // Cannot find any keys
        assert_eq!(table.get_value(b"\0\0\0"), None);
        assert_eq!(table.get_value(b"aaa"), None);
        assert_eq!(table.get_value(b"\xff\xff\xff"), None);
    }

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn stacked_table_single_key(on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);
        let mut mut_table = store.get_head().unwrap().start_mutation();
        mut_table.add_entry(b"abc".to_vec(), b"value".to_vec());
        let mut _saved_table = None;
        let table: &dyn TableSegment = if on_disk {
            _saved_table = Some(store.save_table(mut_table).unwrap());
            _saved_table.as_ref().unwrap().as_ref()
        } else {
            &mut_table
        };

        // Can find expected keys
        assert_eq!(table.get_value(b"\0\0\0"), None);
        assert_eq!(table.get_value(b"abc"), Some(b"value".as_slice()));
        assert_eq!(table.get_value(b"\xff\xff\xff"), None);
    }

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn stacked_table_multiple_keys(on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);
        let mut mut_table = store.get_head().unwrap().start_mutation();
        mut_table.add_entry(b"zzz".to_vec(), b"val3".to_vec());
        mut_table.add_entry(b"abc".to_vec(), b"value1".to_vec());
        mut_table.add_entry(b"abd".to_vec(), b"value 2".to_vec());
        let mut _saved_table = None;
        let table: &dyn TableSegment = if on_disk {
            _saved_table = Some(store.save_table(mut_table).unwrap());
            _saved_table.as_ref().unwrap().as_ref()
        } else {
            &mut_table
        };

        // Can find expected keys
        assert_eq!(table.get_value(b"\0\0\0"), None);
        assert_eq!(table.get_value(b"abb"), None);
        assert_eq!(table.get_value(b"abc"), Some(b"value1".as_slice()));
        assert_eq!(table.get_value(b"abd"), Some(b"value 2".as_slice()));
        assert_eq!(table.get_value(b"abe"), None);
        assert_eq!(table.get_value(b"zzz"), Some(b"val3".as_slice()));
        assert_eq!(table.get_value(b"\xff\xff\xff"), None);
    }

    #[test]
    fn stacked_table_multiple_keys_with_parent_file() {
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);
        let mut mut_table = store.get_head().unwrap().start_mutation();
        mut_table.add_entry(b"abd".to_vec(), b"value 2".to_vec());
        mut_table.add_entry(b"abc".to_vec(), b"value1".to_vec());
        mut_table.add_entry(b"zzz".to_vec(), b"val3".to_vec());
        for round in 0..10 {
            for i in 0..10 {
                mut_table.add_entry(
                    format!("x{i}{round}").into_bytes(),
                    format!("value {i}{round}").into_bytes(),
                );
            }
            let saved_table = store.save_table(mut_table).unwrap();
            mut_table = MutableTable::incremental(saved_table);
        }

        // Can find expected keys
        assert_eq!(mut_table.get_value(b"\0\0\0"), None);
        assert_eq!(mut_table.get_value(b"x.."), None);
        assert_eq!(mut_table.get_value(b"x14"), Some(b"value 14".as_slice()));
        assert_eq!(mut_table.get_value(b"x41"), Some(b"value 41".as_slice()));
        assert_eq!(mut_table.get_value(b"x49"), Some(b"value 49".as_slice()));
        assert_eq!(mut_table.get_value(b"x94"), Some(b"value 94".as_slice()));
        assert_eq!(mut_table.get_value(b"xAA"), None);
        assert_eq!(mut_table.get_value(b"\xff\xff\xff"), None);
    }

    #[test]
    fn stacked_table_merge() {
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);
        let mut mut_base_table = store.get_head().unwrap().start_mutation();
        mut_base_table.add_entry(b"abc".to_vec(), b"value1".to_vec());
        let base_table = store.save_table(mut_base_table).unwrap();

        let mut mut_table1 = MutableTable::incremental(base_table.clone());
        mut_table1.add_entry(b"abd".to_vec(), b"value 2".to_vec());
        mut_table1.add_entry(b"zzz".to_vec(), b"val3".to_vec());
        mut_table1.add_entry(b"mmm".to_vec(), b"side 1".to_vec());
        let table1 = store.save_table(mut_table1).unwrap();
        let mut mut_table2 = MutableTable::incremental(base_table);
        mut_table2.add_entry(b"yyy".to_vec(), b"val5".to_vec());
        mut_table2.add_entry(b"mmm".to_vec(), b"side 2".to_vec());
        mut_table2.add_entry(b"abe".to_vec(), b"value 4".to_vec());
        mut_table2.merge_in(&table1);

        // Can find expected keys
        assert_eq!(mut_table2.get_value(b"\0\0\0"), None);
        assert_eq!(mut_table2.get_value(b"abc"), Some(b"value1".as_slice()));
        assert_eq!(mut_table2.get_value(b"abd"), Some(b"value 2".as_slice()));
        assert_eq!(mut_table2.get_value(b"abe"), Some(b"value 4".as_slice()));
        // The caller shouldn't write two values for the same key, so it's undefined
        // which wins, but let's test how it currently behaves.
        assert_eq!(mut_table2.get_value(b"mmm"), Some(b"side 1".as_slice()));
        assert_eq!(mut_table2.get_value(b"yyy"), Some(b"val5".as_slice()));
        assert_eq!(mut_table2.get_value(b"zzz"), Some(b"val3".as_slice()));
        assert_eq!(mut_table2.get_value(b"\xff\xff\xff"), None);
    }

    #[test]
    fn stacked_table_automatic_merge() {
        // Same test as above, but here we let the store do the merging on load
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);
        let mut mut_base_table = store.get_head().unwrap().start_mutation();
        mut_base_table.add_entry(b"abc".to_vec(), b"value1".to_vec());
        let base_table = store.save_table(mut_base_table).unwrap();

        let mut mut_table1 = MutableTable::incremental(base_table.clone());
        mut_table1.add_entry(b"abd".to_vec(), b"value 2".to_vec());
        mut_table1.add_entry(b"zzz".to_vec(), b"val3".to_vec());
        mut_table1.add_entry(b"mmm".to_vec(), b"side 1".to_vec());
        store.save_table(mut_table1).unwrap();
        let mut mut_table2 = MutableTable::incremental(base_table);
        mut_table2.add_entry(b"yyy".to_vec(), b"val5".to_vec());
        mut_table2.add_entry(b"mmm".to_vec(), b"side 2".to_vec());
        mut_table2.add_entry(b"abe".to_vec(), b"value 4".to_vec());
        let table2 = store.save_table(mut_table2).unwrap();

        // The saved table does not have the keys from table1
        assert_eq!(table2.get_value(b"abd"), None);

        // Can find expected keys in the merged table we get from get_head()
        let merged_table = store.get_head().unwrap();
        assert_eq!(merged_table.get_value(b"\0\0\0"), None);
        assert_eq!(merged_table.get_value(b"abc"), Some(b"value1".as_slice()));
        assert_eq!(merged_table.get_value(b"abd"), Some(b"value 2".as_slice()));
        assert_eq!(merged_table.get_value(b"abe"), Some(b"value 4".as_slice()));
        // The caller shouldn't write two values for the same key, so it's undefined
        // which wins.
        let value_mmm = merged_table.get_value(b"mmm");
        assert!(value_mmm == Some(b"side 1".as_slice()) || value_mmm == Some(b"side 2".as_slice()));
        assert_eq!(merged_table.get_value(b"yyy"), Some(b"val5".as_slice()));
        assert_eq!(merged_table.get_value(b"zzz"), Some(b"val3".as_slice()));
        assert_eq!(merged_table.get_value(b"\xff\xff\xff"), None);
    }

    #[test]
    fn stacked_table_store_save_empty() {
        let temp_dir = testutils::new_temp_dir();
        let store = TableStore::init(temp_dir.path().to_path_buf(), 3);

        let mut mut_table = store.get_head().unwrap().start_mutation();
        mut_table.add_entry(b"abc".to_vec(), b"value".to_vec());
        store.save_table(mut_table).unwrap();

        let mut_table = store.get_head().unwrap().start_mutation();
        store.save_table(mut_table).unwrap();

        // Table head shouldn't be removed on empty save
        let table = store.get_head().unwrap();
        assert_eq!(table.get_value(b"abc"), Some(b"value".as_slice()));
    }
}
