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

use std::fs::File;
use std::path::PathBuf;

use rustix::fs::FlockOperation;
use tracing::instrument;

use super::FileLockError;

pub struct FileLock {
    path: PathBuf,
    file: File,
}

impl FileLock {
    pub fn lock(path: PathBuf) -> Result<FileLock, FileLockError> {
        loop {
            // Create lockfile, or open pre-existing one
            let file = File::create(&path).map_err(|err| FileLockError {
                message: "Failed to open lock file",
                path: path.clone(),
                err,
            })?;
            // If the lock was already held, wait for it to be released
            rustix::fs::flock(&file, FlockOperation::LockExclusive).map_err(|errno| {
                FileLockError {
                    message: "Failed to lock lock file",
                    path: path.clone(),
                    err: errno.into(),
                }
            })?;

            let stat = rustix::fs::fstat(&file).map_err(|errno| FileLockError {
                message: "failed to stat lock file",
                path: path.clone(),
                err: errno.into(),
            })?;
            if stat.st_nlink == 0 {
                // Lockfile was deleted, probably by the previous holder's `Drop` impl; create a
                // new one so our ownership is visible, rather than hidden in an
                // unlinked file. Not always necessary, since the previous
                // holder might have exited abruptly.
                continue;
            }

            return Ok(Self { path, file });
        }
    }
}

impl Drop for FileLock {
    #[instrument(skip_all)]
    fn drop(&mut self) {
        // Removing the file isn't strictly necessary, but reduces confusion.
        _ = std::fs::remove_file(&self.path);
        // Unblock any processes that tried to acquire the lock while we held it.
        // They're responsible for creating and locking a new lockfile, since we
        // just deleted this one.
        _ = rustix::fs::flock(&self.file, FlockOperation::Unlock);
    }
}
