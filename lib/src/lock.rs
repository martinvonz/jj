// Copyright 2020 Google LLC
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

use backoff::{ExponentialBackoff, Operation};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::time::Duration;

pub struct FileLock {
    path: PathBuf,
    _file: File,
}

impl FileLock {
    pub fn lock(path: PathBuf) -> FileLock {
        let mut options = OpenOptions::new();
        options.create_new(true);
        options.write(true);
        let mut try_write_lock_file = || match options.open(&path) {
            Ok(file) => Ok(FileLock {
                path: path.clone(),
                _file: file,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(backoff::Error::Transient(err))
            }
            Err(err) => Err(backoff::Error::Permanent(err)),
        };
        let mut backoff = ExponentialBackoff {
            initial_interval: Duration::from_millis(1),
            max_elapsed_time: Some(Duration::from_secs(10)),
            ..Default::default()
        };
        match try_write_lock_file.retry(&mut backoff) {
            Err(err) => panic!(
                "failed to create lock file {}: {}",
                path.to_string_lossy(),
                err
            ),
            Ok(file_lock) => file_lock,
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).expect("failed to delete lock file");
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::thread;

    use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

    use super::*;

    #[test]
    fn lock_basic() {
        let number: u32 = rand::random();
        let lock_path = env::temp_dir().join(format!("test-{}.lock", number));
        assert!(!lock_path.exists());
        {
            let _lock = FileLock::lock(lock_path.clone());
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists());
    }

    #[test]
    fn lock_concurrent() {
        let number: u32 = rand::random();
        let data_path = env::temp_dir().join(format!("test-{}", number));
        let lock_path = env::temp_dir().join(format!("test-{}.lock", number));
        let mut data_file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(data_path.clone())
            .unwrap();
        data_file.write_u32::<LittleEndian>(0).unwrap();
        let mut threads = vec![];
        for _ in 0..100 {
            let data_path = data_path.clone();
            let lock_path = lock_path.clone();
            let handle = thread::spawn(move || {
                let _lock = FileLock::lock(lock_path);
                let mut data_file = OpenOptions::new()
                    .read(true)
                    .open(data_path.clone())
                    .unwrap();
                let value = data_file.read_u32::<LittleEndian>().unwrap();
                thread::sleep(Duration::from_millis(1));
                let mut data_file = OpenOptions::new().write(true).open(data_path).unwrap();
                data_file.write_u32::<LittleEndian>(value + 1).unwrap();
            });
            threads.push(handle);
        }
        for thread in threads {
            thread.join().ok().unwrap();
        }
        let mut data_file = OpenOptions::new().read(true).open(data_path).unwrap();
        let value = data_file.read_u32::<LittleEndian>().unwrap();
        assert_eq!(value, 100);
    }
}
