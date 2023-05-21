// Copyright 2020 The Jujutsu Authors
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

#[cfg_attr(unix, path = "lock/unix.rs")]
#[cfg_attr(not(unix), path = "lock/fallback.rs")]
mod platform;

pub use platform::FileLock;

#[cfg(test)]
mod tests {
    use std::cmp::max;
    use std::fs::OpenOptions;
    use std::thread;
    use std::time::Duration;

    use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

    use super::*;

    #[test]
    fn lock_basic() {
        let temp_dir = testutils::new_temp_dir();
        let lock_path = temp_dir.path().join("test.lock");
        assert!(!lock_path.exists());
        {
            let _lock = FileLock::lock(lock_path.clone());
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists());
    }

    #[test]
    fn lock_concurrent() {
        let temp_dir = testutils::new_temp_dir();
        let data_path = temp_dir.path().join("test");
        let lock_path = temp_dir.path().join("test.lock");
        let mut data_file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(data_path.clone())
            .unwrap();
        data_file.write_u32::<LittleEndian>(0).unwrap();
        let num_threads = max(num_cpus::get(), 4);
        thread::scope(|s| {
            for _ in 0..num_threads {
                let data_path = data_path.clone();
                let lock_path = lock_path.clone();
                s.spawn(move || {
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
            }
        });
        let mut data_file = OpenOptions::new().read(true).open(data_path).unwrap();
        let value = data_file.read_u32::<LittleEndian>().unwrap();
        assert_eq!(value, num_threads as u32);
    }
}
