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

#![allow(missing_docs)]

#[cfg_attr(unix, path = "lock/unix.rs")]
#[cfg_attr(not(unix), path = "lock/fallback.rs")]
mod platform;

pub use platform::FileLock;

#[cfg(test)]
mod tests {
    use std::cmp::max;
    use std::time::Duration;
    use std::{fs, thread};

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
        fs::write(&data_path, 0_u32.to_le_bytes()).unwrap();
        let num_threads = max(num_cpus::get(), 4);
        thread::scope(|s| {
            for _ in 0..num_threads {
                s.spawn(|| {
                    let _lock = FileLock::lock(lock_path.clone());
                    let data = fs::read(&data_path).unwrap();
                    let value = u32::from_le_bytes(data.try_into().unwrap());
                    thread::sleep(Duration::from_millis(1));
                    fs::write(&data_path, (value + 1).to_le_bytes()).unwrap();
                });
            }
        });
        let data = fs::read(&data_path).unwrap();
        let value = u32::from_le_bytes(data.try_into().unwrap());
        assert_eq!(value, num_threads as u32);
    }
}
