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

use std::fs::File;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::Duration;

use tracing::instrument;

use super::FileLockError;

pub struct FileLock {
    path: PathBuf,
    _file: File,
}

struct BackoffIterator {
    next_sleep_secs: f32,
    elapsed_secs: f32,
}

impl BackoffIterator {
    fn new() -> Self {
        Self {
            next_sleep_secs: 0.001,
            elapsed_secs: 0.0,
        }
    }
}

impl Iterator for BackoffIterator {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        if self.elapsed_secs >= 10.0 {
            None
        } else {
            let current_sleep = self.next_sleep_secs * (rand::random::<f32>() + 0.5);
            self.next_sleep_secs *= 1.5;
            self.elapsed_secs += current_sleep;
            Some(Duration::from_secs_f32(current_sleep))
        }
    }
}

// Suppress warning on platforms where specialized lock impl is available
#[cfg_attr(unix, allow(dead_code))]
impl FileLock {
    pub fn lock(path: PathBuf) -> Result<FileLock, FileLockError> {
        let mut options = OpenOptions::new();
        options.create_new(true);
        options.write(true);
        let mut backoff_iterator = BackoffIterator::new();
        loop {
            match options.open(&path) {
                Ok(file) => {
                    return Ok(FileLock { path, _file: file });
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::AlreadyExists
                        || (cfg!(windows)
                            && err.kind() == std::io::ErrorKind::PermissionDenied) =>
                {
                    if let Some(duration) = backoff_iterator.next() {
                        std::thread::sleep(duration);
                    } else {
                        return Err(FileLockError {
                            message: "Timed out while trying to create lock file",
                            path,
                            err,
                        });
                    }
                }
                Err(err) => {
                    return Err(FileLockError {
                        message: "Failed to create lock file",
                        path,
                        err,
                    })
                }
            }
        }
    }
}

impl Drop for FileLock {
    #[instrument(skip_all)]
    fn drop(&mut self) {
        std::fs::remove_file(&self.path)
            .inspect_err(|err| tracing::warn!(?err, ?self.path, "Failed to delete lock file"))
            .ok();
    }
}
